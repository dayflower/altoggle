//! Keyboard state the low-level hook cannot observe on its own.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_RWIN,
};

/// Was another modifier already held when the trigger went down?
///
/// A key pressed before the trigger has already had its down event delivered, so
/// the state machine can never see it. `GetAsyncKeyState` is the only way to
/// notice, and it is cheap enough for the hook callback (no cross-process
/// message).
///
/// **The trigger itself has to be excluded**, or a Ctrl, Shift, or Win trigger
/// reports itself as held and never fires.
pub fn foreign_modifier_held(trigger_vk: u16) -> bool {
    const MODIFIERS: [u16; 8] = [
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
    ];
    MODIFIERS
        .iter()
        .any(|&vk| vk != trigger_vk && unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 })
}
