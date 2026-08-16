//! The hook thread: where every low-level hook lives.
//!
//! Everything here runs on one dedicated thread, because a low-level hook
//! callback is delivered on the thread that installed the hook, and that thread
//! must be pumping a message loop. Keeping it separate from the rest of the app
//! means nothing else can make the callback late: exceeding
//! `LowLevelHooksTimeout` (300ms by default) makes Windows drop the hook without
//! telling anyone.
//!
//! Rules for the callback:
//! - No `SendMessage`, no COM, no file or console I/O. `crate::log` only queues
//! - Our own injected events are filtered out by the `dwExtraInfo` tag before
//!   they reach the state machine, or they would loop forever
//!
//! Other threads talk to this one with `PostThreadMessage`; the message loop
//! applies the change. That keeps configuration updates out of the callback.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::channel;
use std::thread::JoinHandle;
use std::time::Instant;

use altoggle_core::{Action, Config, Event, Machine, Side};

use crate::settings::Runtime;
use crate::{ime, inject, log};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Accessibility::{
    HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, HC_ACTION, HHOOK,
    KBDLLHOOKSTRUCT, LLKHF_UP, MSG, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT, WM_APP,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
};

/// Replace the running configuration.
///
/// `wParam` is a `Box<Runtime>` leaked with `Box::into_raw`; the loop takes
/// ownership back. Posting is the only supported way in, because
/// `Machine::set_config` must not run inside the hook callback.
const WM_APP_SET_CONFIG: u32 = WM_APP + 1;
/// Tear the hooks down and install them again.
const WM_APP_REINSTALL: u32 = WM_APP + 2;

static HOOK_TID: AtomicU32 = AtomicU32::new(0);
static START: OnceLock<Instant> = OnceLock::new();
/// The key injected to make a solo press stop looking solo.
///
/// Atomic rather than thread-local: the callback reads it, and it is a plain
/// value with no invariant tying it to the state machine.
static DUMMY_VK: AtomicU32 = AtomicU32::new(0x07);

thread_local! {
    /// A low-level hook callback runs on the thread that installed the hook, and
    /// so does an out-of-context WinEvent callback. Everything that touches this
    /// is therefore on one thread, and thread-local state is enough.
    static MACHINE: RefCell<Machine> = RefCell::new(Machine::new(Config::default()));
    static HOOKS: RefCell<Option<Hooks>> = const { RefCell::new(None) };
}

struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
    foreground: HWINEVENTHOOK,
}

fn now_ms() -> u64 {
    START
        .get()
        .map(|s| s.elapsed().as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- callbacks

/// Was another modifier already held when the trigger went down?
///
/// A key pressed before the trigger has already had its down event delivered, so
/// the state machine can never see it. `GetAsyncKeyState` is the only way to
/// notice, and it is cheap enough for the callback (no cross-process message).
///
/// The trigger itself is excluded, or a Ctrl or Shift trigger would report
/// itself as held and never fire.
fn foreign_modifier_held(trigger_vk: u16) -> bool {
    const MODIFIERS: [u16; 8] = [
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
    ];
    MODIFIERS.iter().any(|&vk| {
        vk != trigger_vk && unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
    })
}

/// Suppress the menu bar and switch the IME, in a single `SendInput` call.
///
/// The order matters, so it has to be one call: `SendInput` guarantees the whole
/// array is delivered without other input interleaving, while three separate
/// calls can be cut apart by a real keystroke.
fn fire(side: Side, trigger_vk: u16) -> u32 {
    let dummy = DUMMY_VK.load(Ordering::Relaxed) as u16;
    let ime_vk = match side {
        Side::Right => ime::VK_IME_ON,
        Side::Left => ime::VK_IME_OFF,
    };
    inject::send(&[
        inject::key_input(dummy, false, false),
        inject::key_input(dummy, true, false),
        inject::key_input(trigger_vk, true, inject::is_extended_trigger(trigger_vk)),
        inject::key_input(ime_vk, false, false),
        inject::key_input(ime_vk, true, false),
    ])
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) };
    }
    let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };

    // Our own injections bypass the state machine entirely. Without this the
    // injected Alt up would fire the machine again and loop forever.
    if k.dwExtraInfo == inject::INJECT_TAG {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) };
    }

    let vk = k.vkCode as u16;
    let is_up = k.flags & LLKHF_UP != 0;
    let t = now_ms();

    let action = MACHINE.with_borrow_mut(|m| {
        if is_up {
            m.on_event(Event::KeyUp(vk), t)
        } else {
            let a = m.on_event(Event::KeyDown(vk), t);
            let cfg = *m.config();
            if (vk == cfg.left_trigger || vk == cfg.right_trigger) && foreign_modifier_held(vk) {
                m.on_event(Event::ForeignKeyHeld, t);
            }
            a
        }
    });

    if let Action::Fire(side) = action {
        // Fire only follows a KeyUp of the held trigger, so `vk` is that trigger.
        let sent = fire(side, vk);
        if sent == 5 {
            log::line(format!("fire {side:?}"));
        } else {
            // A partial injection can leave Alt held down, which would turn every
            // following keystroke into an Alt combination.
            log::line(format!(
                "fire {side:?} FAILED (SendInput={sent}/5), releasing modifiers"
            ));
            inject::release_all_modifiers();
        }
        // Swallow the real Alt up. The replacement was injected above.
        return 1;
    }

    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && matches!(
            wparam as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        // Alt+drag and Alt+click are not solo presses.
        MACHINE.with_borrow_mut(|m| m.on_event(Event::MouseButton, now_ms()));
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Foreground changed, so any half-finished press belongs to a window that is no
/// longer there. Drop it rather than firing into whatever took its place.
unsafe extern "system" fn foreground_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    _hwnd: windows_sys::Win32::Foundation::HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        MACHINE.with_borrow_mut(|m| m.on_event(Event::Reset, now_ms()));
    }
}

// ---------------------------------------------------------------- install

fn install() -> Option<Hooks> {
    let hmod = unsafe { GetModuleHandleW(std::ptr::null()) };
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0) };
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0) };
    // Out-of-context means the callback is delivered on this thread through the
    // message loop, so it can touch the thread-local state machine.
    let foreground = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(foreground_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if keyboard.is_null() || mouse.is_null() {
        if !keyboard.is_null() {
            unsafe { UnhookWindowsHookEx(keyboard) };
        }
        if !mouse.is_null() {
            unsafe { UnhookWindowsHookEx(mouse) };
        }
        if !foreground.is_null() {
            unsafe { UnhookWinEvent(foreground) };
        }
        return None;
    }
    Some(Hooks {
        keyboard,
        mouse,
        foreground,
    })
}

fn uninstall(h: &Hooks) {
    unsafe {
        UnhookWindowsHookEx(h.keyboard);
        UnhookWindowsHookEx(h.mouse);
        if !h.foreground.is_null() {
            UnhookWinEvent(h.foreground);
        }
    }
}

/// Drop the hooks and install them again.
///
/// Windows removes a low-level hook silently when the callback is too slow, and
/// hooks can also be lost across session transitions. There is no API that
/// reports "your hook is gone", so reinstalling on the events we *can* observe
/// (unlock, remote connect, and so on) is the available defence.
fn reinstall() {
    HOOKS.with_borrow_mut(|slot| {
        if let Some(old) = slot.take() {
            uninstall(&old);
        }
        match install() {
            Some(new) => {
                *slot = Some(new);
                log::line("hooks reinstalled");
            }
            None => log::line("hook reinstall FAILED"),
        }
    });
    // The old hooks saw a press we will never see the release of.
    MACHINE.with_borrow_mut(|m| m.on_event(Event::Reset, now_ms()));
}

// ---------------------------------------------------------------- thread

pub struct HookThread {
    join: Option<JoinHandle<()>>,
}

/// Apply a runtime config. Only ever called from the hook thread.
fn apply(rt: Runtime) {
    DUMMY_VK.store(rt.dummy_vk as u32, Ordering::Relaxed);
    MACHINE.with_borrow_mut(|m| m.set_config(rt.core));
}

/// Start the hook thread and wait until the hooks are actually installed.
pub fn spawn(rt: Runtime) -> Result<HookThread, String> {
    START.get_or_init(Instant::now);
    let (ready_tx, ready_rx) = channel::<Result<u32, String>>();

    let join = std::thread::Builder::new()
        .name("altoggle-hooks".into())
        .spawn(move || {
            apply(rt);

            let installed = install();
            let ok = installed.is_some();
            HOOKS.with_borrow_mut(|slot| *slot = installed);

            let tid = unsafe { GetCurrentThreadId() };
            let _ = ready_tx.send(if ok {
                Ok(tid)
            } else {
                Err("SetWindowsHookExW failed".into())
            });
            if !ok {
                return;
            }

            let mut msg: MSG = unsafe { std::mem::zeroed() };
            while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                match msg.message {
                    WM_APP_SET_CONFIG => {
                        // Take back ownership of the Runtime the sender leaked.
                        let rt = unsafe { *Box::from_raw(msg.wParam as *mut Runtime) };
                        apply(rt);
                        log::line(format!(
                            "config applied: triggers 0x{:02X}/0x{:02X}, {}ms, dummy 0x{:02X}",
                            rt.core.left_trigger,
                            rt.core.right_trigger,
                            rt.core.threshold_ms,
                            rt.dummy_vk
                        ));
                    }
                    WM_APP_REINSTALL => reinstall(),
                    _ => unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    },
                }
            }

            HOOKS.with_borrow_mut(|slot| {
                if let Some(h) = slot.take() {
                    uninstall(&h);
                }
            });
        })
        .map_err(|e| format!("could not start the hook thread: {e}"))?;

    match ready_rx.recv() {
        Ok(Ok(tid)) => {
            HOOK_TID.store(tid, Ordering::SeqCst);
            Ok(HookThread { join: Some(join) })
        }
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => Err("the hook thread died during startup".into()),
    }
}

fn post(msg: u32, wparam: WPARAM) -> bool {
    let tid = HOOK_TID.load(Ordering::SeqCst);
    tid != 0 && unsafe { PostThreadMessageW(tid, msg, wparam, 0) } != 0
}

/// Ask the hook thread to reinstall its hooks. Safe to call from any thread.
pub fn request_reinstall() {
    post(WM_APP_REINSTALL, 0);
}

impl HookThread {
    /// Apply a new configuration. Safe to call from any thread.
    ///
    /// The `Runtime` is leaked into the message and reclaimed by the loop. If the
    /// post fails the thread is already gone, so reclaim it here instead.
    pub fn set_config(&self, rt: Runtime) {
        let boxed = Box::into_raw(Box::new(rt));
        if !post(WM_APP_SET_CONFIG, boxed as WPARAM) {
            drop(unsafe { Box::from_raw(boxed) });
        }
    }

    /// Stop the loop, uninstall the hooks, and wait for the thread to finish.
    pub fn shutdown(mut self) {
        post(WM_QUIT, 0);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
