//! Settings, read from a TOML file.
//!
//! Trigger keys are named rather than numeric, because a virtual-key code is a
//! terrible thing to hand-edit and because the eventual settings dialog wants
//! exactly this list to populate a dropdown.
//!
//! The file is the interim front end for changing settings; a dialog replaces it
//! later. Either way the values reach the running state machine the same way:
//! `HookThread::set_config` posts them to the hook thread's message loop.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use altoggle_core::Config;

/// A key that can act as a trigger.
///
/// Deliberately not "any key": the suppression strategy differs per key (Alt
/// opens the menu bar, Win opens the start menu), so only keys we have a
/// strategy for belong here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerKey {
    LeftAlt,
    RightAlt,
    LeftCtrl,
    RightCtrl,
    LeftShift,
    RightShift,
}

impl TriggerKey {
    pub fn vk(self) -> u16 {
        match self {
            TriggerKey::LeftAlt => 0xA4,
            TriggerKey::RightAlt => 0xA5,
            TriggerKey::LeftCtrl => 0xA2,
            TriggerKey::RightCtrl => 0xA3,
            TriggerKey::LeftShift => 0xA0,
            TriggerKey::RightShift => 0xA1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    /// Solo press of this key turns the IME off.
    pub left_trigger: TriggerKey,
    /// Solo press of this key turns the IME on.
    pub right_trigger: TriggerKey,
    /// A press shorter than this counts as a solo press.
    pub threshold_ms: u64,
    /// The key injected to stop a solo press from looking solo.
    pub dummy_vk: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            left_trigger: TriggerKey::LeftAlt,
            right_trigger: TriggerKey::RightAlt,
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
                left_trigger: self.left_trigger.vk(),
                right_trigger: self.right_trigger.vk(),
                threshold_ms: self.threshold_ms,
            },
            dummy_vk: self.dummy_vk,
        }
    }
}

/// Written when no config file exists yet.
///
/// Hand-written rather than serialized, because a serializer cannot emit the
/// comments, and the comments are most of the value of a file you edit by hand.
const TEMPLATE: &str = r#"# altoggle configuration.
# Edit and choose "Reload config" from the tray menu to apply.

# Which key turns the IME off, and which turns it on.
# One of: LeftAlt, RightAlt, LeftCtrl, RightCtrl, LeftShift, RightShift
left_trigger = "LeftAlt"
right_trigger = "RightAlt"

# A press shorter than this counts as a solo press.
#
# Measured: deliberate solo presses reach 215ms, auto-repeat starts around
# 500ms. Below ~250ms you will miss presses you meant; above ~500ms a held key
# starts firing.
#
# This threshold is also the escape hatch: hold the key past it and the press
# falls through to Windows untouched, so Alt still opens the menu bar.
threshold_ms = 400

# The key injected to make a solo press stop looking solo, which is what stops
# Windows from opening the menu bar. 7 is an undefined virtual key and was
# harmless in every application tested. 124-135 (VK_F13 to VK_F24) also work.
dummy_vk = 7
"#;

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
        return match std::fs::write(&path, TEMPLATE) {
            Ok(()) => Loaded::Created(defaults),
            Err(e) => Loaded::Failed(defaults, format!("could not write {}: {e}", path.display())),
        };
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return Loaded::Failed(defaults, format!("could not read {}: {e}", path.display())),
    };
    match toml::from_str::<Settings>(&text) {
        Ok(s) => Loaded::Existing(s),
        Err(e) => Loaded::Failed(defaults, format!("{} is invalid: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_parses_into_the_defaults() {
        // The file handed to the user must not disagree with the built-in
        // defaults, or the first "Reload config" would silently change behaviour.
        let parsed: Settings = toml::from_str(TEMPLATE).expect("template must be valid TOML");
        assert_eq!(parsed, Settings::default());
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
        assert_eq!(s.left_trigger, TriggerKey::LeftCtrl);
        assert_eq!(s.runtime().core.left_trigger, 0xA2);
        assert_eq!(s.runtime().core.right_trigger, 0xA1);
        assert_eq!(s.runtime().core.threshold_ms, 250);
        assert_eq!(s.runtime().dummy_vk, 124);
    }

    #[test]
    fn a_missing_key_falls_back_to_its_default() {
        let s: Settings = toml::from_str("threshold_ms = 300").unwrap();
        assert_eq!(s.threshold_ms, 300);
        assert_eq!(s.left_trigger, TriggerKey::LeftAlt);
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
}
