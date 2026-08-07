//! `icongen` — turn the two signed-off brand SVGs into every raster ksx ships.
//!
//! Run it from the repo root:
//!
//! ```text
//! cargo run --manifest-path tools/icongen/Cargo.toml --release
//! ```
//!
//! It is the ONLY thing that reads `assets/brand/*.svg`, and everything it
//! writes is committed. Nothing in a normal `cargo build` runs this — the
//! rasters are checked in exactly like `crates/ksx-studio/assets/`, so
//! building ksx needs a Rust toolchain and nothing else.
//!
//! # The one idea worth understanding
//!
//! There are two drawings, not one drawing at eight sizes:
//!
//! | size | art | why |
//! |---|---|---|
//! | 16, 20, 24, 32 | `ksx-simple.svg` | a 2.4-unit outline is 0.6 px here — it is not a line, it is grey mud over every edge |
//! | 48, 64, 128, 256 | `ksx-detailed.svg` | there is room for the bezel, the fifth key row and the pad silhouettes |
//!
//! [`ICO_ENTRIES`] is that table, and it is what makes the `.ico` worth
//! producing at all: Windows picks the entry matching the surface it is
//! drawing (16 px tray, 32 px alt-tab, 256 px extra-large view) and gets art
//! drawn FOR that surface. An `.ico` built by scaling one PNG eight times
//! would be a strictly worse file at seven of the eight sizes.
//!
//! # Straight vs premultiplied alpha
//!
//! `tiny-skia` composites in PREMULTIPLIED alpha. PNG, ICO and
//! `egui::IconData` all want STRAIGHT alpha. [`render`] demultiplies once and
//! every consumer shares that buffer, so the three outputs cannot disagree
//! about what the antialiased corners of the plate look like.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use resvg::tiny_skia;
use resvg::usvg;

/// Which drawing serves which size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Art {
    /// `ksx-simple.svg` — no outline, fatter keys, pads as plain rects.
    Simple,
    /// `ksx-detailed.svg` — outlines, bezel, Lucide pad silhouettes, speculars.
    Detailed,
}

impl Art {
    fn file(self) -> &'static str {
        match self {
            Art::Simple => "ksx-simple.svg",
            Art::Detailed => "ksx-detailed.svg",
        }
    }
}

/// The handoff, in one table. Order is the order the entries land in the
/// `.ico` directory — smallest first, which is what shells with a naive
/// "first entry wins" fallback want to find.
const ICO_ENTRIES: &[(u32, Art)] = &[
    (16, Art::Simple),
    (20, Art::Simple),
    (24, Art::Simple),
    (32, Art::Simple),
    (48, Art::Detailed),
    (64, Art::Detailed),
    (128, Art::Detailed),
    (256, Art::Detailed),
];

/// The plate colour, for the one output that must not be transparent — see
/// [`write_apple_touch`]. Kept in sync with `PLATE` in both SVGs by
/// [`assert_plate_matches_art`], which reads the pixel rather than trusting
/// this constant.
const PLATE: [u8; 3] = [0x33, 0x1a, 0x4d];

/// Size of the raw RGBA blob the egui cabinet window embeds.
const CABINET_ICON_PX: u32 = 256;

/// Size of the iOS home-screen icon. Apple's own guidance is 180×180 for
/// @3x phones; iOS downsamples it for everything smaller.
const APPLE_TOUCH_PX: u32 = 180;

fn main() {
    if let Err(e) = run() {
        eprintln!("icongen: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let repo = repo_root()?;
    let brand = repo.join("assets/brand");
    let dist = brand.join("dist");
    let studio_brand = repo.join("crates/ksx-studio/brand");

    fs::create_dir_all(&dist).map_err(|e| format!("create {}: {e}", dist.display()))?;
    fs::create_dir_all(&studio_brand)
        .map_err(|e| format!("create {}: {e}", studio_brand.display()))?;

    let simple = load(&brand.join(Art::Simple.file()))?;
    let detailed = load(&brand.join(Art::Detailed.file()))?;
    let tree = |art: Art| match art {
        Art::Simple => &simple,
        Art::Detailed => &detailed,
    };

    assert_plate_matches_art(&detailed)?;

    // ---- per-size PNGs + the ICO, from the same buffers ------------------
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    let mut written: Vec<(PathBuf, usize)> = Vec::new();

    for &(px, art) in ICO_ENTRIES {
        let rgba = render(tree(art), px);

        let png_path = dist.join(format!("ksx-{px}.png"));
        let bytes = write_png(&png_path, px, &rgba)?;
        written.push((png_path, bytes));

        let image = ico::IconImage::from_rgba_data(px, px, rgba);
        icon_dir.add_entry(
            ico::IconDirEntry::encode(&image).map_err(|e| format!("encode {px} px entry: {e}"))?,
        );
    }

    let ico_path = dist.join("ksx.ico");
    let ico_bytes = {
        let file = fs::File::create(&ico_path).map_err(|e| format!("create ksx.ico: {e}"))?;
        icon_dir
            .write(BufWriter::new(file))
            .map_err(|e| format!("write ksx.ico: {e}"))?;
        len(&ico_path)?
    };
    written.push((ico_path.clone(), ico_bytes));

    // ---- the egui cabinet window icon: raw straight-alpha RGBA -----------
    //
    // Not a PNG, on purpose. `ksx-cabinet`'s dependency list is its boundary
    // (ksx-api + eframe + egui, and that is the whole list); a PNG would drag
    // a decoder in behind it just to fill a `Vec<u8>` that `egui::IconData`
    // wants uncompressed anyway. A bare .rgba blob costs the cabinet build
    // 256 KB of `.rdata` and zero crates.
    let cabinet = render(tree(Art::Detailed), CABINET_ICON_PX);
    let rgba_path = dist.join(format!("ksx-{CABINET_ICON_PX}.rgba"));
    fs::write(&rgba_path, &cabinet).map_err(|e| format!("write {}: {e}", rgba_path.display()))?;
    written.push((rgba_path, cabinet.len()));

    // ---- the web trio, straight into the ksx-studio embed ----------------
    //
    // `crates/ksx-studio/brand/` and NOT `crates/ksx-studio/assets/`: the
    // assets folder is the output directory of `studio-ui/build.mjs`, and
    // @getforma/build `rmSync`s it before every build. Anything of ours
    // parked there survives exactly until the next UI rebuild.
    let favicon_ico = studio_brand.join("favicon.ico");
    fs::copy(&ico_path, &favicon_ico).map_err(|e| format!("copy favicon.ico: {e}"))?;
    written.push((favicon_ico, ico_bytes));

    // favicon.svg is the SIMPLIFIED master, byte for byte. A browser that
    // prefers the SVG icon renders it into a 16–32 px tab, which is the
    // simplified mark's entire job; handing it the detailed one would undo
    // the handoff the .ico above exists to make.
    let favicon_svg = studio_brand.join("favicon.svg");
    fs::copy(brand.join(Art::Simple.file()), &favicon_svg)
        .map_err(|e| format!("copy favicon.svg: {e}"))?;
    let svg_bytes = len(&favicon_svg)?;
    written.push((favicon_svg, svg_bytes));

    let apple = write_apple_touch(tree(Art::Detailed), &dist, &studio_brand)?;
    written.extend(apple);

    for (path, bytes) in &written {
        println!(
            "{:>8}  {}",
            bytes,
            path.strip_prefix(&repo).unwrap_or(path).display()
        );
    }
    println!("icongen: {} files", written.len());
    Ok(())
}

/// The 180 px iOS icon, written twice (dist + the studio embed).
///
/// Flattened onto opaque [`PLATE`] instead of keeping the plate's transparent
/// corners: iOS composites `apple-touch-icon` over BLACK and then applies its
/// own superellipse mask, so transparent corners come out as four black
/// wedges peeking past the rounded plate. Filling behind the mark with its
/// own plate colour makes the mask invisible, which is what "looks right
/// pinned to a home screen" means.
fn write_apple_touch(
    tree: &usvg::Tree,
    dist: &Path,
    studio_brand: &Path,
) -> Result<Vec<(PathBuf, usize)>, String> {
    let mut rgba = render(tree, APPLE_TOUCH_PX);
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        for (c, plate) in px[..3].iter_mut().zip(PLATE) {
            // Straight-alpha `over`: out = src·a + plate·(1−a), rounded.
            let v = u32::from(*c) * a + u32::from(plate) * (255 - a);
            *c = ((v + 127) / 255) as u8;
        }
        px[3] = 255;
    }

    let dist_path = dist.join("apple-touch-icon.png");
    let n = write_png(&dist_path, APPLE_TOUCH_PX, &rgba)?;
    let embed_path = studio_brand.join("apple-touch-icon.png");
    fs::copy(&dist_path, &embed_path).map_err(|e| format!("copy apple-touch-icon.png: {e}"))?;
    Ok(vec![(dist_path, n), (embed_path, n)])
}

/// Rasterise one tree at `px`×`px`, returning STRAIGHT-alpha RGBA8.
///
/// Both masters are `viewBox="0 0 64 64"`, so the scale is uniform and the
/// mark always lands on the pixel grid the same way at every size.
fn render(tree: &usvg::Tree, px: u32) -> Vec<u8> {
    let mut pixmap = tiny_skia::Pixmap::new(px, px).expect("non-zero icon size");
    let scale = px as f32 / tree.size().width();
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for pixel in pixmap.pixels() {
        let c = pixel.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

fn write_png(path: &Path, px: u32, rgba: &[u8]) -> Result<usize, String> {
    let file = fs::File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), px, px);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // These files are committed and shipped, and regenerated by hand a
    // handful of times a year — trade encode time for bytes every time.
    encoder.set_compression(png::Compression::High);
    encoder
        .write_header()
        .map_err(|e| format!("{}: header: {e}", path.display()))?
        .write_image_data(rgba)
        .map_err(|e| format!("{}: pixels: {e}", path.display()))?;
    len(path)
}

fn load(path: &Path) -> Result<usvg::Tree, String> {
    let data = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    // No text is rendered by either master (`<title>` is metadata), so no
    // font database is loaded and the output is byte-identical on any machine.
    usvg::Tree::from_data(&data, &usvg::Options::default())
        .map_err(|e| format!("parse {}: {e}", path.display()))
}

/// The masters are the palette's source of truth; [`PLATE`] is a copy, and a
/// copy that drifts would flatten the iOS icon onto the wrong colour with
/// nothing to notice. So read the actual plate pixel and compare.
fn assert_plate_matches_art(detailed: &usvg::Tree) -> Result<(), String> {
    let rgba = render(detailed, 64);
    // (32, 4): dead centre horizontally, inside the plate's top edge (y=1)
    // and above the keyboard case (y=6) and both pads (y≥9) — plate only.
    let i = ((4 * 64 + 32) * 4) as usize;
    let got = [rgba[i], rgba[i + 1], rgba[i + 2]];
    if got != PLATE {
        return Err(format!(
            "PLATE constant is #{:02x}{:02x}{:02x} but the art renders \
             #{:02x}{:02x}{:02x} — the palette moved; update icongen",
            PLATE[0], PLATE[1], PLATE[2], got[0], got[1], got[2]
        ));
    }
    Ok(())
}

fn len(path: &Path) -> Result<usize, String> {
    fs::metadata(path)
        .map(|m| m.len() as usize)
        .map_err(|e| format!("stat {}: {e}", path.display()))
}

/// `tools/icongen/` → repo root. Derived from `CARGO_MANIFEST_DIR` rather
/// than the working directory, so the one-liner works from anywhere.
fn repo_root() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("cannot find repo root above {}", manifest.display()))
}
