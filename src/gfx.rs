//! The drawing layer the desktop is built on: anti-aliased rounded shapes,
//! alpha blending, images, tinted icons and real fonts.
//!
//! Positions and sizes are i32 so shapes may hang off any edge of the
//! screen. Everything is clipped to the rectangle set in framebuffer.rs.
//! Alpha arguments run from 0 (invisible) to 255 (solid).

use crate::framebuffer as fb;
use crate::image::{Font, Img, Mask};
use core::ptr::addr_of_mut;

struct Surf {
    ptr: *mut u32,
    stride: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

fn surf() -> Surf {
    let (ptr, stride) = fb::surface();
    let (x0, y0, x1, y1) = fb::current_clip();
    Surf { ptr, stride, x0: x0 as i32, y0: y0 as i32, x1: x1 as i32, y1: y1 as i32 }
}

impl Surf {
    /// Pointer to pixel (x, y). The caller has already clipped.
    #[inline]
    unsafe fn at(&self, x: i32, y: i32) -> *mut u32 {
        self.ptr.add(y as usize * self.stride + x as usize)
    }

    #[inline]
    fn row_visible(&self, y: i32) -> bool {
        y >= self.y0 && y < self.y1
    }

    /// Fill pixels [xa, xb) on row y. `a` is 0..=256.
    #[inline]
    fn span(&self, y: i32, xa: i32, xb: i32, color: u32, a: u32) {
        if !self.row_visible(y) || a == 0 {
            return;
        }
        let xa = xa.max(self.x0);
        let xb = xb.min(self.x1);
        if xb <= xa {
            return;
        }
        // SAFETY: xa..xb and y are inside the clip, which is inside the buffer.
        unsafe {
            let row = core::slice::from_raw_parts_mut(self.at(xa, y), (xb - xa) as usize);
            if a >= 256 {
                row.fill(color);
            } else {
                for p in row.iter_mut() {
                    *p = blend(*p, color, a);
                }
            }
        }
    }

    #[inline]
    fn dot(&self, x: i32, y: i32, color: u32, a: u32) {
        if a == 0 || x < self.x0 || x >= self.x1 || y < self.y0 || y >= self.y1 {
            return;
        }
        // SAFETY: (x, y) is inside the clip.
        unsafe {
            let p = self.at(x, y);
            *p = if a >= 256 { color } else { blend(*p, color, a) };
        }
    }
}

/// Mix `fg` over `bg`. `a` runs from 0 (all bg) to 256 (all fg).
#[inline]
fn blend(bg: u32, fg: u32, a: u32) -> u32 {
    let ia = 256 - a;
    let rb = (((fg & 0x00FF_00FF) * a + (bg & 0x00FF_00FF) * ia) >> 8) & 0x00FF_00FF;
    let g = (((fg & 0x0000_FF00) * a + (bg & 0x0000_FF00) * ia) >> 8) & 0x0000_FF00;
    rb | g
}

/// Public 0..=255 alpha to the internal 0..=256 range.
#[inline]
fn a256(alpha: u32) -> u32 {
    let a = alpha.min(255);
    a + (a >> 7)
}

/// Blend two colours. `t` is 0 (all `a`) to 255 (all `b`).
pub fn mix(a: u32, b: u32, t: u32) -> u32 {
    blend(a, b, a256(t))
}

pub fn rect(x: i32, y: i32, w: i32, h: i32, color: u32) {
    rect_a(x, y, w, h, color, 255);
}

pub fn rect_a(x: i32, y: i32, w: i32, h: i32, color: u32, alpha: u32) {
    let s = surf();
    let a = a256(alpha);
    for row in y.max(s.y0)..(y + h).min(s.y1) {
        s.span(row, x, x + w, color, a);
    }
}

// ---- Rounded corners -----------------------------------------------------

const MAX_R: usize = 40;

/// Corner coverage per radius: how much of each pixel in the r x r corner
/// box lies inside the quarter circle. Filled in the first time a radius is
/// used, then reused for every shape with that radius.
static mut COV: [[u8; MAX_R * MAX_R]; MAX_R + 1] = [[0; MAX_R * MAX_R]; MAX_R + 1];
static mut COV_READY: [bool; MAX_R + 1] = [false; MAX_R + 1];

fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

fn cov(r: usize) -> &'static [u8; MAX_R * MAX_R] {
    // SAFETY: the desktop draws from one thread only.
    unsafe {
        let ready = &mut *addr_of_mut!(COV_READY);
        let table = &mut (*addr_of_mut!(COV))[r];
        if !ready[r] {
            for row in 0..r {
                for col in 0..r {
                    // Distances are doubled so pixel centres stay integers.
                    let dx = (2 * r - (2 * col + 1)) as u64;
                    let dy = (2 * r - (2 * row + 1)) as u64;
                    let dist = isqrt((dx * dx + dy * dy) << 14) as i64;
                    let c = (r as i64 * 256 + 128 - dist).clamp(0, 256);
                    table[row * r + col] = (c * 255 / 256) as u8;
                }
            }
            ready[r] = true;
        }
        table
    }
}

fn clamp_radius(r: i32, w: i32, h: i32) -> i32 {
    r.min(w / 2).min(h / 2).min(MAX_R as i32).max(0)
}

fn round_impl(x: i32, y: i32, w: i32, h: i32, r: i32, alpha: u32, color: impl Fn(i32) -> u32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let s = surf();
    let a = a256(alpha);
    let r = clamp_radius(r, w, h);
    let table = cov(r as usize);
    for row in 0..h {
        let py = y + row;
        if !s.row_visible(py) {
            continue;
        }
        let c = color(row);
        let corner = if row < r {
            Some(row)
        } else if row >= h - r {
            Some(h - 1 - row)
        } else {
            None
        };
        match corner {
            None => s.span(py, x, x + w, c, a),
            Some(cr) => {
                for col in 0..r {
                    let cv = table[(cr * r + col) as usize] as u32;
                    if cv != 0 {
                        let pa = a * (cv + (cv >> 7)) >> 8;
                        s.dot(x + col, py, c, pa);
                        s.dot(x + w - 1 - col, py, c, pa);
                    }
                }
                s.span(py, x + r, x + w - r, c, a);
            }
        }
    }
}

/// Solid rounded rectangle with smooth corners.
pub fn round(x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
    round_impl(x, y, w, h, r, 255, |_| color);
}

/// Translucent rounded rectangle.
pub fn round_a(x: i32, y: i32, w: i32, h: i32, r: i32, color: u32, alpha: u32) {
    round_impl(x, y, w, h, r, alpha, |_| color);
}

/// Rounded rectangle shaded from `top` to `bottom`.
pub fn round_grad(x: i32, y: i32, w: i32, h: i32, r: i32, top: u32, bottom: u32, alpha: u32) {
    let span = (h - 1).max(1);
    round_impl(x, y, w, h, r, alpha, |row| mix(top, bottom, (row * 255 / span) as u32));
}

/// Frosted glass: a rounded rectangle that shows `backdrop` (a screen-sized
/// picture, `stride` pixels per row) tinted with `color`, instead of
/// whatever was drawn underneath. Falls back to a plain tint if the
/// backdrop is missing.
pub fn round_frost(x: i32, y: i32, w: i32, h: i32, r: i32, color: u32, alpha: u32, backdrop: &[u32], stride: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let s = surf();
    if stride <= 0 || backdrop.len() < (stride * s.y1.max(0)) as usize || s.x1 > stride {
        round_a(x, y, w, h, r, color, alpha);
        return;
    }
    let a = a256(alpha);
    let r = clamp_radius(r, w, h);
    let table = cov(r as usize);
    let glass = |px: i32, py: i32| blend(backdrop[(py * stride + px) as usize], color, a);
    for row in 0..h {
        let py = y + row;
        if !s.row_visible(py) {
            continue;
        }
        let cr = if row < r {
            row
        } else if row >= h - r {
            h - 1 - row
        } else {
            -1
        };
        let inset = if cr >= 0 { r } else { 0 };
        if cr >= 0 {
            for col in 0..r {
                let cv = table[(cr * r + col) as usize] as u32;
                if cv != 0 {
                    for px in [x + col, x + w - 1 - col] {
                        if px >= s.x0 && px < s.x1 {
                            s.dot(px, py, glass(px, py), cv + (cv >> 7));
                        }
                    }
                }
            }
        }
        let xa = (x + inset).max(s.x0);
        let xb = (x + w - inset).min(s.x1);
        if xb > xa {
            // SAFETY: xa..xb and py are inside the clip, and the backdrop
            // was checked to cover every clipped row.
            unsafe {
                let dst = core::slice::from_raw_parts_mut(s.at(xa, py), (xb - xa) as usize);
                let src = &backdrop[(py * stride + xa) as usize..(py * stride + xb) as usize];
                for (d, b) in dst.iter_mut().zip(src) {
                    *d = blend(*b, color, a);
                }
            }
        }
    }
}

/// One-pixel rounded outline.
pub fn stroke(x: i32, y: i32, w: i32, h: i32, r: i32, color: u32, alpha: u32) {
    if w <= 1 || h <= 1 {
        return;
    }
    let s = surf();
    let a = a256(alpha);
    let r = clamp_radius(r, w, h);
    let outer = cov(r as usize);
    let ri = (r - 1).max(0);
    let inner = cov(ri as usize);
    for row in 0..h {
        let py = y + row;
        if !s.row_visible(py) {
            continue;
        }
        let corner = if row < r {
            Some(row)
        } else if row >= h - r {
            Some(h - 1 - row)
        } else {
            None
        };
        match corner {
            None => {
                if row == 0 || row == h - 1 {
                    s.span(py, x, x + w, color, a);
                } else {
                    s.dot(x, py, color, a);
                    s.dot(x + w - 1, py, color, a);
                }
            }
            Some(cr) => {
                for col in 0..r {
                    let o = outer[(cr * r + col) as usize] as u32;
                    let i = if cr >= 1 && col >= 1 {
                        inner[((cr - 1) * ri + (col - 1)) as usize] as u32
                    } else {
                        0
                    };
                    let cv = o.saturating_sub(i);
                    if cv != 0 {
                        let pa = a * (cv + (cv >> 7)) >> 8;
                        s.dot(x + col, py, color, pa);
                        s.dot(x + w - 1 - col, py, color, pa);
                    }
                }
                if cr == 0 {
                    s.span(py, x + r, x + w - r, color, a);
                }
            }
        }
    }
}

/// Soft glow around a rounded rectangle, drawn as fading outlines.
pub fn shadow(x: i32, y: i32, w: i32, h: i32, r: i32, strength: u32) {
    const LAYERS: i32 = 12;
    for k in 1..=LAYERS {
        let f = (LAYERS + 1 - k) as u32;
        let alpha = strength * f * f / ((LAYERS * LAYERS) as u32);
        stroke(x - k, y - k, w + 2 * k, h + 2 * k, r + k, 0x0000_0000, alpha);
    }
}

// ---- Images ----------------------------------------------------------------

/// Copy a w x h block of opaque pixels.
pub fn pixels(x: i32, y: i32, w: i32, h: i32, px: &[u32]) {
    let s = surf();
    let xa = x.max(s.x0);
    let xb = (x + w).min(s.x1);
    if xb <= xa || px.len() < (w * h) as usize {
        return;
    }
    for py in y.max(s.y0)..(y + h).min(s.y1) {
        let src = ((py - y) * w + (xa - x)) as usize;
        let n = (xb - xa) as usize;
        // SAFETY: both ranges were clipped to their buffers above.
        unsafe {
            core::ptr::copy_nonoverlapping(px.as_ptr().add(src), s.at(xa, py), n);
        }
    }
}

/// Draw an image using its own alpha channel, faded by `alpha`.
pub fn image_a(x: i32, y: i32, img: &Img, alpha: u32) {
    let s = surf();
    let (w, h) = (img.w as i32, img.h as i32);
    let ga = a256(alpha);
    for py in y.max(s.y0)..(y + h).min(s.y1) {
        for px in x.max(s.x0)..(x + w).min(s.x1) {
            let p = img.px[((py - y) * w + (px - x)) as usize];
            let pa = p >> 24;
            if pa != 0 {
                s.dot(px, py, p & 0x00FF_FFFF, (pa + (pa >> 7)) * ga >> 8);
            }
        }
    }
}

/// Draw an opaque image with rounded corners.
pub fn image_round(x: i32, y: i32, img: &Img, r: i32) {
    let s = surf();
    let (w, h) = (img.w as i32, img.h as i32);
    let r = clamp_radius(r, w, h);
    let table = cov(r as usize);
    for row in 0..h {
        let py = y + row;
        if !s.row_visible(py) {
            continue;
        }
        let cr = if row < r {
            row
        } else if row >= h - r {
            h - 1 - row
        } else {
            -1
        };
        for col in 0..w {
            let cc = if col < r {
                col
            } else if col >= w - r {
                w - 1 - col
            } else {
                -1
            };
            let cv = if cr >= 0 && cc >= 0 { table[(cr * r + cc) as usize] as u32 } else { 255 };
            if cv != 0 {
                let p = img.px[(row * w + col) as usize] & 0x00FF_FFFF;
                s.dot(x + col, py, p, cv + (cv >> 7));
            }
        }
    }
}

/// Draw a single-colour icon.
pub fn icon(x: i32, y: i32, m: &Mask, color: u32) {
    icon_a(x, y, m, color, 255);
}

pub fn icon_a(x: i32, y: i32, m: &Mask, color: u32, alpha: u32) {
    let s = surf();
    let n = m.size as i32;
    let ga = a256(alpha);
    for py in y.max(s.y0)..(y + n).min(s.y1) {
        for px in x.max(s.x0)..(x + n).min(s.x1) {
            let cv = m.data[((py - y) * n + (px - x)) as usize] as u32;
            if cv != 0 {
                s.dot(px, py, color, (cv + (cv >> 7)) * ga >> 8);
            }
        }
    }
}

// ---- Text ------------------------------------------------------------------

fn glyph_index(b: u8) -> usize {
    if (32..=126).contains(&b) {
        (b - 32) as usize
    } else {
        (b'?' - 32) as usize
    }
}

/// Width of `text` in pixels.
pub fn text_w(font: &Font, text: &str) -> i32 {
    let mut pen = 0i32;
    for b in text.bytes() {
        pen += font.glyphs[glyph_index(b)].adv as i32;
    }
    (pen + 8) >> 4
}

/// Draw `text` with its top-left corner at (x, y). Returns the x position
/// just past the last glyph.
pub fn text(x: i32, y: i32, font: &Font, text: &str, color: u32) -> i32 {
    text_a(x, y, font, text, color, 255)
}

pub fn text_a(x: i32, y: i32, font: &Font, text: &str, color: u32, alpha: u32) -> i32 {
    let s = surf();
    let ga = a256(alpha);
    let mut pen = x * 16;
    // Skip the whole line if it is outside the clip rows.
    let visible = y < s.y1 && y + font.height > s.y0;
    for b in text.bytes() {
        let g = &font.glyphs[glyph_index(b)];
        if visible && g.w != 0 {
            let gx = ((pen + 8) >> 4) + g.ox as i32;
            let gy = y + g.oy as i32;
            let (gw, gh) = (g.w as i32, g.h as i32);
            if gx < s.x1 && gx + gw > s.x0 {
                for row in (gy.max(s.y0) - gy)..((gy + gh).min(s.y1) - gy) {
                    let base = g.off as usize + (row * gw) as usize;
                    for col in 0..gw {
                        let cv = font.data[base + col as usize] as u32;
                        if cv != 0 {
                            s.dot(gx + col, gy + row, color, (cv + (cv >> 7)) * ga >> 8);
                        }
                    }
                }
            }
        }
        pen += g.adv as i32;
    }
    (pen + 8) >> 4
}

/// Draw `text` so it ends at `right`.
pub fn text_right(right: i32, y: i32, font: &Font, s: &str, color: u32) {
    text(right - text_w(font, s), y, font, s, color);
}

/// Draw `text` centred on `cx`.
pub fn text_center(cx: i32, y: i32, font: &Font, s: &str, color: u32) {
    text(cx - text_w(font, s) / 2, y, font, s, color);
}
