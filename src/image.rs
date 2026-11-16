//! Images, fonts and icons baked into the kernel by build.rs.
//!
//! Nothing here is decoded at run time. Pixels are stored as ARGB words,
//! and fonts and icons as 8-bit coverage, ready for the drawing code in
//! gfx.rs to blend.

/// An ARGB image. Alpha is 255 for wallpapers and varies for logos.
pub struct Img {
    pub w: u32,
    pub h: u32,
    pub px: &'static [u32],
}

pub struct Wallpaper {
    pub name: &'static str,
    pub full: Img,
    pub thumb: Img,
}

pub struct Glyph {
    /// Advance to the next glyph, in 1/16 pixel.
    pub adv: u16,
    pub w: u8,
    pub h: u8,
    pub ox: i8,
    /// Rows from the top of the line to the top of the bitmap.
    pub oy: i8,
    pub off: u32,
}

/// Printable ASCII, rasterised at one size.
pub struct Font {
    pub height: i32,
    pub ascent: i32,
    pub glyphs: [Glyph; 95],
    pub data: &'static [u8],
}

/// A square single-colour icon, stored as coverage so it can be tinted.
pub struct Mask {
    pub size: u32,
    pub data: &'static [u8],
}

/// Forces 4-byte alignment on embedded bytes so they can be read as words.
#[repr(C)]
pub struct Align4<T: ?Sized> {
    pub _align: [u32; 0],
    pub bytes: T,
}

macro_rules! bytes {
    ($file:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $file))
    };
}

macro_rules! words {
    ($file:literal) => {{
        static ALIGNED: &Align4<[u8]> = &Align4 {
            _align: [],
            bytes: *include_bytes!(concat!(env!("OUT_DIR"), "/", $file)),
        };
        // SAFETY: the wrapper is 4-byte aligned and build.rs writes whole
        // little-endian u32 words, which is the native order on x86_64.
        unsafe {
            core::slice::from_raw_parts(ALIGNED.bytes.as_ptr() as *const u32, ALIGNED.bytes.len() / 4)
        }
    }};
}

include!(concat!(env!("OUT_DIR"), "/assets_gen.rs"));

pub fn icon16(i: usize) -> &'static Mask {
    &ICONS_16[i]
}

pub fn icon26(i: usize) -> &'static Mask {
    &ICONS_26[i]
}

pub fn icon34(i: usize) -> &'static Mask {
    &ICONS_34[i]
}
