//! Which tray artwork to show, and how to get at it.
//!
//! Two independent choices, six pictures:
//!
//! - the glyph says what the IME is doing — a Latin **a** for off, a hiragana
//!   **あ** for on, an asterisk for "cannot tell"
//! - the theme says what colour the taskbar is, and therefore whether the glyph
//!   has to be black or white to be visible on it
//!
//! The artwork is committed as PNG under `assets/` and rasterised from
//! `design/*.svg` by `cargo run -p altoggle-icongen`, run by hand. Nothing here
//! parses SVG: a resident app should not carry a rasteriser, and building it
//! should not need one.
//!
//! **The theme is polled, not pushed, and that is not an oversight.** Windows
//! announces a light/dark switch by broadcasting `WM_SETTINGCHANGE` with
//! `"ImmersiveColorSet"`, and the only window this process owns for the purpose
//! is `session`'s, which is parented to `HWND_MESSAGE` and so receives no
//! broadcasts at all. Re-reading the registry on the tick that already reads the
//! IME costs one cached-hive lookup and needs no second window.

use std::io::Cursor;

use tray_icon::Icon;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
};

use crate::wide;

/// Every tray icon is this square. The shell asks for the real pixel size (the
/// manifest makes the process per-monitor DPI aware), so anything else is the
/// shell's own downscale.
const SIZE: u32 = 32;

const THEME_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const THEME_VALUE: &str = "SystemUsesLightTheme";

/// Which colour the glyph is drawn in, chosen to contrast with the taskbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Black,
    White,
}

/// What the icon says about the IME.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Latin,
    Kana,
    Unknown,
}

/// Pick the glyph for what `ime::read_open_status` reported.
///
/// `None` gets its own picture rather than falling back to `Latin`. It means
/// *unreadable*, not *off*: some Electron apps never answer IMM32, and showing
/// **a** there would assert something we did not measure. The `undef` artwork
/// also drops the direction arrow the other two carry, so the three are
/// distinguishable at 32 pixels.
///
/// Store apps and WinUI 3 apps are a different problem and are not solved here:
/// they answer, but wrongly, unless the query goes to the right window. See
/// `ime::input_window`.
pub fn glyph_for(status: Option<bool>) -> Glyph {
    match status {
        Some(true) => Glyph::Kana,
        Some(false) => Glyph::Latin,
        None => Glyph::Unknown,
    }
}

/// Read the taskbar's current colour scheme.
///
/// `SystemUsesLightTheme` is the one that governs the taskbar and tray;
/// `AppsUseLightTheme` next to it governs application chrome and is not what we
/// are drawing on. Anything other than a readable `1` is treated as a dark
/// taskbar, which is both the Windows default and what the taskbar looked like
/// before this value existed.
pub fn detect_theme() -> Theme {
    if light_taskbar() == Some(true) {
        Theme::Black
    } else {
        Theme::White
    }
}

fn light_taskbar() -> Option<bool> {
    let subkey = wide(THEME_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut key) };
    if rc != ERROR_SUCCESS {
        return None;
    }

    let name = wide(THEME_VALUE);
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut u8,
            &mut size,
        )
    };
    unsafe { RegCloseKey(key) };
    (rc == ERROR_SUCCESS).then_some(value != 0)
}

/// The six icons, decoded once.
///
/// `Icon` is a reference-counted `HICON`, so handing one out is a clone of an
/// `Arc` rather than another `CreateIcon`.
pub struct Set {
    black_latin: Icon,
    black_kana: Icon,
    black_unknown: Icon,
    white_latin: Icon,
    white_kana: Icon,
    white_unknown: Icon,
}

impl Set {
    pub fn load() -> Result<Self, String> {
        Ok(Self {
            black_latin: decode(include_bytes!("../assets/tray-black-en.png"))?,
            black_kana: decode(include_bytes!("../assets/tray-black-ja.png"))?,
            black_unknown: decode(include_bytes!("../assets/tray-black-undef.png"))?,
            white_latin: decode(include_bytes!("../assets/tray-white-en.png"))?,
            white_kana: decode(include_bytes!("../assets/tray-white-ja.png"))?,
            white_unknown: decode(include_bytes!("../assets/tray-white-undef.png"))?,
        })
    }

    pub fn get(&self, theme: Theme, glyph: Glyph) -> Icon {
        let icon = match (theme, glyph) {
            (Theme::Black, Glyph::Latin) => &self.black_latin,
            (Theme::Black, Glyph::Kana) => &self.black_kana,
            (Theme::Black, Glyph::Unknown) => &self.black_unknown,
            (Theme::White, Glyph::Latin) => &self.white_latin,
            (Theme::White, Glyph::Kana) => &self.white_kana,
            (Theme::White, Glyph::Unknown) => &self.white_unknown,
        };
        icon.clone()
    }
}

/// Decode one embedded PNG into an icon.
///
/// The files come from `icongen`, which always writes straight-alpha 32x32
/// RGBA8 — exactly what `Icon::from_rgba` takes — so a mismatch here means the
/// assets are stale, not that some runtime case went unhandled. A test asserts
/// the same thing at `cargo test` time, where the message is more use than a
/// blank tray icon is.
fn decode(png: &[u8]) -> Result<Icon, String> {
    let decoder = png::Decoder::new(Cursor::new(png));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("bad icon PNG: {e}"))?;
    let mut rgba = vec![
        0u8;
        reader
            .output_buffer_size()
            .ok_or("the icon PNG is implausibly large")?
    ];
    let info = reader
        .next_frame(&mut rgba)
        .map_err(|e| format!("bad icon PNG: {e}"))?;

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "icon PNG is {:?}/{:?}, expected Rgba/Eight; regenerate the assets",
            info.color_type, info.bit_depth
        ));
    }
    if (info.width, info.height) != (SIZE, SIZE) {
        return Err(format!(
            "icon PNG is {}x{}, expected {SIZE}x{SIZE}; regenerate the assets",
            info.width, info.height
        ));
    }

    // next_frame may report fewer rows than the buffer holds.
    rgba.truncate((info.width * info.height * 4) as usize);
    Icon::from_rgba(rgba, info.width, info.height).map_err(|e| format!("bad icon PNG: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_is_its_own_glyph() {
        assert_eq!(glyph_for(Some(true)), Glyph::Kana);
        assert_eq!(glyph_for(Some(false)), Glyph::Latin);
        // The tempting simplification is to fold this into Latin. It would make
        // every window that stays silent claim the IME is off.
        assert_eq!(glyph_for(None), Glyph::Unknown);
    }

    #[test]
    fn every_asset_is_32x32_rgba8() {
        const ASSETS: [(&str, &[u8]); 6] = [
            (
                "tray-black-en",
                include_bytes!("../assets/tray-black-en.png"),
            ),
            (
                "tray-black-ja",
                include_bytes!("../assets/tray-black-ja.png"),
            ),
            (
                "tray-black-undef",
                include_bytes!("../assets/tray-black-undef.png"),
            ),
            (
                "tray-white-en",
                include_bytes!("../assets/tray-white-en.png"),
            ),
            (
                "tray-white-ja",
                include_bytes!("../assets/tray-white-ja.png"),
            ),
            (
                "tray-white-undef",
                include_bytes!("../assets/tray-white-undef.png"),
            ),
        ];

        for (name, bytes) in ASSETS {
            let mut reader = png::Decoder::new(Cursor::new(bytes))
                .read_info()
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let mut buffer = vec![0u8; reader.output_buffer_size().unwrap()];
            let info = reader
                .next_frame(&mut buffer)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((info.width, info.height), (SIZE, SIZE), "{name}");
            assert_eq!(info.color_type, png::ColorType::Rgba, "{name}");
            assert_eq!(info.bit_depth, png::BitDepth::Eight, "{name}");
        }
    }
}
