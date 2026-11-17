//! The desktop background: one of the built-in wallpapers, or a gradient.
//!
//! Two screen-sized pictures are kept ready: the wallpaper itself, and a
//! blurred copy that translucent windows use as frosted glass. Blurring
//! once here means windows cost no more to draw than a flat colour. Wallpaper
//! mods (scanlines, vignette, grain, tint) are also baked in here, so they
//! add nothing to the cost of drawing each frame.

use crate::framebuffer as fb;
use crate::gfx;
use crate::image::{Img, WALLPAPERS};
use crate::theme;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

pub const GRADIENT_TOP: u32 = 0x0008_090C;

/// The gradient fades from near-black into a dark shade of the accent.
pub fn gradient_bottom() -> u32 {
    gfx::mix(0x0010_1014, theme::accent(), 70)
}

struct Scene {
    /// The wallpaper at screen size. Empty when the built-in image already
    /// matches the screen and can be used in place.
    base: Vec<u32>,
    /// Blurred wallpaper at screen size.
    frost: Vec<u32>,
    scaled: bool,
}

static mut SCENE: Scene = Scene { base: Vec::new(), frost: Vec::new(), scaled: false };

// Only the main loop touches the scene.
fn scene() -> &'static mut Scene {
    unsafe { &mut *addr_of_mut!(SCENE) }
}

fn current() -> Option<&'static Img> {
    WALLPAPERS.get(theme::cfg().wallpaper).map(|w| &w.full)
}

pub fn is_scaled() -> bool {
    scene().scaled
}

/// Get the chosen wallpaper ready. Call at start-up and after a change.
pub fn apply() {
    let s = scene();
    let (w, h) = (fb::width() as usize, fb::height() as usize);
    if w == 0 || h == 0 {
        return;
    }
    // Drop the old pictures first so two copies never sit in the heap at once.
    s.base = Vec::new();
    s.frost = Vec::new();
    s.scaled = false;

    let mods = theme::wallpaper_mods();
    match current() {
        Some(img) if (img.w as usize, img.h as usize) == (w, h) => {
            // A mod needs its own copy to change, so the built-in picture is left alone.
            if mods {
                s.base = img.px.to_vec();
            }
        }
        Some(img) => {
            s.base = cover(img, w, h);
            s.scaled = true;
        }
        None => {
            let (top, bottom) = (GRADIENT_TOP, gradient_bottom());
            let mut px = Vec::with_capacity(w * h);
            for y in 0..h {
                let c = gfx::mix(top, bottom, (y * 255 / (h - 1).max(1)) as u32);
                px.resize(px.len() + w, c);
            }
            s.base = px;
        }
    }

    if mods {
        mod_pass(&mut scene().base, w, h);
    }

    let radius = theme::blur_radius();
    let mut frost = Vec::new();
    frost.extend_from_slice(wallpaper_pixels(scene(), w, h));
    if radius > 0 {
        box_blur(&mut frost, w, h, radius);
        box_blur(&mut frost, w, h, radius);
    }
    scene().frost = frost;
}

/// Apply the wallpaper mods in place. Each one is a multiplier per pixel,
/// so the whole pass is one walk over the picture.
fn mod_pass(px: &mut [u32], w: usize, h: usize) {
    if px.len() != w * h {
        return;
    }
    let c = theme::cfg();
    let (scanlines, vignette, grain) = (c.scanlines, c.vignette, c.grain);
    let wash = if c.desk_tint > 0 { theme::accent() } else { 0 };
    let wash_t = c.desk_tint.min(100) * 255 / 100;

    let (cx, cy) = (w as i64 / 2, h as i64 / 2);
    let max_d2 = (cx * cx + cy * cy).max(1);
    // A tiny xorshift generator. The grain only needs to look random.
    let mut seed: u32 = 0x2545_F491;

    for y in 0..h {
        let dy = y as i64 - cy;
        for x in 0..w {
            let i = y * w + x;
            let mut f: u32 = 256;
            if scanlines && y % 3 == 2 {
                f = 190;
            }
            if vignette {
                let dx = x as i64 - cx;
                let t = ((dx * dx + dy * dy) * 256 / max_d2).min(256) as u32;
                // The corners lose up to 110 of 256 brightness.
                let darken = t * t / 256 * 110 / 256;
                f = f * (256 - darken) / 256;
            }

            let p = px[i];
            let (mut r, mut g, mut b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
            r = r * f / 256;
            g = g * f / 256;
            b = b * f / 256;

            if wash != 0 {
                let m = gfx::mix(
                    (r << 16) | (g << 8) | b,
                    wash,
                    wash_t,
                );
                r = (m >> 16) & 0xFF;
                g = (m >> 8) & 0xFF;
                b = m & 0xFF;
            }

            if grain {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let n = (seed % 13) as i32 - 6;
                r = (r as i32 + n).clamp(0, 255) as u32;
                g = (g as i32 + n).clamp(0, 255) as u32;
                b = (b as i32 + n).clamp(0, 255) as u32;
            }

            px[i] = (r << 16) | (g << 8) | b;
        }
    }
}

fn wallpaper_pixels(s: &'static Scene, w: usize, h: usize) -> &'static [u32] {
    if s.base.len() == w * h {
        &s.base
    } else {
        match current() {
            Some(img) if img.px.len() == w * h => img.px,
            _ => &[],
        }
    }
}

/// Resize to fill w x h, cropping whichever side overflows.
fn cover(img: &Img, w: usize, h: usize) -> Vec<u32> {
    let (iw, ih) = (img.w as usize, img.h as usize);
    let by_width = w * ih >= h * iw;
    let (num, den) = if by_width { (iw, w) } else { (ih, h) };
    let off_x = iw.saturating_sub(w * num / den) / 2;
    let off_y = ih.saturating_sub(h * num / den) / 2;
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = (off_y + y * num / den).min(ih - 1);
        for x in 0..w {
            let sx = (off_x + x * num / den).min(iw - 1);
            px.push(img.px[sy * iw + sx]);
        }
    }
    px
}

/// Average each pixel with its neighbours within `r`, across then down.
/// Uses a running sum, so the cost does not depend on the radius.
fn box_blur(px: &mut [u32], w: usize, h: usize, r: usize) {
    if px.len() != w * h {
        return;
    }
    let mut line = vec![0u32; w.max(h)];
    for y in 0..h {
        line[..w].copy_from_slice(&px[y * w..(y + 1) * w]);
        blur_line(&line[..w], r, |x, c| px[y * w + x] = c);
    }
    for x in 0..w {
        for y in 0..h {
            line[y] = px[y * w + x];
        }
        blur_line(&line[..h], r, |y, c| px[y * w + x] = c);
    }
}

fn blur_line(src: &[u32], r: usize, mut put: impl FnMut(usize, u32)) {
    let n = src.len();
    let at = |i: isize| src[i.clamp(0, n as isize - 1) as usize];
    let window = (2 * r + 1) as u32;
    let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
    for i in -(r as isize)..=(r as isize) {
        let c = at(i);
        sr += (c >> 16) & 0xFF;
        sg += (c >> 8) & 0xFF;
        sb += c & 0xFF;
    }
    for i in 0..n {
        put(i, (sr / window) << 16 | (sg / window) << 8 | (sb / window));
        let add = at(i as isize + r as isize + 1);
        let sub = at(i as isize - r as isize);
        sr = sr + ((add >> 16) & 0xFF) - ((sub >> 16) & 0xFF);
        sg = sg + ((add >> 8) & 0xFF) - ((sub >> 8) & 0xFF);
        sb = sb + (add & 0xFF) - (sub & 0xFF);
    }
}

/// The blurred wallpaper, one pixel per screen pixel. Empty before `apply`.
pub fn frost() -> &'static [u32] {
    &scene().frost
}

pub fn draw_background() {
    let (w, h) = (fb::width() as i32, fb::height() as i32);
    let px = wallpaper_pixels(scene(), w as usize, h as usize);
    if px.is_empty() {
        gfx::rect(0, 0, w, h, GRADIENT_TOP);
    } else {
        gfx::pixels(0, 0, w, h, px);
    }
}
