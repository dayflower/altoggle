//! Start with Windows, through the per-user `Run` key.
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` is enough because the
//! app runs unelevated. The plan's worry about `Run` triggering a UAC prompt on
//! every sign-in only applies to an elevated app, which would need a scheduled
//! task registered to run with highest privileges instead.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

use crate::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "altoggle";

fn open(access: u32) -> Option<HKEY> {
    let subkey = wide(RUN_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &mut key) };
    (rc == ERROR_SUCCESS).then_some(key)
}

/// The command line currently registered, if any.
pub fn registered_command() -> Option<String> {
    let key = open(KEY_READ)?;
    let name = wide(VALUE_NAME);

    // First call sizes the buffer, second one fills it.
    let mut size: u32 = 0;
    let rc = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS || size == 0 {
        unsafe { RegCloseKey(key) };
        return None;
    }

    let mut bytes = vec![0u8; size as usize];
    let rc = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(key) };
    if rc != ERROR_SUCCESS {
        return None;
    }

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    Some(text.trim_end_matches('\0').to_owned())
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
        let Some(key) = open(KEY_SET_VALUE) else {
            return Err("cannot open the Run key for writing".into());
        };
        let name = wide(VALUE_NAME);
        let rc = unsafe { RegDeleteValueW(key, name.as_ptr()) };
        unsafe { RegCloseKey(key) };
        // Deleting something already absent is the state we wanted anyway.
        return if rc == ERROR_SUCCESS || registered_command().is_none() {
            Ok(())
        } else {
            Err(format!("could not remove the Run entry (error {rc})"))
        };
    }

    let command = command_for_this_exe()?;
    let Some(key) = open(KEY_SET_VALUE) else {
        return Err("cannot open the Run key for writing".into());
    };
    let name = wide(VALUE_NAME);
    let data = wide(&command);
    // REG_SZ wants bytes, including the terminating NUL.
    let bytes = data.len() * std::mem::size_of::<u16>();
    let rc = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            bytes as u32,
        )
    };
    unsafe { RegCloseKey(key) };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("could not write the Run entry (error {rc})"))
    }
}
