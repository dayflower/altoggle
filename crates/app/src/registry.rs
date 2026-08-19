//! Minimal RAII wrapper over the handful of HKCU reads and writes this app makes.
//!
//! Not a registry library. It exists because `autostart` and `icons` each wrote
//! `RegOpenKeyExW` ... `RegCloseKey` around every access by hand, and one of
//! those functions has three returns between the two calls. Nothing leaked, but
//! the shape invited it: the next early return added to such a function leaks by
//! default. Closing on drop makes the leak impossible rather than merely absent.
//!
//! What a failure means stays with the caller. This module answers `None` or a
//! raw status code and knows nothing about the Run key or the taskbar theme.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};

use crate::wide;

/// A HKCU key that closes itself.
pub struct Key {
    handle: HKEY,
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.handle) };
    }
}

impl Key {
    /// Open a subkey of HKCU. `None` when it does not exist or cannot be opened
    /// with the access asked for.
    pub fn open_hkcu(subkey: &str, access: u32) -> Option<Key> {
        let subkey = wide(subkey);
        let mut handle: HKEY = std::ptr::null_mut();
        let rc =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &mut handle) };
        (rc == ERROR_SUCCESS).then_some(Key { handle })
    }

    /// A DWORD value. `None` when it cannot be read.
    ///
    /// Unreadable and zero stay apart: `icons` acts only on a readable `1`, and
    /// folding the two together would let an absent value mean something.
    pub fn dword(&self, name: &str) -> Option<u32> {
        let name = wide(name);
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegQueryValueExW(
                self.handle,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut value as *mut u32 as *mut u8,
                &mut size,
            )
        };
        (rc == ERROR_SUCCESS).then_some(value)
    }

    /// A string value. `None` when it cannot be read.
    ///
    /// The value's type is not checked, so a `REG_EXPAND_SZ` comes back with its
    /// `%VARS%` still in it rather than being refused.
    pub fn string(&self, name: &str) -> Option<String> {
        let name = wide(name);

        // First call sizes the buffer, second one fills it. A value that grows
        // between the two comes back as ERROR_MORE_DATA, and so as None.
        let mut size: u32 = 0;
        let rc = unsafe {
            RegQueryValueExW(
                self.handle,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS || size == 0 {
            return None;
        }

        let mut bytes = vec![0u8; size as usize];
        let rc = unsafe {
            RegQueryValueExW(
                self.handle,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                bytes.as_mut_ptr(),
                &mut size,
            )
        };
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

    /// Write a REG_SZ. Returns the raw status code, because what counts as
    /// success is the caller's business.
    pub fn set_string(&self, name: &str, value: &str) -> u32 {
        let name = wide(name);
        let data = wide(value);
        // REG_SZ wants bytes, including the terminating NUL.
        let bytes = data.len() * std::mem::size_of::<u16>();
        unsafe {
            RegSetValueExW(
                self.handle,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                bytes as u32,
            )
        }
    }

    /// Delete a value. Returns the raw status code: deleting something already
    /// absent fails here, and to a caller that wanted it gone it may not be a
    /// failure at all.
    pub fn delete_value(&self, name: &str) -> u32 {
        let name = wide(name);
        unsafe { RegDeleteValueW(self.handle, name.as_ptr()) }
    }
}
