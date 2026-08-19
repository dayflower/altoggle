//! Start with Windows, through the per-user `Run` key.
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` is enough because the
//! app runs unelevated. The plan's worry about `Run` triggering a UAC prompt on
//! every sign-in only applies to an elevated app, which would need a scheduled
//! task registered to run with highest privileges instead.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{KEY_READ, KEY_SET_VALUE};

use crate::registry::Key;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "altoggle";

/// The command line currently registered, if any.
pub fn registered_command() -> Option<String> {
    Key::open_hkcu(RUN_KEY, KEY_READ)?.string(VALUE_NAME)
}

/// The command line that should be registered for this executable.
///
/// Quoted, because an unquoted path containing a space is parsed as several
/// arguments.
pub fn command_for_this_exe() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find our own path: {e}"))?;
    Ok(format!("\"{}\"", exe.display()))
}

pub fn is_enabled() -> bool {
    registered_command().is_some()
}

/// Register or unregister. Enabling always rewrites the path, so a stale entry
/// left by a moved executable is repaired by toggling off and on.
pub fn set_enabled(on: bool) -> Result<(), String> {
    if !on {
        let Some(key) = Key::open_hkcu(RUN_KEY, KEY_SET_VALUE) else {
            return Err("cannot open the Run key for writing".into());
        };
        let rc = key.delete_value(VALUE_NAME);
        // Deleting something already absent is the state we wanted anyway.
        return if rc == ERROR_SUCCESS || registered_command().is_none() {
            Ok(())
        } else {
            Err(format!("could not remove the Run entry (error {rc})"))
        };
    }

    let command = command_for_this_exe()?;
    let Some(key) = Key::open_hkcu(RUN_KEY, KEY_SET_VALUE) else {
        return Err("cannot open the Run key for writing".into());
    };
    let rc = key.set_string(VALUE_NAME, &command);
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("could not write the Run entry (error {rc})"))
    }
}
