//! Win32 adapter: the side that wires the `altoggle-core` state machine to real input.
//!
//! Holds the parts shared between the verification tools (`altprobe`, `imeprobe`)
//! and the app itself.

pub mod ime;
pub mod inject;
