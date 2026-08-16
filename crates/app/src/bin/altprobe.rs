//! altprobe — checks whether plan option A ("inject a dummy key") actually works.
//!
//! Does not touch the IME yet. It only answers one question: **does a solo Alt
//! press stop opening the menu bar?** If that fails, the whole design falls back
//! to option B (intercepting Alt down entirely), which makes this the first
//! thing to verify.
//!
//! What it does:
//! - Passes Alt down through to the OS untouched (Alt+X behaves exactly as before)
//! - On detecting a solo Alt up, **blocks** that up and injects
//!   `[dummy down, dummy up, ALT up]` in **one SendInput call**
//!
//! Because this is a dangerous thing to run, there are three escape hatches:
//! - Exits automatically after 90s by default (first argument overrides)
//! - Ctrl+C goes through the normal shutdown path
//! - Normal exit and panic both inject an up for every modifier before finishing
//!
//! Usage:
//!   altprobe [seconds] [dummy VK in hex]
//!   e.g. altprobe 120 7C   -> 120 seconds, using VK_F13 as the dummy

use std::cell::RefCell;
use std::io::{BufWriter, Write};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use altoggle_app::inject;
use altoggle_core::{Action, Config, Event, Machine, Side, VK_LMENU, VK_RMENU};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_UP, MSG,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN,
    WM_XBUTTONDOWN,
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

/// Was another modifier already held when the trigger was pressed? Alt itself
/// does not count.
fn foreign_modifier_held() -> bool {
    let down = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 };
    down(VK_CONTROL) || down(VK_SHIFT) || down(VK_LWIN) || down(VK_RWIN)
}

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
            if (vk == VK_LMENU || vk == VK_RMENU) && foreign_modifier_held() {
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
        let alt_vk = match side {
            Side::Left => VK_LMENU,
            Side::Right => VK_RMENU,
        };
        let sent = inject::dummy_then_trigger_up(DUMMY_VK.load(Ordering::Relaxed) as u16, alt_vk);
        log(format!(
            "{:>8.3}  *** FIRE {:?}  blocked the real up -> injected [0x{:02X} down, 0x{:02X} up, 0x{:02X} up] (SendInput={}/3)",
            t as f64 / 1000.0,
            side,
            DUMMY_VK.load(Ordering::Relaxed),
            DUMMY_VK.load(Ordering::Relaxed),
            alt_vk,
            sent,
        ));
        if sent != 3 {
            // A failed injection leaves Alt held down. Release it defensively.
            log(format!(
                "{:>8.3}  !!! injection failed, releasing modifiers",
                t as f64 / 1000.0
            ));
            inject::release_all_modifiers();
        }
        return 1; // block the real Alt up
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
    let mut args = std::env::args().skip(1);
    let auto_exit_secs: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(90);
    if let Some(d) = args.next()
        && let Ok(v) = u32::from_str_radix(d.trim_start_matches("0x"), 16)
    {
        DUMMY_VK.store(v, Ordering::Relaxed);
    }
    let dummy = DUMMY_VK.load(Ordering::Relaxed);

    START.set(Instant::now()).ok();
    MAIN_TID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);

    // Do not let a panic leave keys stuck down.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inject::release_all_modifiers();
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
    println!(
        "dummy key: 0x{dummy:02X}   threshold: {}ms",
        Config::default().threshold_ms
    );
    println!("Press Alt alone and watch whether the menu bar opens in each app.");
    println!("The IME is not switched yet. Alt+X and friends should be unchanged.");
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
    inject::release_all_modifiers();
    if let Some(tx) = TX.get() {
        let _ = tx.send(Msg::Stop);
    }
    let _ = writer.join();
    println!("Stopped (modifiers released).");
}
