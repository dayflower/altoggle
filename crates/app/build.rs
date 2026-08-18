//! Embeds an application manifest and the application icon.
//!
//! # The manifest
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
//! - the tray icons are committed at a fixed 32x32 (`crates/app/assets`), and
//!   the shell now asks for the real pixel size rather than a scaled-up one
//!
//! # The icon
//!
//! `app.rc` is compiled and linked, giving the executable an `RT_GROUP_ICON`.
//! Three things about it:
//!
//! - **`app.rc` must never gain a manifest.** `embed_manifest` already supplies
//!   one, and two `RT_MANIFEST` resources produce an executable Windows refuses
//!   to start. Combining the two crates is safe only while the `.rc` stays a
//!   single `ICON` line
//! - the resource is per crate, not per target, exactly like the manifest, so
//!   the three probe binaries carry altoggle's icon too. Cargo offers no way to
//!   scope it, and it is harmless
//! - **this makes `rc.exe` from the Windows SDK a build requirement.** A
//!   toolchain with `link.exe` but no SDK resource compiler fails here and
//!   nowhere else, which is why the failure is not swallowed

use embed_manifest::manifest::DpiAwareness;
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("altoggle").dpi_awareness(DpiAwareness::PerMonitorV2))
            .expect("could not embed the application manifest");

        // `manifest_required` is the variant that treats "no resource compiler
        // was found" as an error rather than a shrug. The name is about
        // embed-resource's own use case; what it buys here is that a build
        // without an icon fails loudly instead of shipping.
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("could not embed the application icon");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/appicon.ico");
}
