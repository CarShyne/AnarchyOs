//! Wallpaper. The gradient is computed once into a row table, so each
//! repaint is a plain fill. The top bar, dock and windows are drawn by the
//! window manager on top of this.

use crate::framebuffer as fb;
use core::ptr::{addr_of, addr_of_mut};

const MAX_H: usize = 1200;

static mut ROW_COLORS: [u32; MAX_H] = [0; MAX_H];

/// Compute the gradient once. Call after the framebuffer is initialized.
pub fn init() {
    let h = fb::height().min(MAX_H as u32);
    let rows = unsafe { &mut *addr_of_mut!(ROW_COLORS) };
    for y in 0..h {
        let t = y * 255 / h.max(1);
        rows[y as usize] = fb::mix(0x000A_0E18, 0x0018_1230, t);
    }
}

/// Wallpaper. Drawn first, under everything else.
pub fn draw_background() {
    let w = fb::width();
    let rows = unsafe { &*addr_of!(ROW_COLORS) };
    for y in 0..fb::height().min(MAX_H as u32) {
        fb::fill_rect(0, y, w, 1, rows[y as usize]);
    }
}
