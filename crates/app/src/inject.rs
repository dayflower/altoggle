//! Key injection through `SendInput`.
//!
//! Injected events come back through our own low-level hook, so they carry
//! `INJECT_TAG` in `dwExtraInfo` to identify them. Real keyboards were measured
//! to use a `dwExtraInfo` of 0 consistently.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VK_LCONTROL, VK_LSHIFT, VK_LWIN, VK_RCONTROL,
    VK_RSHIFT, VK_RWIN,
};

use altoggle_core::{VK_LMENU, VK_RMENU};

/// Marker for events we injected ourselves.
pub const INJECT_TAG: usize = 0xA170_66E1;

/// Build a single key event.
///
/// Some IMEs and applications ignore an event whose `wScan` is 0, so it is
/// filled in with `MapVirtualKeyW`.
pub fn key_input(vk: u16, up: bool, extended: bool) -> INPUT {
    let mut flags = 0u32;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECT_TAG,
            },
        },
    }
}

/// Inject a batch of events. Returns how many were actually sent.
///
/// **Calling this once per event lets other input interleave.** Wherever the
/// order carries meaning, pass the whole sequence as one array in one call.
pub fn send(inputs: &[INPUT]) -> u32 {
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    }
}

/// Right Alt is an extended key; injection has to match that flag.
pub fn is_extended_trigger(vk: u16) -> bool {
    vk == VK_RMENU
}

/// The heart of plan option A: inject `[dummy down, dummy up, ALT up]` in one call.
///
/// The press no longer looks like Alt pressed and released alone, so Windows does
/// not activate the menu bar. The caller must block the real Alt up.
pub fn dummy_then_trigger_up(dummy_vk: u16, trigger_vk: u16) -> u32 {
    send(&[
        key_input(dummy_vk, false, false),
        key_input(dummy_vk, true, false),
        key_input(trigger_vk, true, is_extended_trigger(trigger_vk)),
    ])
}

/// Cleanup that must also run on abnormal exit paths. A key left stuck down is
/// the worst outcome here.
///
/// A Win key stuck down in particular turns every subsequent keystroke into a
/// hotkey.
pub fn release_all_modifiers() -> u32 {
    let keys = [
        (VK_LMENU, false),
        (VK_RMENU, true),
        (VK_LCONTROL, false),
        (VK_RCONTROL, true),
        (VK_LSHIFT, false),
        (VK_RSHIFT, false),
        (VK_LWIN, true),
        (VK_RWIN, true),
    ];
    let batch: Vec<INPUT> = keys
        .iter()
        .map(|&(vk, ext)| key_input(vk, true, ext))
        .collect();
    send(&batch)
}
