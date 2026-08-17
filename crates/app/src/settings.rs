//! Settings, stored in a TOML file.
//!
//! Trigger keys are named rather than numeric, because a virtual-key code is a
//! terrible thing to hand-edit and because `dialog` wants exactly this list to
//! populate its dropdowns.
//!
//! The settings dialog is the front end; the file is the durable record and is
//! still worth reading, because its comments carry the measurements the values
//! rest on. Either way the values reach the running state machine the same way:
//! `HookThread::set_config` posts them to the hook thread's message loop.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use altoggle_core::{Config, VK_NONE};

/// A key that can act as a trigger.
///
/// Deliberately not "any key": a solo press has to be robbed of its usual side
/// effect, and that only works for keys whose side effect we have measured. Alt
/// opens the menu bar and Win opens the start menu; both are suppressed by
/// injecting a dummy key before the up, which is what makes them eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKey {
    LeftAlt,
    RightAlt,
    LeftCtrl,
    RightCtrl,
    LeftShift,
    RightShift,
    LeftWin,
    RightWin,
    /// The context-menu key, `VK_APPS`.
    ///
    /// **Not Alt.** Win32 calls Alt `VK_MENU`, so "Menu" is ambiguous in code;
    /// this is the key with the menu glyph on it, next to right Ctrl. It is also
    /// the only trigger that is not a modifier, and the only one with no left or
    /// right twin.
    Menu,
}

impl TriggerKey {
    /// Every trigger, in the order the config file and the settings dialog
    /// present them.
    pub const ALL: [TriggerKey; 9] = [
        TriggerKey::LeftAlt,
        TriggerKey::RightAlt,
        TriggerKey::LeftCtrl,
        TriggerKey::RightCtrl,
        TriggerKey::LeftShift,
        TriggerKey::RightShift,
        TriggerKey::LeftWin,
        TriggerKey::RightWin,
        TriggerKey::Menu,
    ];

    pub fn vk(self) -> u16 {
        match self {
            TriggerKey::LeftAlt => 0xA4,
            TriggerKey::RightAlt => 0xA5,
            TriggerKey::LeftCtrl => 0xA2,
            TriggerKey::RightCtrl => 0xA3,
            TriggerKey::LeftShift => 0xA0,
            TriggerKey::RightShift => 0xA1,
            TriggerKey::LeftWin => 0x5B,
            TriggerKey::RightWin => 0x5C,
            TriggerKey::Menu => 0x5D, // VK_APPS
        }
    }

    /// The name used in the config file, the dialog and the probes.
    pub fn name(self) -> &'static str {
        match self {
            TriggerKey::LeftAlt => "LeftAlt",
            TriggerKey::RightAlt => "RightAlt",
            TriggerKey::LeftCtrl => "LeftCtrl",
            TriggerKey::RightCtrl => "RightCtrl",
            TriggerKey::LeftShift => "LeftShift",
            TriggerKey::RightShift => "RightShift",
            TriggerKey::LeftWin => "LeftWin",
            TriggerKey::RightWin => "RightWin",
            TriggerKey::Menu => "Menu",
        }
    }

    /// Parse a name, for the probes' command lines. Case-insensitive, because a
    /// command line is typed in a hurry; the config file stays strict.
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(s))
    }
}

/// What an unset trigger is called in the config file.
///
/// TOML has no null, and leaving a key out of the file already means "use the
/// default" — a test pins that — so switching a trigger off needs a word of its
/// own rather than an absence.
pub const NONE_NAME: &str = "None";

/// What a trigger slot is called, whether or not it holds a key.
pub fn slot_name(slot: Option<TriggerKey>) -> &'static str {
    match slot {
        Some(key) => key.name(),
        None => NONE_NAME,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    /// Solo press of this key turns the IME off. `None` switches it off.
    #[serde(with = "trigger_slot")]
    pub left_trigger: Option<TriggerKey>,
    /// Solo press of this key turns the IME on. `None` switches it off.
    ///
    /// Wanting one direction and not the other is ordinary — a keyboard with no
    /// right Win key, or a user who only ever needs the IME turned on — so both
    /// slots may be empty, including both at once.
    #[serde(with = "trigger_slot")]
    pub right_trigger: Option<TriggerKey>,
    /// A press shorter than this counts as a solo press.
    pub threshold_ms: u64,
    /// The key injected to stop a solo press from looking solo.
    pub dummy_vk: u16,
}

/// Reads and writes a trigger slot as its name, with `NONE_NAME` for empty.
///
/// Hand-rolled rather than derived on `TriggerKey`, so that `name` is the only
/// place a trigger's spelling is decided. A derive would put a second copy of
/// every name in the file format, free to drift from the one the dialog and the
/// probes show.
mod trigger_slot {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{NONE_NAME, TriggerKey, slot_name};

    pub fn serialize<S: Serializer>(
        slot: &Option<TriggerKey>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        slot_name(*slot).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<TriggerKey>, D::Error> {
        let name = String::deserialize(deserializer)?;
        if name == NONE_NAME {
            return Ok(None);
        }
        // Case-sensitive, unlike `TriggerKey::from_name`: a config file is
        // edited slowly and read strictly, where a command line is typed fast.
        TriggerKey::ALL
            .into_iter()
            .find(|key| key.name() == name)
            .map(Some)
            .ok_or_else(|| D::Error::custom(format!("{name} is not a trigger key")))
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            left_trigger: Some(TriggerKey::LeftAlt),
            right_trigger: Some(TriggerKey::RightAlt),
            threshold_ms: 400,
            dummy_vk: 0x07,
        }
    }
}

/// Everything the hook thread needs, in one message-sized value.
#[derive(Debug, Clone, Copy)]
pub struct Runtime {
    pub core: Config,
    pub dummy_vk: u16,
}

impl Settings {
    pub fn runtime(&self) -> Runtime {
        Runtime {
            core: Config {
                left_trigger: self.left_trigger.map_or(VK_NONE, TriggerKey::vk),
                right_trigger: self.right_trigger.map_or(VK_NONE, TriggerKey::vk),
                threshold_ms: self.threshold_ms,
            },
            dummy_vk: self.dummy_vk,
        }
    }

    /// Everything about these settings that cannot work.
    ///
    /// Only things that cannot work. An unusual value is not one of them: the
    /// measured bands are written in the config file's comments, and a front
    /// end that also nagged about them would be arguing with the user over the
    /// one number they came here to tune.
    pub fn problems(&self) -> Vec<Problem> {
        let mut found = Vec::new();
        // Two empty slots are not a clash: that is the inert state, which is
        // allowed. Only two real keys colliding costs the user a direction.
        if let (Some(left), Some(right)) = (self.left_trigger, self.right_trigger)
            && left == right
        {
            found.push(Problem::SameTrigger(left));
        }
        if self.threshold_ms == 0 {
            found.push(Problem::ThresholdZero);
        }
        if self.dummy_vk == 0 {
            found.push(Problem::DummyZero);
        } else if let Some(key) = TriggerKey::ALL
            .into_iter()
            .find(|k| k.vk() == self.dummy_vk)
        {
            found.push(Problem::DummyIsTrigger(key));
        }
        found
    }
}

/// Why a settings combination will not do what it looks like it does.
///
/// Shared rather than checked at each front end: the probes' command line used
/// to carry the only such check, which left the dialog and the command line
/// free to disagree about what is legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// `Machine::side_of` tests the left trigger first, so an equal pair makes
    /// "IME on" unreachable: every solo press would turn the IME off.
    SameTrigger(TriggerKey),
    /// The machine compares `elapsed < threshold_ms`, so zero never fires.
    ThresholdZero,
    /// Virtual key 0 is not a key.
    DummyZero,
    /// The dummy is injected as a real down and up. A key that has a
    /// solo-press behaviour of its own performs it, which is the exact thing
    /// the dummy exists to prevent.
    DummyIsTrigger(TriggerKey),
}

impl Problem {
    /// One sentence, at most `MESSAGE_BUDGET` characters.
    pub fn message(self) -> String {
        match self {
            Problem::SameTrigger(k) => {
                format!(
                    "{} is set for both, so nothing would turn the IME on.",
                    k.name()
                )
            }
            Problem::ThresholdZero => {
                "A threshold of 0 never fires: no press is shorter than it.".into()
            }
            Problem::DummyZero => "0x00 is not a virtual key.".into(),
            Problem::DummyIsTrigger(k) => format!(
                "{} acts on its own solo press, so it cannot be the dummy.",
                k.name()
            ),
        }
    }
}

/// How long a `Problem::message` may be.
///
/// The dialog shows it in a fixed two-line label, and about 44 characters fit
/// on a line at the default font. Word wrap does not fill a line evenly, so the
/// budget is well under 88; over it, the tail is clipped rather than wrapped
/// into view, and the tail is the half that says what to do about it.
pub const MESSAGE_BUDGET: usize = 72;

/// The `# One of: ...` comment listing every trigger, wrapped.
///
/// Generated from `TriggerKey::ALL` rather than written out, so a tenth trigger
/// cannot leave the file telling the user about nine.
fn trigger_name_comment() -> String {
    const FIRST: &str = "# One of: ";
    const CONTINUATION: &str = "#         ";
    const WIDTH: usize = 76;

    // `NONE_NAME` last, because it is the answer to a different question than
    // the keys are: not "which key", but "neither".
    let names: Vec<&str> = TriggerKey::ALL
        .iter()
        .map(|key| key.name())
        .chain(std::iter::once(NONE_NAME))
        .collect();

    let mut lines: Vec<String> = Vec::new();
    let mut line = String::from(FIRST);
    let mut empty = true;
    for (i, name) in names.iter().enumerate() {
        let comma = i + 1 < names.len();
        let width = name.len() + usize::from(comma);
        if !empty && line.len() + 1 + width > WIDTH {
            lines.push(line);
            line = String::from(CONTINUATION);
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(name);
        if comma {
            line.push(',');
        }
        empty = false;
    }
    lines.push(line);
    lines.join("\n")
}

/// The whole config file, comments and all, for these values.
///
/// The comments live here rather than in the file because a serializer cannot
/// emit them, and they are most of the value of a file you edit by hand. The
/// consequence is that saving regenerates the file: the explanations survive,
/// anything the user added to it does not.
fn render(s: &Settings) -> String {
    format!(
        r#"# altoggle configuration.
#
# "Settings..." on the tray menu edits this file, and rewrites it whole when
# you press OK or Apply: the comments below survive, anything you add does not.
# Editing it by hand works too, but only takes effect at the next start.

# Which key turns the IME off, and which turns it on.
{triggers}
#
# Whichever key you pick gives up its usual solo-press behaviour: Alt the menu
# bar, Win the start menu, Menu the context menu. Holding the key past
# threshold_ms below still gets you the original behaviour.
#
# "None" leaves that direction alone, for when you only want one of the two.
# Setting both to "None" is allowed and leaves the app doing nothing.
#
# "Menu" is the key with the menu glyph next to right Ctrl, not Alt.
#
# Note that many Japanese laptops and JIS keyboards have no right Win key, and
# some have no Menu key. Mixing sides is fine, for example LeftWin and RightAlt.
left_trigger = "{left}"
right_trigger = "{right}"

# A press shorter than this counts as a solo press.
#
# Measured: deliberate solo presses reach 215ms, auto-repeat starts around
# 500ms. Below ~250ms you will miss presses you meant; above ~500ms a held key
# starts firing.
#
# This threshold is also the escape hatch: hold the key past it and the press
# falls through to Windows untouched, so Alt still opens the menu bar.
threshold_ms = {threshold}

# The key injected to make a solo press stop looking solo, which is what stops
# Windows from opening the menu bar. A virtual-key code, in hex to match the
# dialog and the probes' --dummy flag. 0x07 is undefined and was harmless in
# every application tested; 0x7C-0x87 (VK_F13 to VK_F24) also work.
dummy_vk = 0x{dummy:02X}
"#,
        triggers = trigger_name_comment(),
        left = slot_name(s.left_trigger),
        right = slot_name(s.right_trigger),
        threshold = s.threshold_ms,
        dummy = s.dummy_vk,
    )
}

/// `%APPDATA%\altoggle\config.toml`
pub fn config_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("altoggle").join("config.toml"))
}

/// The outcome of loading, so the caller can log it rather than this module
/// guessing how to report.
pub enum Loaded {
    /// Parsed from an existing file.
    Existing(Settings),
    /// No file existed; a commented default was written.
    Created(Settings),
    /// Something went wrong. Defaults are in use.
    Failed(Settings, String),
}

impl Loaded {
    pub fn settings(&self) -> Settings {
        match self {
            Loaded::Existing(s) | Loaded::Created(s) | Loaded::Failed(s, _) => *s,
        }
    }
}

/// Read the config file, creating it with defaults when it does not exist.
///
/// A broken file never stops the app from running: it falls back to defaults and
/// reports why. Losing the ability to type is a worse outcome than ignoring a
/// bad setting.
pub fn load() -> Loaded {
    let defaults = Settings::default();
    let Some(path) = config_path() else {
        return Loaded::Failed(defaults, "APPDATA is not set".into());
    };

    if !path.exists() {
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            return Loaded::Failed(defaults, format!("could not create {}: {e}", dir.display()));
        }
        return match std::fs::write(&path, render(&defaults)) {
            Ok(()) => Loaded::Created(defaults),
            Err(e) => Loaded::Failed(defaults, format!("could not write {}: {e}", path.display())),
        };
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return Loaded::Failed(defaults, format!("could not read {}: {e}", path.display()));
        }
    };
    match toml::from_str::<Settings>(&text) {
        Ok(s) => Loaded::Existing(s),
        Err(e) => Loaded::Failed(defaults, format!("{} is invalid: {e}", path.display())),
    }
}

/// Rewrite the config file from `settings`.
///
/// Written beside the target and renamed, because this runs while the app is
/// resident: an interrupted write in place would leave a truncated file that
/// the next start rejects, and the user would find that out by losing their
/// settings rather than at the moment it happened.
pub fn save(settings: &Settings) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "APPDATA is not set".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, render(settings))
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("could not replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rendered_defaults_parse_back_into_the_defaults() {
        // The file handed to the user must not disagree with the built-in
        // defaults, or the first reload would silently change behaviour.
        let text = render(&Settings::default());
        let parsed: Settings = toml::from_str(&text).expect("rendered file must be valid TOML");
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn every_setting_round_trips_through_render() {
        // The failure mode a parameterised template invites is a placeholder
        // nobody threaded through, which looks fine until the one value you
        // changed is the one that stayed behind.
        // `None` in each slot as well as every key, because an empty slot is a
        // spelling in the file like any other.
        for key in TriggerKey::ALL.map(Some).into_iter().chain([None]) {
            let other = TriggerKey::ALL
                .into_iter()
                .map(Some)
                .find(|k| *k != key)
                .expect("more than one trigger");
            for settings in [
                Settings {
                    left_trigger: key,
                    right_trigger: other,
                    threshold_ms: 250,
                    dummy_vk: 124,
                },
                Settings {
                    left_trigger: other,
                    right_trigger: key,
                    threshold_ms: 1,
                    dummy_vk: 135,
                },
                Settings {
                    left_trigger: None,
                    right_trigger: None,
                    threshold_ms: 400,
                    dummy_vk: 7,
                },
            ] {
                let text = render(&settings);
                let parsed: Settings = toml::from_str(&text)
                    .unwrap_or_else(|e| panic!("{} rendered invalid TOML: {e}", slot_name(key)));
                assert_eq!(parsed, settings);
            }
        }
    }

    #[test]
    fn render_substitutes_every_field() {
        // Every field differs from its default, so a default leaking through
        // means that field was never substituted.
        let text = render(&Settings {
            left_trigger: Some(TriggerKey::LeftWin),
            right_trigger: Some(TriggerKey::Menu),
            threshold_ms: 321,
            dummy_vk: 130,
        });
        assert!(text.contains(r#"left_trigger = "LeftWin""#), "{text}");
        assert!(text.contains(r#"right_trigger = "Menu""#), "{text}");
        assert!(text.contains("threshold_ms = 321"), "{text}");
        assert!(text.contains("dummy_vk = 0x82"), "{text}");
        assert!(!text.contains(r#"= "LeftAlt""#), "{text}");
        assert!(!text.contains("= 400"), "{text}");
        assert!(!text.contains("= 0x07"), "{text}");
    }

    #[test]
    fn the_dummy_key_is_written_and_read_back_as_hex() {
        // The dialog and `--dummy` both speak hex, so the file does too; TOML
        // reads `0x07` as an integer, and an older file's decimal 7 still works.
        let text = render(&Settings::default());
        assert!(text.contains("dummy_vk = 0x07"), "{text}");
        assert_eq!(
            toml::from_str::<Settings>(&text).unwrap().dummy_vk,
            Settings::default().dummy_vk
        );
        assert_eq!(
            toml::from_str::<Settings>("dummy_vk = 7").unwrap().dummy_vk,
            0x07
        );
    }

    #[test]
    fn every_problem_fits_the_dialogs_label() {
        // The widest case of each variant: whichever trigger has the longest
        // name, in every position a name can appear.
        let longest = TriggerKey::ALL
            .into_iter()
            .max_by_key(|k| k.name().len())
            .unwrap();
        let mut all = vec![
            Problem::SameTrigger(longest),
            Problem::ThresholdZero,
            Problem::DummyZero,
            Problem::DummyIsTrigger(longest),
        ];
        // Plus whatever real settings actually produce, so a variant added
        // later cannot slip past the hand-written list above.
        all.extend(
            Settings {
                left_trigger: Some(TriggerKey::Menu),
                right_trigger: Some(TriggerKey::Menu),
                threshold_ms: 0,
                dummy_vk: 0xA4,
            }
            .problems(),
        );
        for problem in all {
            let message = problem.message();
            assert!(
                message.len() <= MESSAGE_BUDGET,
                "{problem:?} is {} characters, over the {MESSAGE_BUDGET} budget: {message}",
                message.len()
            );
        }
    }

    #[test]
    fn trigger_keys_round_trip_through_their_names() {
        let text = r#"
            left_trigger = "LeftCtrl"
            right_trigger = "RightShift"
            threshold_ms = 250
            dummy_vk = 124
        "#;
        let s: Settings = toml::from_str(text).unwrap();
        assert_eq!(s.left_trigger, Some(TriggerKey::LeftCtrl));
        assert_eq!(s.runtime().core.left_trigger, 0xA2);
        assert_eq!(s.runtime().core.right_trigger, 0xA1);
        assert_eq!(s.runtime().core.threshold_ms, 250);
        assert_eq!(s.runtime().dummy_vk, 124);
    }

    #[test]
    fn a_missing_key_falls_back_to_its_default() {
        // And so is *not* how a trigger gets switched off — `NONE_NAME` is,
        // which is the whole reason it has to be a word rather than an absence.
        let s: Settings = toml::from_str("threshold_ms = 300").unwrap();
        assert_eq!(s.threshold_ms, 300);
        assert_eq!(s.left_trigger, Some(TriggerKey::LeftAlt));
    }

    #[test]
    fn a_trigger_can_be_switched_off_from_the_file() {
        let text = format!("left_trigger = \"{NONE_NAME}\"\nright_trigger = \"{NONE_NAME}\"\n");
        let s: Settings = toml::from_str(&text).unwrap();
        assert_eq!(s.left_trigger, None);
        assert_eq!(s.right_trigger, None);
        // Both off is inert, not an error: the core matches on the vk, and
        // `VK_NONE` is not one any event can carry.
        assert_eq!(s.problems(), Vec::new());
        assert_eq!(s.runtime().core.left_trigger, VK_NONE);
        assert_eq!(s.runtime().core.right_trigger, VK_NONE);
    }

    #[test]
    fn a_misspelled_key_is_rejected_rather_than_ignored() {
        // Silently ignoring "treshold_ms" would look like the setting did nothing.
        let err = toml::from_str::<Settings>("treshold_ms = 300").unwrap_err();
        assert!(err.to_string().contains("treshold_ms"), "{err}");
    }

    #[test]
    fn an_unknown_trigger_name_is_rejected() {
        assert!(toml::from_str::<Settings>(r#"left_trigger = "Space""#).is_err());
    }

    #[test]
    fn every_trigger_name_is_accepted_by_the_parser() {
        // `name()` feeds the probes' command lines and the file's comment,
        // while `trigger_slot` reads the file itself. They must agree on every
        // variant, or a name the comment offers would be rejected on use.
        for key in TriggerKey::ALL {
            let text = format!("left_trigger = \"{}\"", key.name());
            let s: Settings = toml::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not accepted by the parser: {e}", key.name()));
            assert_eq!(s.left_trigger, Some(key));
            assert_eq!(TriggerKey::from_name(key.name()), Some(key));
        }
    }

    #[test]
    fn the_rendered_file_lists_every_trigger_and_none() {
        // A key you cannot discover is a key nobody uses, and that goes double
        // for "None": nothing else in the file hints that it exists. This also
        // guards `trigger_name_comment`, the reason that list is generated.
        let comment = trigger_name_comment();
        for name in TriggerKey::ALL
            .iter()
            .map(|key| key.name())
            .chain([NONE_NAME])
        {
            assert!(
                comment.contains(name),
                "{name} is missing from the config file's list of triggers"
            );
        }
        assert!(render(&Settings::default()).contains(&comment));
    }

    #[test]
    fn the_win_keys_are_the_ones_the_start_menu_uses() {
        assert_eq!(TriggerKey::LeftWin.vk(), 0x5B);
        assert_eq!(TriggerKey::RightWin.vk(), 0x5C);
    }

    #[test]
    fn menu_is_vk_apps_and_not_alt() {
        // Win32 names Alt `VK_MENU`, so the obvious misreading of this variant
        // is 0x12 (or the 0xA4/0xA5 pair). It is the context-menu key.
        assert_eq!(TriggerKey::Menu.vk(), 0x5D);
    }

    #[test]
    fn the_defaults_have_nothing_wrong_with_them() {
        assert_eq!(Settings::default().problems(), Vec::new());
    }

    #[test]
    fn the_same_key_cannot_mean_both_on_and_off() {
        // `side_of` tests left first, so an equal pair silently costs the user
        // "IME on" rather than producing an error anywhere.
        let s = Settings {
            left_trigger: Some(TriggerKey::LeftWin),
            right_trigger: Some(TriggerKey::LeftWin),
            ..Settings::default()
        };
        assert_eq!(
            s.problems().first().copied(),
            Some(Problem::SameTrigger(TriggerKey::LeftWin))
        );
    }

    #[test]
    fn a_zero_threshold_is_rejected() {
        // `elapsed < threshold_ms` is a strict compare, so zero turns the whole
        // feature off rather than making it very strict.
        let s = Settings {
            threshold_ms: 0,
            ..Settings::default()
        };
        assert_eq!(s.problems().first().copied(), Some(Problem::ThresholdZero));
    }

    #[test]
    fn a_dummy_that_is_also_a_trigger_key_is_rejected() {
        for key in TriggerKey::ALL {
            let s = Settings {
                dummy_vk: key.vk(),
                ..Settings::default()
            };
            assert_eq!(
                s.problems().first().copied(),
                Some(Problem::DummyIsTrigger(key)),
                "dummy {} should be rejected",
                key.name()
            );
        }
        assert_eq!(
            Settings {
                dummy_vk: 0,
                ..Settings::default()
            }
            .problems()
            .first()
            .copied(),
            Some(Problem::DummyZero)
        );
    }

    #[test]
    fn an_unusual_but_workable_value_is_not_a_problem() {
        // A threshold outside the measured band and a dummy nobody has tried
        // both still work; they are just unwise. The probes exist to measure
        // exactly these, and a front end that refused them — or nagged about
        // them — would be arguing with the one number a user comes here to tune.
        let s = Settings {
            threshold_ms: 900,
            dummy_vk: 0x60,
            ..Settings::default()
        };
        assert_eq!(s.problems(), Vec::new(), "{s:?}");
    }

    /// A trigger whose up is injected can be stranded down by a partly landed
    /// injection, and `release_stuck_keys` is the only thing that recovers it.
    /// A trigger whose up is withheld on purpose must **not** be in that list,
    /// or the release itself performs the side effect we suppressed.
    ///
    /// Tying the two together here means adding a trigger cannot get this wrong
    /// quietly in either direction.
    #[test]
    fn the_release_list_matches_how_each_trigger_is_suppressed() {
        use crate::inject::{RELEASED_ON_FAILURE, Suppression, suppression_for};
        for key in TriggerKey::ALL {
            let listed = RELEASED_ON_FAILURE.contains(&key.vk());
            match suppression_for(key.vk()) {
                Suppression::DummyThenUp => assert!(
                    listed,
                    "{} has its up injected, so it can stick down and must be released",
                    key.name()
                ),
                Suppression::Swallow => assert!(
                    !listed,
                    "{} has its up withheld on purpose; releasing it would fire the \
                     very side effect the swallow prevents",
                    key.name()
                ),
            }
        }
    }
}
