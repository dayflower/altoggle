//! Embeds an application manifest.
//!
//! Two things the settings dialog needs and cannot get any other way:
//!
//! - a dependency on version 6 of the common controls, so its buttons, edits
//!   and comboboxes are themed rather than Windows-95 grey
//! - per-monitor DPI awareness, so the hand-built layout is laid out in real
//!   pixels instead of being bitmap-stretched by the compositor
//!
//! `new_manifest` supplies both already. `PerMonitorV2` is chosen over the
//! default `PerMonitorV2Only` because the latter emits no `dpiAware` fallback,
//! which would leave anything before Windows 10 1607 running DPI-unaware.
//!
//! Two consequences worth knowing, both process-wide:
//!
//! - `cargo:rustc-link-arg-bins` is per crate, not per target, so `keylog`,
//!   `altprobe` and `imeprobe` get the same manifest. They create no windows,
//!   but they now run DPI-aware. Test binaries are unaffected
//! - the tray icon is generated at a fixed 32x32 (`tray::make_icon`), and the
//!   shell now asks for the real pixel size rather than a scaled-up one

use embed_manifest::manifest::DpiAwareness;
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("altoggle").dpi_awareness(DpiAwareness::PerMonitorV2))
            .expect("could not embed the application manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
