//! Desktop background and top bar. The wallpaper gradient is computed once
//! into a row table, so each repaint is a plain fill instead of a blend.

use crate::framebuffer as fb;
use core::ptr::{addr_of, addr_of_mut};

const TOP_BAR_H: u32 = 28;
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

/// Wallpaper and dock. Drawn first, under every window.
pub fn draw_background() {
    let w = fb::width();
    let h = fb::height();
    let rows = unsafe { &*addr_of!(ROW_COLORS) };

    for y in 0..h.min(MAX_H as u32) {
        fb::fill_rect(0, y, w, 1, rows[y as usize]);
    }

    let dock_w = 400.min(w.saturating_sub(80));
    let dock_h = 72;
    let dock_x = (w - dock_w) / 2;
    let dock_y = h.saturating_sub(dock_h + 28);
    fb::fill_rounded_rect(dock_x, dock_y, dock_w, dock_h, 36, 0x0021_2A38);
    let accents = [0x005B_8CFF, 0x0070_D694, 0x00FF_B86B, 0x00F2_6D85, 0x00B5_8CFF];
    for (i, color) in accents.iter().enumerate() {
        let ix = dock_x + 40 + i as u32 * 64;
        fb::fill_rounded_rect(ix, dock_y + 16, 40, 40, 12, *color);
    }
}

/// Translucent top bar, drawn over the windows so they show through it.
pub fn draw_topbar() {
    let w = fb::width();
    fb::blend_rect(0, 0, w, TOP_BAR_H, 0x0030_3A4E, 150);
    fb::draw_text(14, 6, "AnarchyOS    Files    View    Window", 0x00E6_ECF5, 2);
    let label = "v0.1.0";
    let label_w = label.len() as u32 * 16;
    fb::draw_text(w.saturating_sub(14 + label_w), 6, label, 0x00E6_ECF5, 2);
}
