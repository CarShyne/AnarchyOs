//! Turns everything in assets/ into data the kernel can use directly:
//! wallpapers and logos become raw ARGB pixels, fonts and icons become
//! anti-aliased coverage bitmaps. The kernel has no image or font decoder,
//! so all of that work happens here, on the build machine.

use image::imageops::FilterType;
use image::RgbaImage;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Wallpapers are pre-scaled to this size. The kernel rescales at boot if
/// the real screen turns out to be different.
const SCREEN_W: u32 = 1280;
const SCREEN_H: u32 = 800;
const THUMB_W: u32 = 192;
const THUMB_H: u32 = 120;

/// (constant name, font file, pixel size)
const FONTS: &[(&str, &str, f32)] = &[
    ("SMALL", "Inter-Regular.ttf", 12.0),
    ("BODY", "Inter-Regular.ttf", 14.0),
    ("LABEL", "Inter-Medium.ttf", 14.0),
    ("STRONG", "Inter-SemiBold.ttf", 14.0),
    ("TITLE", "Inter-SemiBold.ttf", 22.0),
    ("HERO", "Inter-Bold.ttf", 44.0),
    ("MONO", "JetBrainsMono-Regular.ttf", 14.0),
];

/// (constant name, codepoint in lucide.ttf)
const ICONS: &[(&str, u32)] = &[
    ("INFO", 57593),
    ("CLOCK", 57479),
    ("ETHERNET", 58912),
    ("WIFI", 57774),
    ("WIFI_OFF", 57775),
    ("BLUETOOTH", 57436),
    ("BLUETOOTH_OFF", 57785),
    ("PALETTE", 57821),
    ("IMAGE", 57590),
    ("DOCK", 58412),
    ("MONITOR", 57629),
    ("DRIVE", 57581),
    ("SETTINGS", 57684),
    ("TERMINAL", 57729),
    ("KEYBOARD", 57988),
    ("SPARKLES", 58386),
    ("FOLDER", 57559),
    ("ACTIVITY", 57400),
    ("GLOBE", 57576),
    ("NOTES", 58774),
    ("GRID", 57599),
    ("CPU", 57513),
    ("MEMORY", 58437),
    ("CHECK", 57452),
    ("CHEVRON", 57455),
    ("REFRESH", 57669),
    ("CALENDAR", 57443),
];

const ICON_SIZES: &[u32] = &[16, 26, 34];

struct Gen {
    out: PathBuf,
    code: String,
    files: u32,
}

impl Gen {
    fn blob(&mut self, bytes: &[u8]) -> String {
        let name = format!("asset_{}.bin", self.files);
        self.files += 1;
        fs::write(self.out.join(&name), bytes).expect("write asset blob");
        name
    }

    /// Emit an `Img { .. }` expression for an RGBA image.
    fn img(&mut self, im: &RgbaImage) -> String {
        let mut bytes = Vec::with_capacity((im.width() * im.height() * 4) as usize);
        for p in im.pixels() {
            let [r, g, b, a] = p.0;
            let argb = (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
            bytes.extend_from_slice(&argb.to_le_bytes());
        }
        let name = self.blob(&bytes);
        format!("Img {{ w: {}, h: {}, px: words!(\"{}\") }}", im.width(), im.height(), name)
    }
}

fn warn(msg: &str) {
    println!("cargo:warning={}", msg);
}

fn load(path: &Path) -> Option<RgbaImage> {
    match image::open(path) {
        Ok(im) => Some(im.to_rgba8()),
        Err(e) => {
            warn(&format!("could not read {}: {}", path.display(), e));
            None
        }
    }
}

/// Crop away fully transparent borders.
fn trim(im: &RgbaImage) -> RgbaImage {
    let (w, h) = im.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in im.enumerate_pixels() {
        if p.0[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return im.clone();
    }
    image::imageops::crop_imm(im, x0, y0, x1 - x0, y1 - y0).to_image()
}

fn fit_height(im: &RgbaImage, h: u32) -> RgbaImage {
    let w = (im.width() as u64 * h as u64 / im.height().max(1) as u64).max(1) as u32;
    image::imageops::resize(im, w, h, FilterType::Lanczos3)
}

/// Scale and centre-crop so the image fills w x h exactly.
fn cover(im: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (iw, ih) = im.dimensions();
    let scale = f64::max(w as f64 / iw as f64, h as f64 / ih as f64);
    let sw = ((iw as f64 * scale).ceil() as u32).max(w);
    let sh = ((ih as f64 * scale).ceil() as u32).max(h);
    let scaled = image::imageops::resize(im, sw, sh, FilterType::CatmullRom);
    let mut out = image::imageops::crop_imm(&scaled, (sw - w) / 2, (sh - h) / 2, w, h).to_image();
    for p in out.pixels_mut() {
        p.0[3] = 255;
    }
    out
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let direct = root.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let parent = root.parent()?.join(name);
    parent.is_file().then_some(parent)
}

fn pretty_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Wallpaper");
    stem.chars().filter(|c| c.is_ascii() && *c != '"' && *c != '\\').collect()
}

fn wallpapers(g: &mut Gen, root: &Path) {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [root.join("wallpaper"), root.join("wallpapers")] {
        println!("cargo:rerun-if-changed={}", dir.display());
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
                if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp") {
                    println!("cargo:rerun-if-changed={}", p.display());
                    files.push(p);
                }
            }
        }
    }
    files.sort();

    let mut items = Vec::new();
    for p in &files {
        if let Some(im) = load(p) {
            let full = cover(&im, SCREEN_W, SCREEN_H);
            let thumb = image::imageops::resize(&full, THUMB_W, THUMB_H, FilterType::Lanczos3);
            let full_e = g.img(&full);
            let thumb_e = g.img(&thumb);
            items.push(format!(
                "Wallpaper {{ name: \"{}\", full: {}, thumb: {} }}",
                pretty_name(p),
                full_e,
                thumb_e
            ));
        }
    }
    if items.is_empty() {
        warn("no wallpapers found in assets/wallpaper, the desktop will use a gradient");
    }
    writeln!(g.code, "pub static WALLPAPERS: [Wallpaper; {}] = [", items.len()).unwrap();
    for it in &items {
        writeln!(g.code, "    {},", it).unwrap();
    }
    writeln!(g.code, "];").unwrap();
}

fn logos(g: &mut Gen, root: &Path) {
    let blank = RgbaImage::new(1, 1);

    let mark = find_file(root, "App-Drawer.png").and_then(|p| {
        println!("cargo:rerun-if-changed={}", p.display());
        load(&p)
    });
    if mark.is_none() {
        warn("assets/App-Drawer.png not found, using a blank icon");
    }
    let mark = mark.map(|m| trim(&m)).unwrap_or_else(|| blank.clone());
    for (name, size) in [("MARK_22", 22u32), ("MARK_48", 48), ("MARK_64", 64)] {
        let im = if mark.width() > 1 { fit_height(&mark, size) } else { blank.clone() };
        let e = g.img(&im);
        writeln!(g.code, "pub static {}: Img = {};", name, e).unwrap();
    }

    let word = find_file(root, "AnarchyOs-logo.png").and_then(|p| {
        println!("cargo:rerun-if-changed={}", p.display());
        load(&p)
    });
    if word.is_none() {
        warn("assets/AnarchyOs-logo.png not found, using a blank logo");
    }
    let word = word.map(|m| trim(&m)).unwrap_or_else(|| blank.clone());
    let im = if word.width() > 1 { fit_height(&word, 96) } else { blank };
    let e = g.img(&im);
    writeln!(g.code, "pub static WORDMARK: Img = {};", e).unwrap();
}

fn read_font(root: &Path, file: &str) -> fontdue::Font {
    let path = root.join("fonts").join(file);
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("missing font {}: {}", path.display(), e));
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
        .unwrap_or_else(|e| panic!("bad font {}: {}", path.display(), e))
}

fn fonts(g: &mut Gen, root: &Path) {
    for (name, file, px) in FONTS {
        let font = read_font(root, file);
        let lm = font.horizontal_line_metrics(*px).expect("font has no line metrics");
        let ascent = lm.ascent.round() as i32;
        let height = (lm.ascent - lm.descent).ceil() as i32;

        let mut data: Vec<u8> = Vec::new();
        let mut glyphs = String::new();
        for c in 32u8..=126 {
            let (m, bitmap) = font.rasterize(c as char, *px);
            let off = data.len();
            data.extend_from_slice(&bitmap);
            // adv is in 1/16 pixel so runs of text keep their natural spacing.
            let adv = (m.advance_width * 16.0).round() as i32;
            let top = ascent - (m.ymin + m.height as i32);
            write!(
                glyphs,
                "Glyph {{ adv: {}, w: {}, h: {}, ox: {}, oy: {}, off: {} }}, ",
                adv, m.width, m.height, m.xmin, top, off
            )
            .unwrap();
        }
        let blob = g.blob(&data);
        writeln!(
            g.code,
            "pub static {}: Font = Font {{ height: {}, ascent: {}, glyphs: [{}], data: bytes!(\"{}\") }};",
            name, height, ascent, glyphs, blob
        )
        .unwrap();
    }
}

fn icons(g: &mut Gen, root: &Path) {
    let font = read_font(root, "lucide.ttf");
    writeln!(g.code, "pub mod icon {{").unwrap();
    for (i, (name, _)) in ICONS.iter().enumerate() {
        writeln!(g.code, "    pub const {}: usize = {};", name, i).unwrap();
    }
    writeln!(g.code, "}}").unwrap();

    for size in ICON_SIZES {
        let n = *size as usize;
        let mut entries = String::new();
        for (name, cp) in ICONS {
            let ch = char::from_u32(*cp).expect("bad icon codepoint");
            if font.lookup_glyph_index(ch) == 0 {
                warn(&format!("icon {} is missing from lucide.ttf", name));
            }
            let (m, bitmap) = font.rasterize(ch, *size as f32);
            let mut cell = vec![0u8; n * n];
            let ox = (n as i32 - m.width as i32) / 2;
            let oy = (n as i32 - m.height as i32) / 2;
            for y in 0..m.height {
                for x in 0..m.width {
                    let (tx, ty) = (x as i32 + ox, y as i32 + oy);
                    if tx >= 0 && ty >= 0 && (tx as usize) < n && (ty as usize) < n {
                        cell[ty as usize * n + tx as usize] = bitmap[y * m.width + x];
                    }
                }
            }
            let blob = g.blob(&cell);
            write!(entries, "Mask {{ size: {}, data: bytes!(\"{}\") }}, ", size, blob).unwrap();
        }
        writeln!(
            g.code,
            "pub static ICONS_{}: [Mask; {}] = [{}];",
            size,
            ICONS.len(),
            entries
        )
        .unwrap();
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = env::var("ANARCHY_ASSETS").map(PathBuf::from).unwrap_or_else(|_| manifest.join("assets"));
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ANARCHY_ASSETS");
    println!("cargo:rerun-if-changed={}", root.display());

    let mut g = Gen { out: out.clone(), code: String::new(), files: 0 };
    writeln!(g.code, "pub const WALLPAPER_W: u32 = {};", SCREEN_W).unwrap();
    writeln!(g.code, "pub const WALLPAPER_H: u32 = {};", SCREEN_H).unwrap();
    wallpapers(&mut g, &root);
    logos(&mut g, &root);
    fonts(&mut g, &root);
    icons(&mut g, &root);

    fs::write(out.join("assets_gen.rs"), &g.code).expect("write assets_gen.rs");
}
