//! imeprobe — verifies IME switching.
//!
//! Option A's suppression, as confirmed by `altprobe`, plus the IME operation.
//! This is essentially the app itself, minus the tray icon and the config file.
//!
//! - Solo press of the right trigger -> `VK_IME_ON`
//! - Solo press of the left trigger  -> `VK_IME_OFF`
//!
//! There is no layout check. Injecting the IME keys under en-US was measured to
//! do nothing, so "do nothing when no Japanese IME is active" holds without one.
//!
//! Every fire prints the IME open state as read back through IMM32. A `?` there
//! is expected for TSF-only apps, UWP, and some Electron apps.
//! **Whether `VK_IME_ON` worked is ultimately judged by eye** (can you actually
//! type Japanese).
//!
//! Usage:
//!   imeprobe [--secs=N] [--dummy=HEX] [--left=KEY] [--right=KEY] [--threshold=MS]
//!            [--split] [--dry-run]
//!   --split:   send the suppression and the IME keys as two separate SendInput
//!              calls (the default batches them into one)
//!   --dry-run: print what would be used and install no hook

use std::ptr::null_mut;
use std::time::Duration;

use altoggle_app::dispatch::start_clock;
use altoggle_app::ime;
use altoggle_app::inject::{self, key_input};
use altoggle_app::lowlevel::{self, Callbacks, Fire};
use altoggle_app::probe_args::IMEPROBE;
use altoggle_app::{probe_exit, probe_log};
use altoggle_core::Side;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

/// Injection on fire. Returns (events sent, events expected).
fn inject_for(dummy: u16, split: bool, side: Side, trigger_vk: u16) -> (u32, u32) {
    let ime_vk = match side {
        Side::Right => ime::VK_IME_ON,
        Side::Left => ime::VK_IME_OFF,
    };

    let suppression = inject::suppress(dummy, trigger_vk);
    if split {
        let (a, expected) = inject::send_batch(&suppression);
        let b = ime::set_open(matches!(side, Side::Right));
        (a + b, expected + 2)
    } else {
        // The order carries meaning, so batch it into a single SendInput call.
        // The IME keys come after the suppression, never before: while the
        // trigger is still held they would read as a chord, and Win+key is a
        // hotkey.
        let mut batch = suppression;
        batch.push(key_input(ime_vk, false));
        batch.push(key_input(ime_vk, true));
        inject::send_batch(&batch)
    }
}

/// What a completed solo press does here: suppress, switch the IME, and report
/// what the IME says afterwards.
fn on_fire(dummy: u16, split: bool, f: Fire) {
    let Fire {
        side,
        trigger_vk,
        at,
    } = f;
    let (sent, expected) = inject_for(dummy, split, side, trigger_vk);
    let at = at as f64 / 1000.0;
    probe_log::deferred(move || {
        // IMM32 is read only here, never from the hook callback.
        // Give the switch a moment to take effect.
        std::thread::sleep(Duration::from_millis(150));
        let status = match ime::read_open_status() {
            Some(true) => "ON",
            Some(false) => "OFF",
            None => "?(unreadable)",
        };
        let what = match side {
            Side::Right => "injected IME_ON",
            Side::Left => "injected IME_OFF",
        };
        format!(
            "{at:>8.3}  FIRE {side:?}  {what}  SendInput={sent}/{expected}  -> read back: IME={status}"
        )
    });
    if sent != expected {
        inject::release_stuck_keys();
        probe_log::line("!!! injection failed, modifiers released");
    }
}

fn main() {
    let args = match IMEPROBE.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}: {e}\n{}", IMEPROBE.name, IMEPROBE.usage());
            std::process::exit(2);
        }
    };
    let auto_exit_secs = args.secs;
    let dummy = args.dummy_vk;
    let split = args.split;

    start_clock();
    lowlevel::set_config(args.config());

    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inject::release_stuck_keys();
        prev(info);
    }));

    println!("imeprobe - verifying IME switching");
    println!("{}", args.describe());
    println!("injection: {}", if split { "split" } else { "batch" });
    if args.dry_run {
        println!("--dry-run: no hook installed, nothing intercepted.");
        return;
    }
    println!(
        "Solo {} -> IME ON   /   solo {} -> IME OFF",
        args.right.name(),
        args.left.name()
    );
    println!("No layout check (the IME keys do nothing under en-US anyway).");
    println!(
        "Quit: Ctrl+C / automatic exit after {auto_exit_secs}s / last resort is killing the process from Ctrl+Alt+Del"
    );
    println!("{:-<100}", "");

    probe_log::init();
    probe_exit::arm(auto_exit_secs);

    let Some(hooks) = lowlevel::install(Callbacks::new(move |f| on_fire(dummy, split, f))) else {
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
