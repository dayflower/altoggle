//! Turning the IME on and off, and reading its state.
//!
//! Switching is done by injecting `VK_IME_ON` / `VK_IME_OFF` (idempotent, not a
//! toggle). Only the state read falls back to IMM32, for the tray icon and for
//! verification.
//!
//! **Never call `read_open_status` from a hook callback.** `SendMessageTimeout`
//! waits on the target's message loop, which would break the 300ms budget.
//!
//! There is deliberately no "is a Japanese IME active" check. Injecting
//! `VK_IME_ON` / `VK_IME_OFF` under en-US was measured to do nothing at all, so
//! "do nothing when no Japanese IME is active" holds without any check.
//! `GetKeyboardLayout` cannot be trusted across processes anyway (a Notepad
//! instance started earlier was reported as en-US), and it was too heavy to call
//! from the hook callback.

use std::sync::LazyLock;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW,
};

use crate::inject::{key_input, send};
use crate::wide;

/// `VK_IME_ON`
pub const VK_IME_ON: u16 = 0x16;
/// `VK_IME_OFF`
pub const VK_IME_OFF: u16 = 0x1A;

// IMM32 constants. windows-sys scatters these across features, so define them here.
const WM_IME_CONTROL: u32 = 0x0283;
const IMC_GETOPENSTATUS: usize = 0x0005;

/// The class of the window a Store app really takes input on.
static CORE_WINDOW_CLASS: LazyLock<Vec<u16>> = LazyLock::new(|| wide("Windows.UI.Core.CoreWindow"));

/// The window with the keyboard focus, as seen by the thread that owns `hwnd`.
///
/// Null when the thread has no focus window, which is ordinary rather than an
/// error: the caller falls back to the window it started from.
fn focus_of(hwnd: HWND) -> HWND {
    let thread = unsafe { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
    let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<GUITHREADINFO>() as u32;
    if unsafe { GetGUIThreadInfo(thread, &mut info) } == 0 {
        return std::ptr::null_mut();
    }
    info.hwndFocus
}

/// The window whose IME state is the one the user is actually looking at.
///
/// **The foreground window is the wrong thing to ask**, and it does not decline
/// — it answers "closed" with total confidence, which is worse than silence
/// because there is no way to tell it apart from a genuine "off". Two different
/// modern app shapes were measured doing this, and each needs one of the steps
/// below:
///
/// - a **Store app**: `GetForegroundWindow` returns an `ApplicationFrameWindow`
///   owned by ApplicationFrameHost.exe, a shell process that hosts the frame and
///   nothing else. The app's own input lives in a `Windows.UI.Core.CoreWindow`
///   child of it, in the app's own process
/// - **Windows 11's Notepad** (WinUI 3): one process, but the top-level window
///   and the `RichEditD2DPT` text control that holds the focus do not answer
///   alike. Asking the focus window is what gets the real answer
///
/// So: cross into the app's process if there is a `CoreWindow`, then ask
/// whatever holds the focus. Ordinary windows are their own focus window's
/// answer, so this path costs them nothing but is not a special case either.
fn input_window() -> Option<HWND> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }

    // Direct children only, which is where the frame keeps it.
    let core = unsafe {
        FindWindowExW(
            foreground,
            std::ptr::null_mut(),
            CORE_WINDOW_CLASS.as_ptr(),
            std::ptr::null(),
        )
    };
    let host = if core.is_null() { foreground } else { core };

    let focus = focus_of(host);
    Some(if focus.is_null() { host } else { focus })
}

/// Read whether the IME is open. `None` if it cannot be read.
///
/// Some Electron applications will not answer. `SendMessageTimeout` keeps a hung
/// target from taking us down with it.
pub fn read_open_status() -> Option<bool> {
    unsafe {
        let hwnd = input_window()?;
        let ime_wnd = ImmGetDefaultIMEWnd(hwnd);
        if ime_wnd.is_null() {
            return None;
        }
        let mut result: usize = 0;
        let ok = SendMessageTimeoutW(
            ime_wnd,
            WM_IME_CONTROL,
            IMC_GETOPENSTATUS,
            0,
            SMTO_ABORTIFHUNG,
            200,
            &mut result,
        );
        if ok == 0 { None } else { Some(result != 0) }
    }
}

/// Turn the IME on or off. Idempotent.
pub fn set_open(on: bool) -> u32 {
    let vk = if on { VK_IME_ON } else { VK_IME_OFF };
    send(&[key_input(vk, false), key_input(vk, true)])
}
