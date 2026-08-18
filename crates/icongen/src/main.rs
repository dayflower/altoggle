//! Turns the artwork in `design/` into the assets `crates/app` embeds.
//!
//! Run by hand when the artwork changes, never as part of a build:
//!
//! ```text
//! cargo run -p altoggle-icongen
//! ```
//!
//! This crate is outside the workspace's `default-members` for that reason. The
//! app embeds finished pixels and decodes a PNG at startup; it must not acquire
//! an SVG rasteriser as a build dependency, and the machine building it must not
//! need one either.
//!
//! Two outputs, and they are not symmetrical:
//!
//! - the six tray icons, one 32x32 PNG each. `tiny_skia::Pixmap::encode_png`
//!   demultiplies on the way out, so the files carry straight RGBA — which is
//!   what `tray_icon::Icon::from_rgba` wants, with no conversion at the far end
//! - `appicon.ico`, a multi-size icon linked into the executable as a resource.
//!   Windows picks the entry it wants, so every size the shell asks for has to
//!   be in the file

use std::path::{Path, PathBuf};

use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use resvg::usvg;

/// The tray icon size. One size only, matching what the shell asks for at 100%
/// and 200% scaling; anything else is downscaled by the shell.
const TRAY_SIZE: u32 = 32;

/// Sizes to put in `appicon.ico`. Windows asks for all of these in one place or
/// another: 16 in Explorer's details view, 32 on the desktop, 256 in the extra
/// large view, and the rest at intermediate display scalings.
const APP_ICON_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

/// Above this size an entry is stored as PNG rather than BMP. A 256x256 BMP
/// entry is 256KB of uncompressed pixels in the executable for nothing.
const PNG_ENTRY_FROM: u32 = 256;

/// `<theme>-<glyph>` for each tray icon, matching both the `design/` file names
/// and the `assets/` ones, so the mapping stays greppable in both directions.
const TRAY_VARIANTS: [&str; 6] = [
    "black-en",
    "black-ja",
    "black-undef",
    "white-en",
    "white-ja",
    "white-undef",
];

fn main() {
    let root = repo_root();
    let design = root.join("design");
    let assets = root.join("crates/app/assets");

    std::fs::create_dir_all(&assets).expect("could not create the assets directory");

    for variant in TRAY_VARIANTS {
        let source = design.join(format!("altoggle-trayicon-{variant}.svg"));
        let target = assets.join(format!("tray-{variant}.png"));
        render_svg(&source, TRAY_SIZE)
            .encode_png()
            .and_then(|png| Ok(std::fs::write(&target, png)?))
            .unwrap_or_else(|e| panic!("could not write {}: {e}", target.display()));
        println!("{} -> {}", source.display(), target.display());
    }

    let source = design.join("appicon.png");
    let target = assets.join("appicon.ico");
    write_app_icon(&source, &target);
    println!("{} -> {}", source.display(), target.display());
}

/// The repository root, derived from this crate rather than the working
/// directory, so `cargo run -p altoggle-icongen` works from anywhere.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate is not two levels below the repository root")
        .to_path_buf()
}

/// Rasterise one SVG into a square pixmap.
///
/// The scale comes from the parsed tree's own size, not from the `viewBox`
/// literal: the artwork declares `width="100%"`, which usvg resolves against its
/// default size, so 960 is not necessarily the number to divide by.
fn render_svg(path: &Path, size: u32) -> Pixmap {
    let data =
        std::fs::read(path).unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
        .unwrap_or_else(|e| panic!("could not parse {}: {e}", path.display()));

    let mut pixmap = Pixmap::new(size, size).expect("the tray icon size is not zero");
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
}

/// Assemble the multi-size `.ico` from the master PNG.
fn write_app_icon(source: &Path, target: &Path) {
    let data = std::fs::read(source)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", source.display()));
    let master = Pixmap::decode_png(&data)
        .unwrap_or_else(|e| panic!("could not decode {}: {e}", source.display()));

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in APP_ICON_SIZES {
        // `IconImage` wants straight RGBA; a `Pixmap` holds premultiplied.
        let rgba = downscale(&master, size).take_demultiplied();
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        let entry = if size >= PNG_ENTRY_FROM {
            ico::IconDirEntry::encode_as_png(&image)
        } else {
            ico::IconDirEntry::encode_as_bmp(&image)
        };
        dir.add_entry(entry.unwrap_or_else(|e| panic!("could not encode the {size}px entry: {e}")));
    }

    let file = std::fs::File::create(target)
        .unwrap_or_else(|e| panic!("could not create {}: {e}", target.display()));
    dir.write(file)
        .unwrap_or_else(|e| panic!("could not write {}: {e}", target.display()));
}

/// Shrink to a square of `size`, halving repeatedly first.
///
/// The master is 1254px and the smallest entry is 16px. Resampling that in one
/// step aliases badly whatever the filter: each output pixel would be decided by
/// a handful of the ~6000 source pixels it covers. Halving averages the whole
/// area down first, and only the last step, which is never more than 2:1, needs
/// a good filter.
fn downscale(master: &Pixmap, size: u32) -> Pixmap {
    let mut current = master.clone();
    while current.width() / 2 >= size {
        current = scale_to(&current, current.width() / 2, FilterQuality::Bilinear);
    }
    if current.width() == size {
        current
    } else {
        scale_to(&current, size, FilterQuality::Bicubic)
    }
}

fn scale_to(source: &Pixmap, size: u32, quality: FilterQuality) -> Pixmap {
    let mut target = Pixmap::new(size, size).expect("an icon size is never zero");
    let scale = size as f32 / source.width() as f32;
    target.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &PixmapPaint {
            quality,
            ..Default::default()
        },
        Transform::from_scale(scale, scale),
        None,
    );
    target
}
