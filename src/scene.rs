//! The first look at the visual direction: gradient wallpaper, a
//! translucent top bar, a rounded panel, and a rounded dock. Built only
//! from the primitives in framebuffer.rs, so it also tests them.

use crate::console;
use crate::framebuffer as fb;

const TOP_BAR_H: u32 = 28;

pub fn draw() {
    console::disable();
    let w = fb::width();
    let h = fb::height();

    // Wallpaper: vertical gradient from deep navy to dark violet.
    for y in 0..h {
        let t = y * 255 / h.max(1);
        fb::fill_rect(0, y, w, 1, fb::mix(0x000A_0E18, 0x0018_1230, t));
    }

    // Main panel with a title and a call-to-action button.
    let pw = 720.min(w.saturating_sub(80));
    let ph = 340.min(h.saturating_sub(200));
    let px = (w - pw) / 2;
    let py = (h - ph) / 2 - 40;
    fb::fill_rounded_rect(px, py, pw, ph, 28, 0x0014_1A28);
    fb::draw_text(px + 48, py + 52, "AnarchyOS", 0x00E6_ECF5, 6);
    fb::draw_text(px + 48, py + 132, "A clean, fun kernel and desktop", 0x0095_A3B8, 2);
    fb::fill_rounded_rect(px + 48, py + ph - 88, 120, 40, 20, 0x005B_8CFF);
    fb::draw_text(px + 72, py + ph - 76, "Start", 0x00FF_FFFF, 2);

    // Dock: rounded pill with five icon tiles.
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

    // Top bar: translucent, so the wallpaper shows through it.
    fb::blend_rect(0, 0, w, TOP_BAR_H, 0x0030_3A4E, 150);
    fb::draw_text(14, 6, "AnarchyOS    Files    View    Window", 0x00E6_ECF5, 2);
    let label = "v0.1.0";
    let label_w = label.len() as u32 * 16;
    fb::draw_text(w.saturating_sub(14 + label_w), 6, label, 0x00E6_ECF5, 2);
}
