//! altoggle — switches the IME from a solo press of either Alt key.
//!
//! - Solo press of right Alt -> IME on
//! - Solo press of left Alt  -> IME off
//! - Alt with anything else  -> ordinary Alt, untouched
//!
//! Threads:
//! - main: a message-only window watching session changes, plus the main loop
//! - `altoggle-hooks`: every low-level hook and the state machine
//! - `altoggle-log`: the only thread allowed to block on I/O
//!
//! A press held past the threshold deliberately falls through to Windows, which
//! opens the menu bar as it always did. That keeps the normal Alt behaviour
//! reachable instead of taking it away.

// The console is wanted while developing; a resident app should not own one.
// Release builds therefore log through OutputDebugStringW only (see `log`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use altoggle_app::{hook, inject, log, session, single_instance};
use altoggle_core::Config;

use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

/// One instance per logged-on session.
const INSTANCE_NAME: &str = "altoggle-single-instance";

/// Seconds after which to quit on our own, from `--exit-after=<seconds>`.
///
/// This is the escape hatch for developing on the machine you are sitting at.
/// The app suppresses real Alt up events, so a bug that wedges the message loop
/// would otherwise need Ctrl+Alt+Del to recover from. Off unless asked for,
/// because a resident app should stay resident.
fn exit_after_secs() -> Option<u64> {
    std::env::args()
        .find_map(|a| a.strip_prefix("--exit-after=").map(str::to_owned))
        .and_then(|v| v.parse().ok())
}

/// Report and exit. Flushes the log first, or the reason never gets out.
fn fail(message: &str) -> ! {
    log::line(message);
    log::shutdown();
    std::process::exit(1);
}

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> i32 {
    // Do not let the default termination run: it would skip the cleanup that
    // releases the modifiers, and a suppressed Alt up could stay stuck down.
    session::request_quit();
    1
}

fn main() {
    log::init();

    let Some(_instance) = single_instance::acquire(INSTANCE_NAME) else {
        fail("already running in this session");
    };

    // A panic must never leave a modifier held down. A stuck Win key in
    // particular turns every following keystroke into a hotkey.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inject::release_all_modifiers();
        previous(info);
    }));

    let hooks = match hook::spawn(Config::default()) {
        Ok(h) => h,
        Err(e) => fail(&format!("could not install the hooks: {e}")),
    };

    unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), 1) };

    log::line("started");
    log::line("  right Alt (solo) -> IME on");
    log::line("  left Alt  (solo) -> IME off");
    log::line(format!(
        "  hold either past {}ms -> ordinary Alt",
        Config::default().threshold_ms
    ));

    if let Some(secs) = exit_after_secs() {
        log::line(format!("quitting automatically after {secs}s"));
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            session::request_quit();
        });
    }

    if let Err(e) = session::run() {
        log::line(format!("the session watcher failed: {e}"));
    }

    hooks.shutdown();
    inject::release_all_modifiers();
    log::line("stopped");
    log::shutdown();
}
