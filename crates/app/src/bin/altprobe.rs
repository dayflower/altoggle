//! altprobe — checks whether plan option A ("inject a dummy key") actually works.
//!
//! Does not touch the IME. It answers one question per trigger key: **does a solo
//! press stop having its usual side effect?** For Alt that side effect is the
//! menu bar; for Win it is the start menu. If suppression fails, the design falls
//! back to option B (intercepting the key down entirely), which makes this the
//! first thing to verify for any newly added trigger.
//!
//! What it does:
//! - Passes the trigger's down through to the OS untouched, so Alt+X and Win+E
//!   behave exactly as before
//! - On detecting a solo up, **blocks** that up and injects the suppression plus
//!   a replacement up in **one SendInput call**
//!
//! Because this is a dangerous thing to run, there are three escape hatches:
//! - Exits automatically after 90s by default (`--secs` overrides)
//! - Ctrl+C goes through the normal shutdown path
//! - Normal exit and panic both inject an up for every modifier before finishing
//!
//! `--dry-run` prints what would be used and installs no hook. Use it to confirm
//! you are running the build you think you are before arming anything.
//!
//! Usage:
//!   altprobe [--secs=N] [--dummy=HEX] [--left=KEY] [--right=KEY] [--threshold=MS]
//!            [--dry-run]
//!   e.g. altprobe --left=LeftWin --right=RightWin

use std::ptr::null_mut;

use altoggle_app::dispatch::start_clock;
use altoggle_app::inject;
use altoggle_app::lowlevel::{self, Callbacks, Fire};
use altoggle_app::probe_args::ALTPROBE;
use altoggle_app::{probe_exit, probe_log};

use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

/// What a completed solo press does here: the suppression, and nothing else.
///
/// **Nothing in here may touch `ime`.** Leaving the IME alone is the reason
/// altprobe exists: it isolates the question of whether the suppression works
/// from the question of whether switching the IME works.
fn on_fire(dummy: u16, f: Fire) {
    let Fire {
        side,
        trigger_vk: vk,
        at,
    } = f;
    let batch = inject::suppress(dummy, vk);
    let (sent, expected) = inject::send_batch(&batch);
    let what = match inject::suppression_for(vk) {
        inject::Suppression::DummyThenUp => format!(
            "injected [0x{dummy:02X} down, 0x{dummy:02X} up, 0x{vk:02X} up] \
             (SendInput={sent}/{expected})"
        ),
        // Nothing goes out: the up is the side effect, so replaying it would
        // perform exactly what the suppression is for.
        inject::Suppression::Swallow => "injected nothing (swallowed)".to_string(),
    };
    probe_log::line(format!(
        "{:>8.3}  *** FIRE {side:?}  blocked the real up -> {what}",
        at as f64 / 1000.0,
    ));
    if sent != expected {
        // A failed injection leaves the trigger held down. Release it
        // defensively: a stuck Win key turns every later keystroke into a
        // hotkey.
        probe_log::line(format!(
            "{:>8.3}  !!! injection failed, releasing modifiers",
            at as f64 / 1000.0
        ));
        inject::release_stuck_keys();
    }
}

fn main() {
    let args = match ALTPROBE.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}: {e}\n{}", ALTPROBE.name, ALTPROBE.usage());
            std::process::exit(2);
        }
    };
    let auto_exit_secs = args.secs;
    let dummy = args.dummy_vk;

    start_clock();
    lowlevel::set_config(args.config());

    // Do not let a panic leave keys stuck down.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inject::release_stuck_keys();
        prev(info);
    }));

    println!("altprobe - verifying option A (dummy key injection)");
    println!("{}", args.describe());
    if args.dry_run {
        println!("--dry-run: no hook installed, nothing intercepted.");
        return;
    }
    println!("Press a trigger alone and watch whether its usual side effect happens");
    println!("(Alt: the menu bar. Win: the start menu). Check every app that matters.");
    println!("The IME is not switched yet. Chords such as Alt+X and Win+E must be unchanged.");
    println!(
        "Quit: Ctrl+C / automatic exit after {auto_exit_secs}s / last resort is killing the process from Ctrl+Alt+Del"
    );
    println!("{:-<100}", "");

    probe_log::init();
    probe_exit::arm(auto_exit_secs);

    let callbacks = Callbacks::new(move |f| on_fire(dummy, f)).reporting_contamination(|at| {
        probe_log::line(format!(
            "{:>8.3}  a modifier was already held -> contaminated",
            at as f64 / 1000.0
        ))
    });
    let Some(hooks) = lowlevel::install(callbacks) else {
        eprintln!("SetWindowsHookExW failed");
        return;
    };

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    lowlevel::uninstall(&hooks);
    inject::release_stuck_keys();
    probe_log::shutdown();
    println!("Stopped (modifiers released).");
}
