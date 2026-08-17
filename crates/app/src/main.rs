//! altoggle — switches the IME from a solo press of either Alt key.
//!
//! - Solo press of right Alt -> IME on
//! - Solo press of left Alt  -> IME off
//! - Alt with anything else  -> ordinary Alt, untouched
//!
//! Threads:
//! - main: the tray icon, a message-only window watching session changes, and
//!   the message loop that drives both
//! - `altoggle-hooks`: every low-level hook and the state machine
//! - `altoggle-log`: the only thread allowed to block on I/O
//!
//! A press held past the threshold deliberately falls through to Windows, which
//! opens the menu bar as it always did. That keeps the normal Alt behaviour
//! reachable instead of taking it away.

// The console is wanted while developing; a resident app should not own one.
// Release builds therefore log through OutputDebugStringW only (see `log`).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use altoggle_app::settings::Loaded;
use altoggle_app::tray::{Command, Tray};
use altoggle_app::{autostart, dialog, hook, inject, log, session, settings, single_instance};

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

/// Load the config, reporting what happened. Never fails: bad settings must not
/// cost the user the ability to type.
fn load_settings() -> settings::Settings {
    let loaded = settings::load();
    match &loaded {
        Loaded::Existing(_) => log::line("config loaded"),
        Loaded::Created(_) => log::line(format!(
            "wrote a default config to {}",
            settings::config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )),
        Loaded::Failed(_, why) => log::line(format!("using defaults: {why}")),
    }
    loaded.settings()
}

/// Warn when autostart points at a different executable than the running one.
///
/// The tick would otherwise claim "starts with Windows" while silently launching
/// some other build, which is exactly the kind of thing you discover months later.
fn report_stale_autostart() {
    let (Some(registered), Ok(current)) =
        (autostart::registered_command(), autostart::command_for_this_exe())
    else {
        return;
    };
    if registered != current {
        log::line(format!(
            "autostart points at {registered}, not this executable ({current}); \
             toggle it off and on to repoint it"
        ));
    }
}

/// Act on the autostart tick the user just flipped, undoing it if the registry
/// write fails so the menu cannot claim something untrue.
fn toggle_autostart(tray: &Tray) {
    let wanted = tray.autostart_checked();
    match autostart::set_enabled(wanted) {
        Ok(()) => log::line(if wanted {
            "autostart enabled"
        } else {
            "autostart disabled"
        }),
        Err(e) => {
            log::line(format!("could not change autostart: {e}"));
            tray.set_autostart_checked(!wanted);
        }
    }
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
        inject::release_stuck_keys();
        previous(info);
    }));

    let settings = load_settings();
    let hooks = match hook::spawn(settings.runtime()) {
        Ok(h) => h,
        Err(e) => fail(&format!("could not install the hooks: {e}")),
    };

    // Without a tray icon there would be no way to quit a release build, which
    // has no console and so no Ctrl+C.
    let tray = match Tray::new("altoggle", autostart::is_enabled()) {
        Ok(t) => t,
        Err(e) => {
            hooks.shutdown();
            fail(&format!("could not create the tray icon: {e}"));
        }
    };
    report_stale_autostart();

    unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), 1) };

    log::line("started");
    log::line(format!(
        "  solo {} -> IME off, solo {} -> IME on, hold past {}ms -> ordinary key",
        settings::slot_name(settings.left_trigger),
        settings::slot_name(settings.right_trigger),
        settings.threshold_ms
    ));

    if let Some(secs) = exit_after_secs() {
        log::line(format!("quitting automatically after {secs}s"));
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            session::request_quit();
        });
    }

    // What the hook thread is running, so the dialog opens showing the values
    // that are actually in effect rather than re-reading the file.
    let mut current = settings;
    let result = session::run(|| {
        for command in tray.poll() {
            match command {
                Command::OpenSettings => dialog::open(current),
                Command::ReinstallHooks => hook::request_reinstall(),
                Command::ToggleAutostart => toggle_autostart(&tray),
                Command::Quit => session::request_quit(),
            }
        }
        // The dialog cannot reach the hook thread itself, so it leaves what the
        // user committed here. `set_config` stays the only way in.
        for applied in dialog::poll() {
            current = applied;
            hooks.set_config(applied.runtime());
        }
    });
    if let Err(e) = result {
        log::line(format!("the session watcher failed: {e}"));
    }

    hooks.shutdown();
    inject::release_stuck_keys();
    log::line("stopped");
    log::shutdown();
}
