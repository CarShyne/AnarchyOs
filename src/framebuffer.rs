//! Pixel drawing on the framebuffer Limine gives us. Colors are 0x00RRGGBB,
//! the layout Limine uses for 32 bpp modes.

use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};
use font8x8::legacy::BASIC_LEGACY;

static ADDR: AtomicU64 = AtomicU64::new(0);
static WIDTH: AtomicU64 = AtomicU64::new(0);
static HEIGHT: AtomicU64 = AtomicU64::new(0);
static PITCH: AtomicU64 = AtomicU64::new(0);

pub fn init(addr: u64, width: u64, height: u64, pitch: u64) {
    ADDR.store(addr, Ordering::Relaxed);
    WIDTH.store(width, Ordering::Relaxed);
    HEIGHT.store(height, Ordering::Relaxed);
    PITCH.store(pitch, Ordering::Relaxed);
}

pub fn width() -> u32 {
    WIDTH.load(Ordering::Relaxed) as u32
}

pub fn height() -> u32 {
    HEIGHT.load(Ordering::Relaxed) as u32
}

fn pixel_addr(x: u32, y: u32) -> u64 {
    ADDR.load(Ordering::Relaxed)
        + y as u64 * PITCH.load(Ordering::Relaxed)
        + x as u64 * 4
}

pub fn put_pixel(x: u32, y: u32, color: u32) {
    if x >= width() || y >= height() {
        return;
    }
    unsafe { ptr::write_volatile(pixel_addr(x, y) as *mut u32, color) };
}

pub fn get_pixel(x: u32, y: u32) -> u32 {
    unsafe { ptr::read_volatile(pixel_addr(x, y) as *const u32) }
}

/// Blend `fg` over `bg`. `alpha` runs from 0 (only bg) to 255 (only fg).
pub fn mix(bg: u32, fg: u32, alpha: u32) -> u32 {
    let inv = 255 - alpha;
    let ch = |shift: u32| {
        let f = (fg >> shift) & 0xff;
        let b = (bg >> shift) & 0xff;
        ((f * alpha + b * inv) / 255) << shift
    };
    ch(16) | ch(8) | ch(0)
}

pub fn fill_rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    let x_end = x.saturating_add(w).min(width());
    let y_end = y.saturating_add(h).min(height());
    for py in y..y_end {
        for px in x..x_end {
            put_pixel(px, py, color);
        }
    }
}

pub fn blend_rect(x: u32, y: u32, w: u32, h: u32, color: u32, alpha: u32) {
    let x_end = x.saturating_add(w).min(width());
    let y_end = y.saturating_add(h).min(height());
    for py in y..y_end {
        for px in x..x_end {
            let bg = get_pixel(px, py);
            put_pixel(px, py, mix(bg, color, alpha));
        }
    }
}

pub fn clear(color: u32) {
    fill_rect(0, 0, width(), height(), color);
}

/// Integer square root, plenty accurate for corner radii.
fn isqrt(n: u32) -> u32 {
    let mut x = 0;
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    x
}

/// How far a corner row is pulled in from the rectangle's edge. Row 0 is
/// the top row of a corner of radius r.
fn corner_inset(r: u32, row: u32) -> u32 {
    let d = r - row;
    r - isqrt(r * r - d * d)
}

pub fn fill_rounded_rect(x: u32, y: u32, w: u32, h: u32, r: u32, color: u32) {
    let r = r.min(w / 2).min(h / 2);
    for row in 0..h {
        let inset = if row < r {
            corner_inset(r, row)
        } else if row >= h - r {
            corner_inset(r, h - 1 - row)
        } else {
            0
        };
        fill_rect(x + inset, y + row, w - 2 * inset, 1, color);
    }
}

pub fn draw_char(x: u32, y: u32, c: u8, color: u32, scale: u32) {
    let glyph = if (c as usize) < BASIC_LEGACY.len() {
        BASIC_LEGACY[c as usize]
    } else {
        BASIC_LEGACY[b'?' as usize]
    };
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..8u32 {
            if (*bits >> col) & 1 == 1 {
                fill_rect(x + col * scale, y + row as u32 * scale, scale, scale, color);
            }
        }
    }
}

pub fn draw_text(x: u32, y: u32, text: &str, color: u32, scale: u32) {
    let mut cx = x;
    for b in text.bytes() {
        draw_char(cx, y, b, color, scale);
        cx += 8 * scale;
    }
}

/// Move everything up by `rows` pixel rows, then clear the freed bottom rows.
pub fn scroll_up(rows: u32, color: u32) {
    let h = height();
    let rows = rows.min(h);
    let pitch = PITCH.load(Ordering::Relaxed);
    let base = ADDR.load(Ordering::Relaxed);
    unsafe {
        ptr::copy(
            (base + rows as u64 * pitch) as *const u8,
            base as *mut u8,
            ((h - rows) as u64 * pitch) as usize,
        );
    }
    fill_rect(0, h - rows, width(), rows, color);
}
