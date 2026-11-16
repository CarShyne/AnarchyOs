//! The desktop background: one of the built-in wallpapers, or a gradient.
//!
//! Two screen-sized pictures are kept ready: the wallpaper itself, and a
//! blurred copy that translucent windows use as frosted glass. Blurring
//! once here means windows cost no more to draw than a flat colour.

use crate::framebuffer as fb;
use crate::gfx;
use crate::image::{Img, WALLPAPERS};
use crate::theme;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

pub const GRADIENT_TOP: u32 = 0x0008_090C;
const BLUR_RADIUS: usize = 14;

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

    match current() {
        Some(img) if (img.w as usize, img.h as usize) == (w, h) => {}
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

    let mut frost = Vec::new();
    frost.extend_from_slice(wallpaper_pixels(scene(), w, h));
    box_blur(&mut frost, w, h, BLUR_RADIUS);
    box_blur(&mut frost, w, h, BLUR_RADIUS);
    scene().frost = frost;
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
