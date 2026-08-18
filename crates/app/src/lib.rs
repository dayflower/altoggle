//! Win32 adapter: the side that wires the `altoggle-core` state machine to real input.

pub mod autostart;
pub mod dialog;
pub mod hook;
pub mod icons;
pub mod ime;
pub mod inject;
pub mod keys;
pub mod log;
pub mod probe_args;
pub mod session;
pub mod settings;
pub mod single_instance;
pub mod tray;

/// Convert to the NUL-terminated UTF-16 the wide Win32 entry points expect.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
