//! The parts of the hook wiring that hold no Win32 handle.
//!
//! The app and both probes install the same two low-level hooks and feed the
//! same state machine; only what they do with the resulting `Action` differs.
//! What is here is everything those three callbacks did identically, so that the
//! invariants inside them exist once.
//!
//! Installing the hooks and the callbacks themselves stay with each binary: they
//! own their `thread_local` machine and their message loop, and `hook.rs` has
//! responsibilities (config changes, reinstalling, the foreground WinEvent) the
//! probes deliberately do not.

use std::sync::OnceLock;
use std::time::Instant;

use altoggle_core::{Action, Event, Machine};

use crate::keys::foreign_modifier_held;

use windows_sys::Win32::Foundation::WPARAM;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
};

static START: OnceLock<Instant> = OnceLock::new();

/// Fix the zero of the timestamps the machine is fed. Idempotent.
pub fn start_clock() {
    START.get_or_init(Instant::now);
}

/// Milliseconds since `start_clock`, or 0 before it was ever called.
///
/// Never a panic and never a wait: this is read from the hook callback, which
/// has `LowLevelHooksTimeout` (300ms) for everything it does.
pub fn now_ms() -> u64 {
    START
        .get()
        .map(|s| s.elapsed().as_millis() as u64)
        .unwrap_or(0)
}

/// Feed one raw key event to the machine.
///
/// Returns the action, and whether another modifier was found already held —
/// `altprobe` reports the second so that a non-fire can be told from a bug; the
/// app and `imeprobe` ignore it.
///
/// `ForeignKeyHeld` has to follow the `KeyDown`, not replace it: the machine
/// needs the press before it can be told the press is contaminated. The check
/// itself only makes sense on a trigger, and `foreign_modifier_held` excludes
/// the trigger from its own list, or a Ctrl, Shift, or Win trigger would report
/// itself as held and never fire.
///
/// Filtering out our own injections is **not** done here. That reads
/// `KBDLLHOOKSTRUCT.dwExtraInfo`, which is the callback's business; this takes a
/// bare virtual key and a direction.
pub fn dispatch(m: &mut Machine, vk: u16, is_up: bool, t: u64) -> (Action, bool) {
    if is_up {
        return (m.on_event(Event::KeyUp(vk), t), false);
    }
    let action = m.on_event(Event::KeyDown(vk), t);
    let cfg = *m.config();
    if (vk == cfg.left_trigger || vk == cfg.right_trigger) && foreign_modifier_held(vk) {
        m.on_event(Event::ForeignKeyHeld, t);
        return (action, true);
    }
    (action, false)
}

/// Is this mouse message a button going down?
///
/// Alt+drag and Alt+click are not solo presses, so any of the four ends one.
pub fn is_button_down(wparam: WPARAM) -> bool {
    matches!(
        wparam as u32,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    )
}
