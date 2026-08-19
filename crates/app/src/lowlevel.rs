//! The two low-level hooks, and the callback behind them.
//!
//! `WH_KEYBOARD_LL` and `WH_MOUSE_LL` are installed the same way by the app and
//! by both probes, and their callbacks used to differ in one thing only: what
//! goes out once the state machine says `Fire`. That difference is
//! [`Callbacks`]. Everything around it — dropping our own injections, feeding
//! the machine, blocking the real up — is here once, so the invariants that
//! AGENTS.md calls out live in one place.
//!
//! What is deliberately **not** here is the message loop. `hook.rs` pumps
//! `WM_APP_SET_CONFIG`, `WM_APP_REINSTALL` and an out-of-context WinEvent that
//! the probes have no use for, so each binary keeps its own loop and only the
//! hooks themselves are shared.
//!
//! A low-level hook callback is delivered on the thread that installed the hook,
//! so the machine and the callbacks live in a `thread_local`. [`install`],
//! [`set_config`] and [`reset`] all have to be called on that same thread.
//!
//! Rules for the callback (AGENTS.md "Rules for the hook callback"): no
//! `SendMessage`, no COM, no file or console I/O, no IMM32. Exceeding
//! `LowLevelHooksTimeout` (300ms by default) makes Windows drop the hook without
//! telling anyone.

use std::cell::RefCell;

use altoggle_core::{Action, Config, Event, Machine, Side};

use crate::dispatch::{dispatch, is_button_down, now_ms};
use crate::inject;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_UP, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
};

/// A solo press that completed, as handed to [`Callbacks`].
pub struct Fire {
    /// Right means "IME on" to everything that switches the IME.
    pub side: Side,
    /// The trigger that fired. `Fire` only ever follows the `KeyUp` of the held
    /// trigger, so this is the key whose up is about to be blocked.
    pub trigger_vk: u16,
    /// When the machine saw the up, in milliseconds on the shared clock. The
    /// probes print it; the app does not.
    pub at: u64,
}

/// What a binary does with what the shared callback decides.
///
/// The `Fn` bound is not incidental: `SendInput` from inside a hook callback can
/// re-enter that callback, and a shared borrow survives that where a mutable one
/// would panic. (Our own injections turn back at the `INJECT_TAG` check before
/// reaching any of this, but that is a second line of defence, not the first.)
pub struct Callbacks {
    fire: Box<dyn Fn(Fire)>,
    contaminated: Box<dyn Fn(u64)>,
}

impl Callbacks {
    /// `fire` is what replaces the trigger's usual side effect: the suppression,
    /// plus whatever else this binary exists to do. The real up is blocked
    /// whether or not `fire` sends anything.
    pub fn new(fire: impl Fn(Fire) + 'static) -> Self {
        Self {
            fire: Box::new(fire),
            contaminated: Box::new(|_| {}),
        }
    }

    /// Also report a trigger pressed while another modifier was already held,
    /// with the timestamp of the press.
    ///
    /// Such a press can no longer fire, which looks exactly like a bug from the
    /// outside. `altprobe` says so out loud; the app and `imeprobe` do not care.
    pub fn reporting_contamination(mut self, report: impl Fn(u64) + 'static) -> Self {
        self.contaminated = Box::new(report);
        self
    }
}

thread_local! {
    static MACHINE: RefCell<Machine> = RefCell::new(Machine::new(Config::default()));
    static CALLBACKS: RefCell<Option<Callbacks>> = const { RefCell::new(None) };
}

/// Run `f` on the installed callbacks, if there are any.
fn with_callbacks(f: impl FnOnce(&Callbacks)) {
    CALLBACKS.with_borrow(|slot| {
        if let Some(callbacks) = slot {
            f(callbacks);
        }
    });
}

// ---------------------------------------------------------------- callbacks

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) };
    }
    let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };

    // Our own injections bypass the state machine entirely. Without this the
    // injected trigger up would fire the machine again and loop forever.
    if k.dwExtraInfo == inject::INJECT_TAG {
        return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) };
    }

    let vk = k.vkCode as u16;
    let is_up = k.flags & LLKHF_UP != 0;
    let at = now_ms();

    let (action, contaminated) = MACHINE.with_borrow_mut(|m| dispatch(m, vk, is_up, at));
    if contaminated {
        with_callbacks(|c| (c.contaminated)(at));
    }

    if let Action::Fire(side) = action {
        with_callbacks(|c| {
            (c.fire)(Fire {
                side,
                trigger_vk: vk,
                at,
            })
        });
        // Swallow the real up. Whatever had to replace it went out above.
        return 1;
    }

    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && is_button_down(wparam) {
        MACHINE.with_borrow_mut(|m| m.on_event(Event::MouseButton, now_ms()));
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

// ---------------------------------------------------------------- install

/// Both low-level hooks, for as long as the caller keeps this.
pub struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
}

/// Install both hooks on the calling thread, routing what they decide to
/// `callbacks`. `None` means nothing was installed.
///
/// The caller must pump a message loop on this thread afterwards, or the
/// callbacks are never delivered.
pub fn install(callbacks: Callbacks) -> Option<Hooks> {
    // In place before the hooks are, or the first fire would find nothing here.
    CALLBACKS.with_borrow_mut(|slot| *slot = Some(callbacks));

    let hmod = unsafe { GetModuleHandleW(std::ptr::null()) };
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0) };
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0) };
    if keyboard.is_null() || mouse.is_null() {
        // Half installed is worse than not installed: the keyboard hook without
        // the mouse hook would fire on Alt+drag.
        if !keyboard.is_null() {
            unsafe { UnhookWindowsHookEx(keyboard) };
        }
        if !mouse.is_null() {
            unsafe { UnhookWindowsHookEx(mouse) };
        }
        return None;
    }
    Some(Hooks { keyboard, mouse })
}

pub fn uninstall(h: &Hooks) {
    unsafe {
        UnhookWindowsHookEx(h.keyboard);
        UnhookWindowsHookEx(h.mouse);
    }
}

/// Replace the machine's configuration.
///
/// Never from inside the callback: applying a config is the message loop's
/// business (AGENTS.md "Rules for the hook callback").
pub fn set_config(config: Config) {
    MACHINE.with_borrow_mut(|m| m.set_config(config));
}

/// Throw away a half-finished press, for when the press can no longer be
/// completed: the foreground moved, or the hooks were reinstalled under it.
pub fn reset() {
    MACHINE.with_borrow_mut(|m| m.on_event(Event::Reset, now_ms()));
}
