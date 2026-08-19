//! keylog — observes the raw events arriving at WH_KEYBOARD_LL / WH_MOUSE_LL.
//!
//! Its only job is to check whether the design's assumptions (how left/right Alt
//! look, auto-repeat, the phantom Ctrl of AltGr, the INJECTED flag) hold on real
//! hardware. No state machine here.
//!
//! Built to fail safe:
//! - Never blocks input. The callback always falls through to CallNextHookEx
//! - The callback only pushes to an mpsc channel; formatting and writing to
//!   stdout happen on another thread (so console QuickEdit selection stalling
//!   the write cannot stall the hook)
//! - Exits on its own after 120s by default. The first argument overrides the
//!   number of seconds (0 disables it)

use std::io::{BufWriter, Write};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::time::Instant;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN,
    LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, LLKHF_UP, MSG, MSLLHOOKSTRUCT,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

/// The only state the hook callback is allowed to touch.
static TX: OnceLock<Sender<Rec>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

enum Rec {
    Key {
        at: f64,
        msg: u32,
        vk: u32,
        sc: u32,
        flags: u32,
        extra: usize,
    },
    Mouse {
        at: f64,
        msg: u32,
        data: u32,
        extra: usize,
    },
    /// Shutdown signal. The sender lives in a static forever, so dropping it
    /// cannot close the channel.
    Stop,
}

fn now() -> f64 {
    START
        .get()
        .map(|s| s.elapsed().as_secs_f64())
        .unwrap_or(0.0)
}

fn emit(rec: Rec) {
    if let Some(tx) = TX.get() {
        // Swallow send failures (receiver gone). Panicking here would take the
        // whole desktop's input down with it.
        let _ = tx.send(rec);
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        emit(Rec::Key {
            at: now(),
            msg: wparam as u32,
            vk: k.vkCode,
            sc: k.scanCode,
            flags: k.flags,
            extra: k.dwExtraInfo,
        });
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let msg = wparam as u32;
        // Drop WM_MOUSEMOVE. Only buttons and the wheel matter for solo-press
        // detection.
        if is_interesting_mouse(msg) {
            let m = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
            emit(Rec::Mouse {
                at: now(),
                msg,
                data: m.mouseData,
                extra: m.dwExtraInfo,
            });
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

fn is_interesting_mouse(msg: u32) -> bool {
    matches!(
        msg,
        WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_XBUTTONDOWN
            | WM_XBUTTONUP
            | WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL
    )
}

fn main() {
    let auto_exit_secs: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(120);

    START.set(Instant::now()).ok();
    let (tx, rx) = channel::<Rec>();
    TX.set(tx).ok();

    println!("keylog - WH_KEYBOARD_LL / WH_MOUSE_LL observer");
    println!("Input is never blocked. Quit with Ctrl+C (focus this window first).");
    if auto_exit_secs > 0 {
        println!("Exiting automatically after {auto_exit_secs}s (argument overrides, 0 disables).");
    } else {
        println!("Automatic exit is disabled.");
    }
    println!("{:-<100}", "");
    println!(
        "{:>9}  {:<4} {:<5}  {:<26} {:<8} {:<22} extraInfo",
        "time", "kind", "updn", "vk", "scan", "flags"
    );

    let writer = std::thread::spawn(move || {
        // Holding stdout.lock() would deadlock the main thread's println!.
        let mut out = BufWriter::new(std::io::stdout());
        let mut last_down_vk: Option<u32> = None;
        while let Ok(rec) = rx.recv() {
            match rec {
                Rec::Stop => break,
                Rec::Key {
                    at,
                    msg,
                    vk,
                    sc,
                    flags,
                    extra,
                } => {
                    let up = flags & LLKHF_UP != 0;
                    // Two downs in a row for the same vk is auto-repeat; an
                    // intervening up means a separate press.
                    let repeat = !up && last_down_vk == Some(vk);
                    last_down_vk = if up { None } else { Some(vk) };

                    let _ = writeln!(
                        out,
                        "{at:>9.3}  {:<4} {:<5}  0x{vk:02X} {:<21} 0x{sc:02X}     {:<22} 0x{extra:X}{}",
                        msg_kind(msg),
                        if up { "UP" } else { "DOWN" },
                        vk_name(vk),
                        flag_names(flags),
                        if repeat { "  <repeat>" } else { "" },
                    );
                }
                Rec::Mouse {
                    at,
                    msg,
                    data,
                    extra,
                } => {
                    let _ = writeln!(
                        out,
                        "{at:>9.3}  {:<4} {:<5}  {:<26} {:<8} {:<22} 0x{extra:X}",
                        "MOUS",
                        "",
                        mouse_name(msg),
                        format!("hi=0x{:04X}", (data >> 16) & 0xFFFF),
                        "",
                    );
                }
            }
            let _ = out.flush();
        }
    });

    let hmod = unsafe { GetModuleHandleW(null_mut()) };
    let kb_hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0) };
    let ms_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0) };
    if kb_hook.is_null() || ms_hook.is_null() {
        eprintln!("SetWindowsHookExW failed");
        return;
    }

    if auto_exit_secs > 0 {
        // Escape hatch. This runs on the development machine itself, so it must
        // always die on its own even if left alone.
        let main_tid = unsafe { GetCurrentThreadId() };
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(auto_exit_secs));
            unsafe { PostThreadMessageW(main_tid, WM_QUIT, 0, 0) };
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
    if let Some(tx) = TX.get() {
        let _ = tx.send(Rec::Stop);
    }
    let _ = writer.join();
    println!("Stopped.");
}

fn msg_kind(msg: u32) -> &'static str {
    match msg {
        WM_KEYDOWN | WM_KEYUP => "KEY",
        WM_SYSKEYDOWN | WM_SYSKEYUP => "SYS",
        _ => "?",
    }
}

fn flag_names(flags: u32) -> String {
    let mut s = String::new();
    if flags & LLKHF_EXTENDED != 0 {
        s.push_str("EXT ");
    }
    if flags & LLKHF_INJECTED != 0 {
        s.push_str("INJ ");
    }
    if flags & LLKHF_LOWER_IL_INJECTED != 0 {
        s.push_str("LOWIL ");
    }
    if flags & LLKHF_ALTDOWN != 0 {
        s.push_str("ALTDN ");
    }
    if s.is_empty() {
        s.push('-');
    }
    s.trim_end().to_string()
}

fn mouse_name(msg: u32) -> &'static str {
    match msg {
        WM_LBUTTONDOWN => "LBUTTONDOWN",
        WM_LBUTTONUP => "LBUTTONUP",
        WM_RBUTTONDOWN => "RBUTTONDOWN",
        WM_RBUTTONUP => "RBUTTONUP",
        WM_MBUTTONDOWN => "MBUTTONDOWN",
        WM_MBUTTONUP => "MBUTTONUP",
        WM_XBUTTONDOWN => "XBUTTONDOWN",
        WM_XBUTTONUP => "XBUTTONUP",
        WM_MOUSEWHEEL => "MOUSEWHEEL",
        WM_MOUSEHWHEEL => "MOUSEHWHEEL",
        _ => "MOUSE?",
    }
}

/// Names only the VKs that matter to solo-press detection. Hex is enough for the rest.
fn vk_name(vk: u32) -> &'static str {
    match vk {
        0x08 => "VK_BACK",
        0x09 => "VK_TAB",
        0x0D => "VK_RETURN",
        0x10 => "VK_SHIFT",
        0x11 => "VK_CONTROL",
        0x12 => "VK_MENU",
        0x13 => "VK_PAUSE",
        0x14 => "VK_CAPITAL",
        0x15 => "VK_KANA/HANGUL",
        0x16 => "VK_IME_ON",
        0x17 => "VK_JUNJA",
        0x19 => "VK_KANJI",
        0x1A => "VK_IME_OFF",
        0x1B => "VK_ESCAPE",
        0x1C => "VK_CONVERT",
        0x1D => "VK_NONCONVERT",
        0x20 => "VK_SPACE",
        0x5B => "VK_LWIN",
        0x5C => "VK_RWIN",
        0x5D => "VK_APPS",
        0x7C..=0x87 => "VK_F13..F24",
        0xA0 => "VK_LSHIFT",
        0xA1 => "VK_RSHIFT",
        0xA2 => "VK_LCONTROL",
        0xA3 => "VK_RCONTROL",
        0xA4 => "VK_LMENU",
        0xA5 => "VK_RMENU",
        0xF0 => "VK_OEM_ATTN",
        0xF3 => "VK_OEM_AUTO",
        0xF4 => "VK_OEM_ENLW",
        _ => "",
    }
}
