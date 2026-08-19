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

use std::cell::RefCell;
use std::io::{BufWriter, Write};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use altoggle_app::ime;
use altoggle_app::inject::{self, key_input};
use altoggle_app::keys::foreign_modifier_held;
use altoggle_app::probe_args::{self, ProbeArgs};
use altoggle_core::{Action, Config, Event, Machine, Side};

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
static DUMMY_VK: AtomicU32 = AtomicU32::new(0x07);
static SPLIT: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// A low-level hook callback runs on the thread that installed the hook
    /// (that is, the message loop thread). Both the keyboard and mouse hooks are
    /// installed on the same thread, so thread-local state is enough.
    static MACHINE: RefCell<Machine> = RefCell::new(Machine::new(Config::default()));
}

enum Msg {
    Line(String),
    Fired {
        at: f64,
        side: Side,
        sent: u32,
        expected: u32,
    },
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

/// Injection on fire. Returns (events sent, events expected).
fn fire(side: Side, trigger_vk: u16) -> (u32, u32) {
    let dummy = DUMMY_VK.load(Ordering::Relaxed) as u16;
    let ime_vk = match side {
        Side::Right => ime::VK_IME_ON,
        Side::Left => ime::VK_IME_OFF,
    };

    let suppression = inject::suppress(dummy, trigger_vk);
    if SPLIT.load(Ordering::Relaxed) {
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

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) };
    }
    let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
    if k.dwExtraInfo == inject::INJECT_TAG {
        return unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) };
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
        let (sent, expected) = fire(side, vk);
        if let Some(tx) = TX.get() {
            let _ = tx.send(Msg::Fired {
                at: t as f64 / 1000.0,
                side,
                sent,
                expected,
            });
        }
        if sent != expected {
            inject::release_stuck_keys();
            log("!!! injection failed, modifiers released".into());
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
    unsafe { PostThreadMessageW(MAIN_TID.load(Ordering::Relaxed), WM_QUIT, 0, 0) };
    1
}

fn main() {
    let args = match ProbeArgs::parse(120) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("imeprobe: {e}\n{}", probe_args::usage("imeprobe"));
            std::process::exit(2);
        }
    };
    let auto_exit_secs = args.secs;
    DUMMY_VK.store(args.dummy_vk as u32, Ordering::Relaxed);
    SPLIT.store(args.split, Ordering::Relaxed);
    MACHINE.with_borrow_mut(|m| m.set_config(args.config()));

    START.set(Instant::now()).ok();
    MAIN_TID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);

    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inject::release_stuck_keys();
        prev(info);
    }));

    println!("imeprobe - verifying IME switching");
    println!("{}", args.describe());
    println!("injection: {}", if args.split { "split" } else { "batch" });
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

    let (tx, rx) = channel::<Msg>();
    TX.set(tx).ok();
    let writer = std::thread::spawn(move || {
        let mut out = BufWriter::new(std::io::stdout());
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Line(s) => {
                    let _ = writeln!(out, "{s}");
                }
                Msg::Fired {
                    at,
                    side,
                    sent,
                    expected,
                } => {
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
                    let _ = writeln!(
                        out,
                        "{at:>8.3}  FIRE {side:?}  {what}  SendInput={sent}/{expected}  -> read back: IME={status}",
                    );
                }
                Msg::Stop => break,
            }
            let _ = out.flush();
        }
    });

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
            std::thread::sleep(Duration::from_secs(auto_exit_secs));
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
