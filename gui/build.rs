//! Renders the application icon from its SVG, and on Windows compiles it into the executable along
//! with the version information the file's Properties dialog shows.
//!
//! Two reasons this is a build step rather than committed images. On Windows an icon is not a file
//! beside the binary the way it is under a desktop entry on Linux, it is a resource inside the
//! `.exe`, put there at link time - so `cargo install --path .` can only produce a binary that
//! carries its icon if it happens here. And rendering from the vector means the artwork in
//! `assets/icon/` is the only copy of it: nothing generated to commit, and nothing to fall out of
//! step when it changes.
//!
//! resvg brings its own rasteriser, so none of this asks for a tool on the machine.

use std::path::{Path, PathBuf};

/// Where the artwork lives, and what the icon is called.
const ICON_DIR: &str = "assets/icon";
const FULL: &str = "phonoscule.svg";
/// The simplified artwork, for sizes where the full one turns to mud (see the SVG's own comment).
const SMALL: &str = "phonoscule-small.svg";
/// At and below this, render `SMALL`.
const SMALL_UPTO: u32 = 32;

/// What Windows asks for: Explorer uses 16/32/48/256, the taskbar and Alt-Tab the ones between.
/// Largest first, since some readers take the first entry big enough rather than the closest fit.
const SIZES: [u32; 7] = [256, 128, 64, 48, 32, 24, 16];

/// The size the running player hands the window system, which scales it as it likes.
const WINDOW_ICON: u32 = 256;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dir = Path::new(ICON_DIR);
    let full = read_svg(&dir.join(FULL));
    let small = read_svg(&dir.join(SMALL));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));

    // Always: the window icon, which every platform's player sets (see `main.rs`).
    let window_icon = render(&full, WINDOW_ICON);
    std::fs::write(out.join("window-icon.png"), &window_icon).expect("writing the window icon");

    // The rest is the Windows executable's own resource. Keyed on what we are building *for*, not
    // on the host, so a Windows binary cross-compiled from elsewhere gets it too.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let frames: Vec<(u32, Vec<u8>)> = SIZES
        .iter()
        .map(|&size| {
            let svg = if size <= SMALL_UPTO { &small } else { &full };
            (size, if size == WINDOW_ICON { window_icon.clone() } else { render(svg, size) })
        })
        .collect();
    let ico = out.join("phonoscule.ico");
    std::fs::write(&ico, pack_ico(&frames)).expect("writing the icon");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().expect("a UTF-8 OUT_DIR"));
    // FileVersion and ProductVersion come from the package version; these are the strings Explorer
    // shows, which otherwise default to the crate name.
    res.set("ProductName", "Phonoscule");
    res.set("FileDescription", "Phonoscule");
    res.set("LegalCopyright", "Copyright (c) 2026 Jojo. Mozilla Public License 2.0.");

    // A missing resource compiler is not worth failing a build over: without this the binary is
    // exactly what it was before, minus the icon. `cargo:warning` says so where it will be seen.
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon and version info: {e}");
    }
}

/// Parses one of the artwork files, and tells cargo to rerun when it changes.
fn read_svg(path: &Path) -> resvg::usvg::Tree {
    println!("cargo:rerun-if-changed={}", path.display());
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

/// The artwork as a square PNG of `size` pixels, rendered from the vector at that size rather than
/// downsampled from a larger one, so the small forms stay crisp.
fn render(tree: &resvg::usvg::Tree, size: u32) -> Vec<u8> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("a non-zero size");
    let scale = size as f32 / tree.size().width();
    resvg::render(tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().expect("encoding a rendered icon")
}

/// The frames as a Windows `.ico`, in the order given.
///
/// The bodies are the PNGs themselves rather than the BMPs the original format called for. Every
/// Windows since Vista reads that, it keeps the alpha channel straightforward, and it is what keeps
/// a 256-pixel frame from costing a quarter of a megabyte.
fn pack_ico(frames: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend([0, 0]); // reserved
    out.extend(1u16.to_le_bytes()); // type 1 = icon
    out.extend((frames.len() as u16).to_le_bytes());

    // Every directory entry is a fixed 16 bytes, so the first image begins after all of them.
    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, png) in frames {
        // The width and height fields are single bytes, in which 0 means 256.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([side, side, 0 /* palette */, 0 /* reserved */]);
        out.extend(1u16.to_le_bytes()); // colour planes
        out.extend(32u16.to_le_bytes()); // bits per pixel
        out.extend((png.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in frames {
        out.extend(png);
    }
    out
}
