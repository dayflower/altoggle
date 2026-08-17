//! State machine that detects a "solo press" of a modifier key.
//!
//! OS independent. It holds no Win32 types and no clock; events and timestamps
//! are supplied by the caller. This is the layer where the bugs that bite on real
//! hardware (auto-repeat, both Alts at once, Alt+drag, a modifier already held)
//! get pinned down by unit tests over synthetic events.
//!
//! Responsibilities of the caller (the Win32 adapter):
//! - Events we injected ourselves **must not reach here** (filter them out by the
//!   `dwExtraInfo` tag)
//! - If another key was already down when the trigger was pressed
//!   (`GetAsyncKeyState`), emit `ForeignKeyHeld` right after that `KeyDown`
//! - Emit `Reset` on foreground changes and session changes

#![cfg_attr(not(test), no_std)]

/// Which side the trigger key is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// An input event from the observation layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Key pressed. Auto-repeat also arrives as `KeyDown` for the same vk.
    KeyDown(u16),
    /// Key released.
    KeyUp(u16),
    /// A mouse button went down (which button does not matter).
    MouseButton,
    /// Another key was already held when the trigger was pressed.
    ///
    /// That key's down event will never reach the low-level hook again, so
    /// `KeyDown` cannot express it. The adapter must emit this **right after**
    /// the trigger's `KeyDown` (emitting it earlier is harmless but useless:
    /// the machine is still `Idle` and drops it).
    ForeignKeyHeld,
    /// Discard the current state. Foreground change, session change, hook
    /// reinstallation, and so on.
    Reset,
}

/// What the state machine wants the caller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do. The event may be passed on to the OS unchanged.
    None,
    /// A solo press completed. The caller suppresses the side effect and
    /// performs the IME switch.
    Fire(Side),
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub left_trigger: u16,
    pub right_trigger: u16,
    /// A press shorter than this counts as a solo press.
    ///
    /// Measured on real hardware: even deliberate solo presses reached 215ms,
    /// while auto-repeat starts at roughly 500ms. 400ms sits between the two.
    pub threshold_ms: u64,
}

/// `VK_LMENU`
pub const VK_LMENU: u16 = 0xA4;
/// `VK_RMENU`
pub const VK_RMENU: u16 = 0xA5;

impl Default for Config {
    fn default() -> Self {
        Self {
            left_trigger: VK_LMENU,
            right_trigger: VK_RMENU,
            threshold_ms: 400,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,
    Held {
        vk: u16,
        since_ms: u64,
        /// No longer a solo press. The matching up event will not fire.
        contaminated: bool,
    },
}

#[derive(Debug)]
pub struct Machine {
    cfg: Config,
    state: State,
}

impl Machine {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: State::Idle,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Replace the configuration, discarding any held state.
    ///
    /// This is the entry point for the settings dialog changing values while
    /// the app runs. `Machine` lives in the hook thread, so it cannot be touched
    /// from outside directly: notify the hook thread's message loop with
    /// `PostThreadMessage` and call this **from the loop**. Never call it from
    /// the hook callback.
    ///
    /// The held state is dropped so that a `Held` entry cannot be left dangling
    /// when the threshold or the trigger keys change underneath it. Changing
    /// settings mid-press simply means the next press is the one that counts.
    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
        self.state = State::Idle;
    }

    fn side_of(&self, vk: u16) -> Option<Side> {
        if vk == self.cfg.left_trigger {
            Some(Side::Left)
        } else if vk == self.cfg.right_trigger {
            Some(Side::Right)
        } else {
            None
        }
    }

    fn contaminate(&mut self) {
        if let State::Held {
            ref mut contaminated,
            ..
        } = self.state
        {
            *contaminated = true;
        }
    }

    pub fn on_event(&mut self, ev: Event, now_ms: u64) -> Action {
        match ev {
            Event::KeyDown(vk) => self.on_key_down(vk, now_ms),
            Event::KeyUp(vk) => self.on_key_up(vk, now_ms),
            Event::MouseButton => {
                // Alt+drag, Alt+click. Not a solo press.
                self.contaminate();
                Action::None
            }
            Event::ForeignKeyHeld => {
                self.contaminate();
                Action::None
            }
            Event::Reset => {
                self.state = State::Idle;
                Action::None
            }
        }
    }

    fn on_key_down(&mut self, vk: u16, now_ms: u64) -> Action {
        let is_trigger = self.side_of(vk).is_some();
        match self.state {
            State::Idle => {
                if is_trigger {
                    self.state = State::Held {
                        vk,
                        since_ms: now_ms,
                        contaminated: false,
                    };
                }
            }
            State::Held { vk: held, .. } => {
                if is_trigger && vk == held {
                    // Auto-repeat. Does not count as pressing another key;
                    // a genuine long press is rejected by the time threshold.
                } else {
                    // Another key, or the opposite trigger pressed at the same time.
                    self.contaminate();
                }
            }
        }
        Action::None
    }

    fn on_key_up(&mut self, vk: u16, now_ms: u64) -> Action {
        let side = self.side_of(vk);
        match self.state {
            State::Idle => Action::None,
            State::Held {
                vk: held,
                since_ms,
                contaminated,
            } => {
                if vk != held {
                    // Release of some other key. A key that was already down
                    // before Alt was pressed also surfaces here, so treat this
                    // as evidence that the press was not solo.
                    self.contaminate();
                    return Action::None;
                }
                self.state = State::Idle;
                let elapsed = now_ms.saturating_sub(since_ms);
                match side {
                    Some(s) if !contaminated && elapsed < self.cfg.threshold_ms => Action::Fire(s),
                    _ => Action::None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Machine {
        Machine::new(Config::default())
    }

    /// An arbitrary non-trigger key (`X`).
    const VK_X: u16 = 0x58;
    const VK_LCONTROL: u16 = 0xA2;

    #[test]
    fn solo_press_of_right_alt_fires() {
        let mut m = machine();
        assert_eq!(m.on_event(Event::KeyDown(VK_RMENU), 0), Action::None);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_RMENU), 87),
            Action::Fire(Side::Right)
        );
    }

    #[test]
    fn solo_press_of_left_alt_fires() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_LMENU), 215),
            Action::Fire(Side::Left)
        );
    }

    #[test]
    fn longest_measured_solo_press_215ms_still_fires() {
        // A 300ms threshold would make this a coin flip. Hence 400ms.
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 7188);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_LMENU), 7403),
            Action::Fire(Side::Left)
        );
    }

    #[test]
    fn alt_plus_another_key_does_not_fire() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        m.on_event(Event::KeyDown(VK_X), 50);
        m.on_event(Event::KeyUp(VK_X), 100);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 150), Action::None);
    }

    #[test]
    fn press_longer_than_the_threshold_does_not_fire() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_RMENU), 0);
        // Boundary: exactly 400ms does not fire.
        assert_eq!(m.on_event(Event::KeyUp(VK_RMENU), 400), Action::None);
    }

    #[test]
    fn auto_repeat_does_not_count_as_another_key() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_RMENU), 0);
        // 31ms apart, as measured. Still allowed to fire within the threshold.
        for t in [200, 231, 262, 293] {
            assert_eq!(m.on_event(Event::KeyDown(VK_RMENU), t), Action::None);
        }
        assert_eq!(
            m.on_event(Event::KeyUp(VK_RMENU), 320),
            Action::Fire(Side::Right)
        );
    }

    #[test]
    fn sustained_auto_repeat_is_rejected_by_the_time_threshold() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_RMENU), 19901);
        // Measured: repeat started 512ms in.
        for t in [20413, 20444, 20475, 20775] {
            m.on_event(Event::KeyDown(VK_RMENU), t);
        }
        assert_eq!(m.on_event(Event::KeyUp(VK_RMENU), 20789), Action::None);
    }

    #[test]
    fn both_alts_at_once_fires_neither() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_RMENU), 0);
        m.on_event(Event::KeyDown(VK_LMENU), 30);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 60), Action::None);
        assert_eq!(m.on_event(Event::KeyUp(VK_RMENU), 90), Action::None);
    }

    #[test]
    fn alt_drag_does_not_fire() {
        // The measured sequence at t=35.134:
        // Alt down -> LBUTTONDOWN -> auto-repeat -> LBUTTONUP -> Alt up
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        m.on_event(Event::MouseButton, 352);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 380), Action::None);
    }

    #[test]
    fn a_modifier_already_held_prevents_firing() {
        // The adapter emits ForeignKeyHeld right after the Alt down.
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        m.on_event(Event::ForeignKeyHeld, 0);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 50), Action::None);
    }

    #[test]
    fn foreign_key_held_is_harmless_while_idle() {
        // Getting the order wrong must not fail towards "never fires again".
        let mut m = machine();
        m.on_event(Event::ForeignKeyHeld, 0);
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_LMENU), 50),
            Action::Fire(Side::Left)
        );
    }

    #[test]
    fn releasing_a_prior_modifier_also_contaminates() {
        // Even if ForeignKeyHeld was missed, releasing Ctrl contaminates the press.
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        m.on_event(Event::KeyUp(VK_LCONTROL), 20);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 50), Action::None);
    }

    #[test]
    fn releasing_a_key_pressed_before_alt_contaminates() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        m.on_event(Event::KeyUp(VK_X), 20);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 50), Action::None);
    }

    #[test]
    fn reset_discards_the_held_state() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_RMENU), 0);
        m.on_event(Event::Reset, 10);
        // The up event after a foreground change is dangling, so it must not fire.
        assert_eq!(m.on_event(Event::KeyUp(VK_RMENU), 50), Action::None);
    }

    #[test]
    fn an_up_without_a_matching_held_state_is_ignored() {
        let mut m = machine();
        assert_eq!(m.on_event(Event::KeyUp(VK_RMENU), 0), Action::None);
    }

    #[test]
    fn releasing_and_pressing_again_after_contamination_fires() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_LMENU), 0);
        m.on_event(Event::KeyDown(VK_X), 10);
        m.on_event(Event::KeyUp(VK_X), 20);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 30), Action::None);
        m.on_event(Event::KeyDown(VK_LMENU), 100);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_LMENU), 150),
            Action::Fire(Side::Left)
        );
    }

    #[test]
    fn a_new_threshold_takes_effect_from_the_next_press() {
        let mut m = machine();
        // Long enough to fire under the 400ms default.
        m.on_event(Event::KeyDown(VK_RMENU), 0);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_RMENU), 300),
            Action::Fire(Side::Right)
        );

        m.set_config(Config {
            threshold_ms: 200,
            ..Config::default()
        });

        m.on_event(Event::KeyDown(VK_RMENU), 1000);
        assert_eq!(m.on_event(Event::KeyUp(VK_RMENU), 1300), Action::None);
        m.on_event(Event::KeyDown(VK_RMENU), 2000);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_RMENU), 2100),
            Action::Fire(Side::Right)
        );
    }

    #[test]
    fn trigger_keys_can_be_swapped() {
        // Solo-press detection is shared; only which keys to watch is configurable.
        let mut m = machine();
        m.set_config(Config {
            left_trigger: VK_LCONTROL,
            right_trigger: VK_RMENU,
            ..Config::default()
        });
        m.on_event(Event::KeyDown(VK_LCONTROL), 0);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_LCONTROL), 50),
            Action::Fire(Side::Left)
        );
        // Left Alt is no longer a trigger, so a solo press does nothing.
        m.on_event(Event::KeyDown(VK_LMENU), 100);
        assert_eq!(m.on_event(Event::KeyUp(VK_LMENU), 150), Action::None);
    }

    #[test]
    fn changing_the_config_discards_the_held_state() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_RMENU), 0);
        m.set_config(Config::default());
        // The dangling up event must not fire.
        assert_eq!(m.on_event(Event::KeyUp(VK_RMENU), 50), Action::None);
    }

    #[test]
    fn two_solo_presses_in_a_row_both_fire() {
        let mut m = machine();
        m.on_event(Event::KeyDown(VK_RMENU), 0);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_RMENU), 30),
            Action::Fire(Side::Right)
        );
        m.on_event(Event::KeyDown(VK_LMENU), 100);
        assert_eq!(
            m.on_event(Event::KeyUp(VK_LMENU), 212),
            Action::Fire(Side::Left)
        );
    }
}
