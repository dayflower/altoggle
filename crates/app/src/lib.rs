//! Win32 adapter: the side that wires the `altoggle-core` state machine to real input.

pub mod hook;
pub mod ime;
pub mod inject;
pub mod log;
pub mod session;
pub mod single_instance;

/// Convert to the NUL-terminated UTF-16 the wide Win32 entry points expect.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
