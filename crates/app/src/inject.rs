//! Key injection through `SendInput`.
//!
//! Injected events come back through our own low-level hook, so they carry
//! `INJECT_TAG` in `dwExtraInfo` to identify them. Real keyboards were measured
//! to use a `dwExtraInfo` of 0 consistently.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MAPVK_VK_TO_VSC_EX, MapVirtualKeyW, SendInput, VK_APPS, VK_LCONTROL, VK_LSHIFT, VK_LWIN,
    VK_RCONTROL, VK_RSHIFT, VK_RWIN,
};

use altoggle_core::{VK_LMENU, VK_RMENU};

/// Marker for events we injected ourselves.
pub const INJECT_TAG: usize = 0xA170_66E1;

/// The scan code of `vk`, and whether it is an extended key.
///
/// `MAPVK_VK_TO_VSC_EX` returns the 0xE0 prefix of an extended key in the high
/// byte, which is the only self-maintaining way to get this right: right Alt,
/// right Ctrl, and **both** Win keys are extended, left Alt and both Shifts are
/// not. Injecting a Win up without `KEYEVENTF_EXTENDEDKEY` does not reliably
/// clear the shell's idea that Win is still held.
///
/// `wScan` takes the low byte only; the prefix travels as the flag instead.
///
/// Pause reports 0xE11D, whose prefix is 0xE1 rather than 0xE0, so it would come
/// back as "not extended". We never inject it, and it is not a candidate trigger.
fn scan_code(vk: u16) -> (u16, bool) {
    let sc = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX) };
    (sc as u16 & 0xFF, sc >> 8 == 0xE0)
}

/// Is `vk` an extended key? Exposed for the probes, which report it.
pub fn is_extended(vk: u16) -> bool {
    scan_code(vk).1
}

/// The scan code `vk` would be injected with. Zero means it has none.
///
/// Reported by the probes so that a measurement record says what was actually
/// injected rather than what was asked for.
pub fn scan_of(vk: u16) -> u16 {
    scan_code(vk).0
}

/// Build a single key event.
///
/// Some IMEs and applications ignore an event whose `wScan` is 0, so it is
/// filled in from the virtual key. The extended flag is derived the same way
/// rather than passed in: every caller would otherwise carry its own table of
/// which keys are extended, and one of them would be wrong.
pub fn key_input(vk: u16, up: bool) -> INPUT {
    let (scan, extended) = scan_code(vk);
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
                wScan: scan,
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

/// How a solo press is robbed of its usual side effect.
///
/// Two shapes, because Windows produces the two side effects by two different
/// mechanisms. This is not a per-key table dressed up as a design: the split is
/// between keys the OS tracks as "pressed alone" and keys it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suppression {
    /// `[dummy down, dummy up, TRIGGER up]`, in one `SendInput`.
    ///
    /// Alt and Win are tracked by Windows as "pressed alone", and the dummy is
    /// what breaks that. Their up must still be delivered, or the modifier stays
    /// logically held and every later keystroke turns into a chord.
    ///
    /// Measured to stop both the menu bar and the start menu. Win needed nothing
    /// beyond this — not a dummy with a real scan code, not holding the dummy
    /// across the up, not `KEYEVENTF_SCANCODE`. All three were tried; none
    /// helped or was needed.
    DummyThenUp,
    /// Nothing. Block the real up and inject no replacement.
    ///
    /// `VK_APPS` has no "pressed alone" state to break: `DefWindowProc` turns
    /// *any* `WM_KEYUP` for it into `WM_CONTEXTMENU`. Measured — `DummyThenUp`
    /// does not suppress the context menu, because the up it injects is itself
    /// what opens the menu. Withholding the up is only safe because `VK_APPS` is
    /// not a modifier: left logically down it changes nothing, where a withheld
    /// Alt up would wreck every following keystroke.
    Swallow,
}

/// Which shape `trigger_vk` needs.
pub fn suppression_for(trigger_vk: u16) -> Suppression {
    if trigger_vk == VK_APPS {
        Suppression::Swallow
    } else {
        Suppression::DummyThenUp
    }
}

/// The events to inject in place of the trigger's blocked up. May be empty.
///
/// The caller must block the real up, and must send this — plus anything it
/// appends — in a **single** `SendInput`.
pub fn suppress(dummy_vk: u16, trigger_vk: u16) -> Vec<INPUT> {
    match suppression_for(trigger_vk) {
        Suppression::DummyThenUp => vec![
            key_input(dummy_vk, false),
            key_input(dummy_vk, true),
            key_input(trigger_vk, true),
        ],
        Suppression::Swallow => Vec::new(),
    }
}

/// Send a batch, reporting `(delivered, expected)`. An empty batch sends nothing.
pub fn send_batch(inputs: &[INPUT]) -> (u32, u32) {
    if inputs.is_empty() {
        return (0, 0);
    }
    (send(inputs), inputs.len() as u32)
}

/// Cleanup that must also run on abnormal exit paths. A key left stuck down is
/// the worst outcome here.
///
/// A Win key stuck down in particular turns every subsequent keystroke into a
/// hotkey.
///
/// See `RELEASED_ON_FAILURE` for what is in the list and what is pointedly not.
pub fn release_stuck_keys() -> u32 {
    let batch: Vec<INPUT> = RELEASED_ON_FAILURE
        .iter()
        .map(|&vk| key_input(vk, true))
        .collect();
    send(&batch)
}

/// The keys `release_stuck_keys` lets go of.
///
/// **Must cover every `Suppression::DummyThenUp` trigger**, held there by a test.
/// Those keys have their real up blocked and a replacement injected, so a partly
/// landed injection can strand them down.
///
/// `VK_APPS` is deliberately **not** here, even though it is a trigger. Its up is
/// withheld by design, and injecting one is exactly what opens the context menu —
/// `DefWindowProc` does not care whether a down came first. Adding it would pop a
/// context menu on every exit and every panic. Left down it is harmless, being no
/// modifier.
pub const RELEASED_ON_FAILURE: [u16; 8] = [
    VK_LMENU,
    VK_RMENU,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LWIN,
    VK_RWIN,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The reason `key_input` derives the flag instead of taking it: getting this
    /// wrong for a Win key leaves the shell believing Win is still held.
    #[test]
    fn the_extended_keys_are_the_ones_windows_says_they_are() {
        for vk in [VK_RMENU, VK_RCONTROL, VK_LWIN, VK_RWIN] {
            assert!(is_extended(vk), "0x{vk:02X} should be extended");
        }
        for vk in [VK_LMENU, VK_LCONTROL, VK_LSHIFT, VK_RSHIFT, 0x07] {
            assert!(!is_extended(vk), "0x{vk:02X} should not be extended");
        }
    }

    #[test]
    fn the_scan_code_carries_no_prefix() {
        // The 0xE0 travels as KEYEVENTF_EXTENDEDKEY, not in wScan.
        assert_eq!(scan_code(VK_RMENU), (0x38, true));
        assert_eq!(scan_code(VK_LMENU), (0x38, false));
        assert_eq!(scan_code(VK_LWIN), (0x5B, true));
    }

    /// The default dummy is injected with `wScan` zero, because an undefined
    /// virtual key has no scan code. Measured to suppress the start menu anyway,
    /// so this is a property of the dummy worth knowing, not a defect.
    #[test]
    fn an_undefined_virtual_key_has_no_scan_code() {
        assert_eq!(scan_of(0x07), 0);
        assert_eq!(scan_of(0x7C), 0x64); // VK_F13
        assert_eq!(scan_of(VK_LCONTROL), 0x1D);
    }
}
