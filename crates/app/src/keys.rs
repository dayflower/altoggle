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
    /// The keys whose being held ruins a solo press. An observing concept.
    ///
    /// The same eight keys as `inject::RELEASED_ON_FAILURE`, in the same order so
    /// that the two read against each other — but **for a different reason, and
    /// deliberately not shared**. That list holds the keys an injection can
    /// strand down; this one holds the keys that can be found already down.
    ///
    /// They come apart the moment either reason applies alone. A `DummyThenUp`
    /// trigger that is no modifier belongs in `RELEASED_ON_FAILURE` and must
    /// stay out of here; a `Swallow` modifier belongs here and must stay out of
    /// there. `VK_APPS` is the near case of the first, and its absence from both
    /// lists is not a coincidence.
    const MODIFIERS: [u16; 8] = [
        VK_LMENU,
        VK_RMENU,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LWIN,
        VK_RWIN,
    ];
    MODIFIERS
        .iter()
        .any(|&vk| vk != trigger_vk && unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 })
}
