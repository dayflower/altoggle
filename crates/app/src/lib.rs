//! Win32 adapter: the side that wires the `altoggle-core` state machine to real input.

pub mod autostart;
pub mod dialog;
pub mod dispatch;
pub mod hook;
pub mod icons;
pub mod ime;
pub mod inject;
pub mod keys;
pub mod log;
pub mod lowlevel;
pub mod probe_args;
pub mod probe_exit;
pub mod probe_log;
pub mod registry;
pub mod session;
pub mod settings;
pub mod single_instance;
pub mod tray;

/// Convert to the NUL-terminated UTF-16 the wide Win32 entry points expect.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    /// The version resource repeats what Cargo.toml already says, because
    /// rc.exe cannot read Cargo.toml. Left unchecked that is a fact with two
    /// homes, and the one nobody looks at is the one that goes stale: a release
    /// would then ship claiming the previous version, with nothing to notice it
    /// but a user reading the file properties.
    #[test]
    fn the_version_resource_agrees_with_the_package_version() {
        let rc = include_str!("../app.rc");
        let version = env!("CARGO_PKG_VERSION");

        for field in ["FileVersion", "ProductVersion"] {
            let expected = format!("VALUE \"{field}\", \"{version}\"");
            assert!(
                rc.contains(&expected),
                "crates/app/app.rc is missing `{expected}`; bump it with Cargo.toml"
            );
        }

        // FILEVERSION wants four comma-separated numbers, so the three-part
        // package version gains a trailing build number.
        let numeric = format!("{},0", version.replace('.', ","));
        for field in ["FILEVERSION", "PRODUCTVERSION"] {
            let expected = format!("{field} {numeric}");
            assert!(
                rc.contains(&expected),
                "crates/app/app.rc is missing `{expected}`; bump it with Cargo.toml"
            );
        }
    }
}
