//! The hook thread: where every low-level hook lives.
//!
//! Everything here runs on one dedicated thread, because a low-level hook
//! callback is delivered on the thread that installed the hook, and that thread
//! must be pumping a message loop. Keeping it separate from the rest of the app
//! means nothing else can make the callback late: exceeding
//! `LowLevelHooksTimeout` (300ms by default) makes Windows drop the hook without
//! telling anyone.
//!
//! The keyboard and mouse hooks themselves are [`crate::lowlevel`], shared with
//! the probes. What stays here is everything the probes have no use for: the
//! message loop, the configuration that arrives through it, reinstalling, and
//! the foreground WinEvent.
//!
//! Rules for the callback:
//! - No `SendMessage`, no COM, no file or console I/O. `crate::log` only queues
//! - No `Machine::set_config`. Config changes arrive as a posted message and are
//!   applied by the loop below
//!
//! Other threads talk to this one with `PostThreadMessage`; the message loop
//! applies the change. That keeps configuration updates out of the callback.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::channel;
use std::thread::JoinHandle;

use altoggle_core::Side;

use crate::dispatch::start_clock;
use crate::lowlevel::{Callbacks, Fire};
use crate::settings::Runtime;
use crate::{ime, inject, log, lowlevel};

use windows_sys::Win32::Foundation::WPARAM;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetMessageW, MSG, PostThreadMessageW,
    TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_APP, WM_QUIT,
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
/// The key injected to make a solo press stop looking solo.
///
/// Atomic rather than thread-local: the callback reads it, the message loop
/// writes it, and it is a plain value with no invariant tying it to the state
/// machine.
static DUMMY_VK: AtomicU32 = AtomicU32::new(0x07);

thread_local! {
    /// Owned by the hook thread, like everything else the callbacks reach.
    static HOOKS: RefCell<Option<Hooks>> = const { RefCell::new(None) };
}

struct Hooks {
    input: lowlevel::Hooks,
    foreground: HWINEVENTHOOK,
}

// ---------------------------------------------------------------- callbacks

/// Suppress the trigger's usual side effect and switch the IME, in a single
/// `SendInput` call.
///
/// The order matters, so it has to be one call: `SendInput` guarantees the whole
/// array is delivered without other input interleaving, while three separate
/// calls can be cut apart by a real keystroke.
///
/// The IME keys go **after** the suppression, never before: injected while the
/// trigger is still held they would read as a chord, and Win+key chords are
/// hotkeys.
///
/// How many events that is depends on the trigger — a `Swallow` trigger
/// contributes none — so the count is returned rather than assumed.
fn fire(side: Side, trigger_vk: u16) -> (u32, u32) {
    let dummy = DUMMY_VK.load(Ordering::Relaxed) as u16;
    let ime_vk = match side {
        Side::Right => ime::VK_IME_ON,
        Side::Left => ime::VK_IME_OFF,
    };
    let mut batch = inject::suppress(dummy, trigger_vk);
    batch.push(inject::key_input(ime_vk, false));
    batch.push(inject::key_input(ime_vk, true));
    inject::send_batch(&batch)
}

/// What a completed solo press does in the app. Runs inside the hook callback,
/// which is why it only queues its logging.
fn on_fire(f: Fire) {
    let side = f.side;
    let (sent, expected) = fire(side, f.trigger_vk);
    if sent == expected {
        log::line(format!("fire {side:?}"));
    } else {
        // A partial injection can leave the trigger held down, which for a
        // modifier turns every following keystroke into a chord.
        log::line(format!(
            "fire {side:?} FAILED (SendInput={sent}/{expected}), releasing modifiers"
        ));
        inject::release_stuck_keys();
    }
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
        lowlevel::reset();
    }
}

// ---------------------------------------------------------------- install

fn install() -> Option<Hooks> {
    let input = lowlevel::install(Callbacks::new(on_fire))?;
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
    Some(Hooks { input, foreground })
}

fn uninstall(h: &Hooks) {
    lowlevel::uninstall(&h.input);
    if !h.foreground.is_null() {
        unsafe { UnhookWinEvent(h.foreground) };
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
    lowlevel::reset();
}

// ---------------------------------------------------------------- thread

pub struct HookThread {
    join: Option<JoinHandle<()>>,
}

/// Apply a runtime config. Only ever called from the hook thread.
fn apply(rt: Runtime) {
    DUMMY_VK.store(rt.dummy_vk as u32, Ordering::Relaxed);
    lowlevel::set_config(rt.core);
}

/// Start the hook thread and wait until the hooks are actually installed.
pub fn spawn(rt: Runtime) -> Result<HookThread, String> {
    start_clock();
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
