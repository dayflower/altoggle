//! The command line shared by `altprobe` and `imeprobe`.
//!
//! The probes exist to measure one layer each, and a measurement you cannot
//! re-target is worth little: which key triggers, which dummy key suppresses it,
//! and how long a press still counts all have to be reachable without a rebuild.
//!
//! Unrecognised or malformed arguments are **rejected, never ignored**. A typo in
//! `--left=LeftWin` that silently left the probe watching Alt would waste a whole
//! measurement session and, worse, produce a confident wrong answer.

use altoggle_core::Config;

use crate::inject;
use crate::settings::{Settings, TriggerKey};

pub struct ProbeArgs {
    /// Seconds before quitting on our own. 0 disables it.
    pub secs: u64,
    /// The key injected to stop a solo press from looking solo.
    pub dummy_vk: u16,
    /// Solo press of this key means "off".
    pub left: TriggerKey,
    /// Solo press of this key means "on".
    pub right: TriggerKey,
    /// Send the suppression and the IME keys as two `SendInput` calls.
    /// Read by `imeprobe` only.
    pub split: bool,
    pub threshold_ms: u64,
    /// Print the header and exit without installing any hook.
    pub dry_run: bool,
}

/// Printed on a parse error, so the caller does not have to keep its own copy in
/// sync with what is actually accepted.
pub fn usage(bin: &str) -> String {
    let names: Vec<&str> = TriggerKey::ALL.iter().map(|k| k.name()).collect();
    format!(
        "usage: {bin} [--secs=N] [--dummy=HEX] [--left=KEY] [--right=KEY] \
         [--threshold=MS] [--dry-run]{split}\n  \
         KEY: {keys}\n  \
         --dry-run: print the settings and exit, installing no hook",
        split = if bin == "imeprobe" { " [--split]" } else { "" },
        keys = names.join(", "),
    )
}

impl ProbeArgs {
    pub fn parse(default_secs: u64) -> Result<Self, String> {
        let defaults = Config::default();
        let mut args = Self {
            secs: default_secs,
            dummy_vk: 0x07,
            left: TriggerKey::LeftAlt,
            right: TriggerKey::RightAlt,
            split: false,
            threshold_ms: defaults.threshold_ms,
            dry_run: false,
        };

        for arg in std::env::args().skip(1) {
            let (key, value) = match arg.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (arg.as_str(), None),
            };
            let need = |v: Option<&str>| {
                v.ok_or_else(|| format!("{key} needs a value"))
                    .map(str::to_owned)
            };
            match key {
                "--secs" => args.secs = parse_u64(&need(value)?, key)?,
                "--threshold" => args.threshold_ms = parse_u64(&need(value)?, key)?,
                "--dummy" => {
                    let v = need(value)?;
                    args.dummy_vk = u16::from_str_radix(v.trim_start_matches("0x"), 16)
                        .map_err(|e| format!("--dummy={v} is not a hex byte: {e}"))?;
                }
                "--left" => args.left = parse_trigger(&need(value)?)?,
                "--right" => args.right = parse_trigger(&need(value)?)?,
                "--split" => args.split = true,
                "--dry-run" => args.dry_run = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }

        // `problems` reports only what cannot work, so an odd but measurable
        // value like --threshold=900 still runs, which is the whole point of a
        // probe. What must not run is a combination that cannot do what the
        // flags say it does.
        if let Some(problem) = args.settings().problems().first() {
            return Err(problem.message());
        }
        Ok(args)
    }

    /// The probes always watch both sides, so neither slot is ever empty here.
    fn settings(&self) -> Settings {
        Settings {
            left_trigger: Some(self.left),
            right_trigger: Some(self.right),
            threshold_ms: self.threshold_ms,
            dummy_vk: self.dummy_vk,
        }
    }

    pub fn config(&self) -> Config {
        Config {
            left_trigger: self.left.vk(),
            right_trigger: self.right.vk(),
            threshold_ms: self.threshold_ms,
        }
    }

    /// The header every probe prints.
    ///
    /// It spells out the scan code and extendedness of everything injected,
    /// because both are invisible when wrong: a Win up missing
    /// `KEYEVENTF_EXTENDEDKEY` sticks the key down, and a dummy with no scan code
    /// silently fails to suppress the start menu.
    pub fn describe(&self) -> String {
        let show = |vk: u16, name: &str| {
            format!(
                "{name} (vk 0x{vk:02X}, scan 0x{:02X}{})",
                inject::scan_of(vk),
                if inject::is_extended(vk) {
                    ", extended"
                } else {
                    ""
                }
            )
        };
        format!(
            "built from: {}\noff: {}\non:  {}\ndummy: {}\nthreshold: {}ms",
            // Which source tree this binary came from. A measurement session was
            // once lost to running the probe from a different checkout, where
            // the old positional parser silently discarded every flag and
            // watched Alt throughout.
            env!("CARGO_MANIFEST_DIR"),
            show(self.left.vk(), self.left.name()),
            show(self.right.vk(), self.right.name()),
            show(self.dummy_vk, "dummy"),
            self.threshold_ms,
        )
    }
}

fn parse_u64(v: &str, key: &str) -> Result<u64, String> {
    v.parse()
        .map_err(|e| format!("{key}={v} is not a number: {e}"))
}

fn parse_trigger(v: &str) -> Result<TriggerKey, String> {
    TriggerKey::from_name(v).ok_or_else(|| {
        let names: Vec<&str> = TriggerKey::ALL.iter().map(|k| k.name()).collect();
        format!("{v} is not a trigger key. One of: {}", names.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_misspelled_trigger_is_an_error_rather_than_a_silent_default() {
        // The whole point: a probe run that quietly watched the wrong key would
        // report a confident wrong answer.
        assert!(parse_trigger("LeftWindows").is_err());
        assert_eq!(parse_trigger("leftwin"), Ok(TriggerKey::LeftWin));
    }

    #[test]
    fn the_usage_line_lists_every_trigger() {
        let text = usage("altprobe");
        for key in TriggerKey::ALL {
            assert!(text.contains(key.name()), "{} missing", key.name());
        }
    }
}
