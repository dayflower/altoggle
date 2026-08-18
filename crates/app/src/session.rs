//! Watches session transitions and owns the process's main message loop.
//!
//! Low-level hooks can be lost when the desktop switches: locking and unlocking,
//! connecting over RDP, fast user switching. Windows never reports a lost hook,
//! so the practical defence is to reinstall on the transitions we can observe.
//!
//! Receiving `WM_WTSSESSION_CHANGE` needs a window, so this creates a
//! message-only one (parented to `HWND_MESSAGE`): it never appears on screen,
//! never appears in the taskbar, and receives no broadcast messages.
//!
//! That last property is why the window also carries a timer. `GetMessageW`
//! blocks, so without one `after_message` would only run when something else
//! happened to arrive, and the tray icon needs a heartbeat to read the IME on.
//! A broadcast would have served for the light/dark theme — Windows sends
//! `WM_SETTINGCHANGE` with `"ImmersiveColorSet"` — but it does not reach an
//! `HWND_MESSAGE` window, so that is polled on the same tick.

use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    KillTimer, MSG, PostThreadMessageW, RegisterClassW, SetTimer, TranslateMessage, WNDCLASSW,
    WM_DESTROY, WM_QUIT, WM_TIMER,
};

use crate::{dialog, hook, log, wide};

/// `WM_WTSSESSION_CHANGE`. Not exported by windows-sys, so declared here.
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;

// Session state codes delivered in wParam.
const WTS_CONSOLE_CONNECT: u32 = 0x1;
const WTS_REMOTE_CONNECT: u32 = 0x3;
const WTS_SESSION_LOGON: u32 = 0x5;
const WTS_SESSION_UNLOCK: u32 = 0x8;

/// Timer id for the heartbeat. Only one timer exists on this window, so the
/// value is arbitrary; it just has to be non-zero.
const TICK_TIMER: usize = 1;

/// How often to wake the loop, in milliseconds.
///
/// This is the worst-case lag between the IME changing and the tray icon saying
/// so. Fast enough that pressing a trigger key feels answered, slow enough that
/// the `SendMessageTimeout` behind it is not worth thinking about.
const TICK_MS: u32 = 400;

static MAIN_TID: AtomicU32 = AtomicU32::new(0);

/// Ask the main loop to quit. Safe to call from any thread.
pub fn request_quit() {
    let tid = MAIN_TID.load(Ordering::SeqCst);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
    }
}

fn state_name(state: u32) -> &'static str {
    match state {
        WTS_CONSOLE_CONNECT => "console connect",
        WTS_REMOTE_CONNECT => "remote connect",
        WTS_SESSION_LOGON => "session logon",
        WTS_SESSION_UNLOCK => "session unlock",
        _ => "other",
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_WTSSESSION_CHANGE => {
            let state = wparam as u32;
            // Only the transitions that bring a desktop back matter. Reinstalling
            // on lock or disconnect would just target a desktop nobody is at.
            if matches!(
                state,
                WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT | WTS_SESSION_LOGON | WTS_SESSION_UNLOCK
            ) {
                log::line(format!(
                    "session change: {} -> reinstalling hooks",
                    state_name(state)
                ));
                hook::request_reinstall();
            }
            0
        }
        // Deliberately empty. The tick exists to wake `GetMessageW` so that the
        // loop's `after_message` runs; the work itself belongs to the caller,
        // which is the only thing that knows about the tray.
        WM_TIMER => 0,
        WM_DESTROY => 0,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Create the message-only window, subscribe to session notifications, and pump
/// messages until `WM_QUIT`.
///
/// `after_message` runs after each dispatch. Tray menu clicks arrive as window
/// messages and are turned into channel events by muda, so draining that channel
/// here is what makes the menu responsive without a polling timer.
pub fn run(mut after_message: impl FnMut()) -> Result<(), String> {
    MAIN_TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

    let class_name = wide("altoggle-session-watcher");
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };

    let mut class: WNDCLASSW = unsafe { std::mem::zeroed() };
    class.lpfnWndProc = Some(wnd_proc);
    class.hInstance = hinstance;
    class.lpszClassName = class_name.as_ptr();
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err("RegisterClassW failed".into());
    }

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err("CreateWindowExW failed".into());
    }

    // Losing session notifications is not fatal: the app still works, it just
    // will not notice a lost hook after unlocking.
    if unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) } == 0 {
        log::line("WTSRegisterSessionNotification failed: session changes will not be tracked");
    }

    // Also not fatal, for the same reason: a tray icon that stops following the
    // IME is a nuisance, and taking the app down over it would cost the user
    // the ability to type.
    if unsafe { SetTimer(hwnd, TICK_TIMER, TICK_MS, None) } == 0 {
        log::line("SetTimer failed: the tray icon will not follow the IME");
    }

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        // The settings window is modeless and shares this loop, so Tab, Esc,
        // Enter and mnemonics only work if it is offered the message before
        // TranslateMessage gets it.
        if !dialog::pre_translate(&msg) {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // Runs on every iteration, swallowed messages included: a tray click,
        // a session change and `--exit-after` all have to keep working while
        // the settings window is open.
        after_message();
    }

    // WM_QUIT arrives without destroying anything, so the settings window can
    // still be alive here, holding a font and its state.
    dialog::close();
    unsafe {
        KillTimer(hwnd, TICK_TIMER);
        WTSUnRegisterSessionNotification(hwnd);
        DestroyWindow(hwnd);
    }
    Ok(())
}
