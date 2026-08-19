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

use std::cell::RefCell;
use std::io::{BufWriter, Write};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use altoggle_app::inject;
use altoggle_app::keys::foreign_modifier_held;
use altoggle_app::probe_args::ALTPROBE;
use altoggle_core::{Action, Config, Event, Machine};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_UP, MSG,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
};

static TX: OnceLock<Sender<Msg>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static MAIN_TID: AtomicU32 = AtomicU32::new(0);
/// The dummy key injected for option A. Swappable by argument for measurement.
static DUMMY_VK: AtomicU32 = AtomicU32::new(0x07);

thread_local! {
    /// A low-level hook callback runs on the thread that installed the hook
    /// (that is, the message loop thread). Both the keyboard and mouse hooks are
    /// installed on the same thread, so thread-local state is enough.
    static MACHINE: RefCell<Machine> = RefCell::new(Machine::new(Config::default()));
}

enum Msg {
    Line(String),
    Stop,
}

fn now_ms() -> u64 {
    START
        .get()
        .map(|s| s.elapsed().as_millis() as u64)
        .unwrap_or(0)
}

fn log(s: String) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(Msg::Line(s));
    }
}

// ---------------------------------------------------------------- hooks

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) };
    }
    let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };

    // Our own injections bypass the state machine. Forgetting this loops forever.
    if k.dwExtraInfo == inject::INJECT_TAG {
        return unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) };
    }

    let vk = k.vkCode as u16;
    let is_up = k.flags & LLKHF_UP != 0;
    let t = now_ms();

    let mut contaminated_by_prior = false;
    let action = MACHINE.with_borrow_mut(|m| {
        if is_up {
            m.on_event(Event::KeyUp(vk), t)
        } else {
            let a = m.on_event(Event::KeyDown(vk), t);
            let cfg = *m.config();
            if (vk == cfg.left_trigger || vk == cfg.right_trigger) && foreign_modifier_held(vk) {
                m.on_event(Event::ForeignKeyHeld, t);
                contaminated_by_prior = true;
            }
            a
        }
    });
    if contaminated_by_prior {
        log(format!(
            "{:>8.3}  a modifier was already held -> contaminated",
            t as f64 / 1000.0
        ));
    }

    if let Action::Fire(side) = action {
        // Fire only follows a KeyUp of the held trigger, so `vk` is that trigger.
        let dummy = DUMMY_VK.load(Ordering::Relaxed) as u16;
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
        log(format!(
            "{:>8.3}  *** FIRE {side:?}  blocked the real up -> {what}",
            t as f64 / 1000.0,
        ));
        if sent != expected {
            // A failed injection leaves the trigger held down. Release it
            // defensively: a stuck Win key turns every later keystroke into a
            // hotkey.
            log(format!(
                "{:>8.3}  !!! injection failed, releasing modifiers",
                t as f64 / 1000.0
            ));
            inject::release_stuck_keys();
        }
        return 1; // block the real up
    }

    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && matches!(
            wparam as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        )
    {
        MACHINE.with_borrow_mut(|m| m.on_event(Event::MouseButton, now_ms()));
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> i32 {
    // Letting the default termination run could leave a blocked Alt stuck down.
    // Post WM_QUIT so we go through the normal path (unhook + release modifiers).
    unsafe { PostThreadMessageW(MAIN_TID.load(Ordering::Relaxed), WM_QUIT, 0, 0) };
    1
}

// ---------------------------------------------------------------- main

fn main() {
    let args = match ALTPROBE.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}: {e}\n{}", ALTPROBE.name, ALTPROBE.usage());
            std::process::exit(2);
        }
    };
    let auto_exit_secs = args.secs;
    DUMMY_VK.store(args.dummy_vk as u32, Ordering::Relaxed);
    MACHINE.with_borrow_mut(|m| m.set_config(args.config()));

    START.set(Instant::now()).ok();
    MAIN_TID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);

    // Do not let a panic leave keys stuck down.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inject::release_stuck_keys();
        prev(info);
    }));

    let (tx, rx) = channel::<Msg>();
    TX.set(tx).ok();
    let writer = std::thread::spawn(move || {
        // Holding stdout.lock() would deadlock the main thread's println!.
        // BufWriter<Stdout> takes the lock per write, so it is safe.
        let mut out = BufWriter::new(std::io::stdout());
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Line(s) => {
                    let _ = writeln!(out, "{s}");
                    let _ = out.flush();
                }
                Msg::Stop => break,
            }
        }
    });

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

    unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), 1) };

    let hmod = unsafe { GetModuleHandleW(null_mut()) };
    let kb_hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0) };
    let ms_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0) };
    if kb_hook.is_null() || ms_hook.is_null() {
        eprintln!("SetWindowsHookExW failed");
        return;
    }

    if auto_exit_secs > 0 {
        let tid = MAIN_TID.load(Ordering::Relaxed);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(auto_exit_secs));
            unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
        });
    }

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        UnhookWindowsHookEx(kb_hook);
        UnhookWindowsHookEx(ms_hook);
    }
    inject::release_stuck_keys();
    if let Some(tx) = TX.get() {
        let _ = tx.send(Msg::Stop);
    }
    let _ = writer.join();
    println!("Stopped (modifiers released).");
}
