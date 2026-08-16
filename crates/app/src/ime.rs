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

use windows_sys::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
};

use crate::inject::{key_input, send};

/// `VK_IME_ON`
pub const VK_IME_ON: u16 = 0x16;
/// `VK_IME_OFF`
pub const VK_IME_OFF: u16 = 0x1A;

// IMM32 constants. windows-sys scatters these across features, so define them here.
const WM_IME_CONTROL: u32 = 0x0283;
const IMC_GETOPENSTATUS: usize = 0x0005;

/// Read whether the IME is open. `None` if it cannot be read.
///
/// TSF-only applications, UWP, and some Electron apps will not answer.
/// `SendMessageTimeout` keeps a hung target from taking us down with it.
pub fn read_open_status() -> Option<bool> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
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
