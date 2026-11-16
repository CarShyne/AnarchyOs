//! The desktop background and top bar. These are drawn under and over the
//! windows by the window manager, which clips them to the damaged area.

use crate::framebuffer as fb;

const TOP_BAR_H: u32 = 28;

/// Wallpaper gradient and dock. Drawn first, under every window.
pub fn draw_background() {
    let w = fb::width();
    let h = fb::height();

    for y in 0..h {
        let t = y * 255 / h.max(1);
        fb::fill_rect(0, y, w, 1, fb::mix(0x000A_0E18, 0x0018_1230, t));
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

/// Translucent top bar. Drawn last, over the windows, so the windows show
/// through it.
pub fn draw_topbar() {
    let w = fb::width();
    fb::blend_rect(0, 0, w, TOP_BAR_H, 0x0030_3A4E, 150);
    fb::draw_text(14, 6, "AnarchyOS    Files    View    Window", 0x00E6_ECF5, 2);
    let label = "v0.1.0";
    let label_w = label.len() as u32 * 16;
    fb::draw_text(w.saturating_sub(14 + label_w), 6, label, 0x00E6_ECF5, 2);
}
