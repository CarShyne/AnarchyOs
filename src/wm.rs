//! Window manager. A translucent top bar lists open windows and shows the
//! clock. A translucent dock at the bottom opens and closes apps. Opacity
//! for both comes from sliders in the Settings window. Any change repaints
//! the damaged rectangle from the wallpaper up, so nothing leaves trails.

use crate::framebuffer as fb;
use crate::pit;
use crate::ps2;
use crate::scene;
use crate::terminal;
use alloc::format;
use core::ptr::addr_of_mut;

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn union(self, o: Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    fn contains(self, px: u32, py: u32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    fn overlaps(self, o: Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Welcome,
    Keys,
    Terminal,
    Settings,
}

#[derive(Clone, Copy)]
struct Window {
    rect: Rect,
    title: &'static str,
    kind: Kind,
    open: bool,
}

const MAX_WINDOWS: usize = 4;
const SLIDERS: usize = 2;
const TITLE_H: u32 = 30;
const RADIUS: u32 = 14;
const MIN_W: u32 = 260;
const MIN_H: u32 = 160;
const GRIP: u32 = 18;

const TOP_H: u32 = 40;
const BTN_X0: u32 = 170;
const BTN_Y: u32 = 6;
const BTN_W: u32 = 140;
const BTN_H: u32 = 28;
const BTN_GAP: u32 = 8;

const DOCK_H: u32 = 64;
const DOCK_PAD: u32 = 14;
const DOCK_BOTTOM_GAP: u32 = 16;
const TILE: u32 = 44;
const TILE_GAP: u32 = 14;
const DOCK_COLORS: [u32; MAX_WINDOWS] = [0x005B_8CFF, 0x0070_D694, 0x00FF_B86B, 0x00B5_8CFF];

const ACCENT: u32 = 0x005B_8CFF;
const TEXT: u32 = 0x00E6_ECF5;
const MUTED: u32 = 0x0095_A3B8;
/// Timer ticks between cursor blinks: 50 ticks at 100 Hz is 500 ms.
const BLINK_TICKS: u64 = 50;

struct State {
    windows: [Window; MAX_WINDOWS],
    /// Indices of open windows, back to front. The last entry is focused.
    order: [usize; MAX_WINDOWS],
    count: usize,
    cur_x: u32,
    cur_y: u32,
    left: bool,
    /// Window being dragged, with the pointer's offset inside it.
    drag: Option<(usize, u32, u32)>,
    /// Window being resized.
    resizing: Option<usize>,
    /// Opacity percentages: [top bar, dock].
    opacity: [u32; SLIDERS],
    /// Slider being dragged, if any.
    slider: Option<usize>,
}

static mut STATE: State = State {
    windows: [
        Window { rect: Rect::new(80, 100, 560, 320), title: "Welcome", kind: Kind::Welcome, open: true },
        Window { rect: Rect::new(680, 140, 480, 240), title: "Keys", kind: Kind::Keys, open: true },
        Window { rect: Rect::new(60, 360, 760, 320), title: "Terminal", kind: Kind::Terminal, open: true },
        Window { rect: Rect::new(420, 160, 440, 220), title: "Settings", kind: Kind::Settings, open: false },
    ],
    order: [0, 1, 2, 3],
    count: 3,
    cur_x: 0,
    cur_y: 0,
    left: false,
    drag: None,
    resizing: None,
    opacity: [60, 60],
    slider: None,
};

// Only interrupt handlers and init touch this, and interrupts don't nest.
fn state() -> &'static mut State {
    unsafe { &mut *addr_of_mut!(STATE) }
}

pub fn init() {
    scene::init();
    terminal::init();
    let s = state();
    s.cur_x = fb::width() / 2;
    s.cur_y = fb::height() / 2;
    render(s, Rect::new(0, 0, fb::width(), fb::height()));
}

pub fn cursor() -> (u32, u32) {
    let s = state();
    (s.cur_x, s.cur_y)
}

/// Called from the timer interrupt, 100 times a second.
pub fn tick(t: u64) {
    if t % BLINK_TICKS != 0 {
        return;
    }
    terminal::toggle_blink();
    let s = state();
    let ti = index_of(s, Kind::Terminal);
    if s.windows[ti].open {
        render(s, s.windows[ti].rect);
    }
    render(s, top_rect());
}

/// Called for every pointer update: new position and whether the left
/// button is held.
pub fn pointer(x: u32, y: u32, left: bool) {
    let s = state();
    let mut damage = cursor_rect(s.cur_x, s.cur_y).union(cursor_rect(x, y));
    s.cur_x = x;
    s.cur_y = y;
    let pressed = left && !s.left;
    s.left = left;

    if let Some(i) = s.slider {
        if left {
            let idx = index_of(s, Kind::Settings);
            let rect = s.windows[idx].rect;
            set_opacity(s, i, x, slider_track(rect, i));
            damage = damage.union(rect).union(top_rect()).union(dock_rect());
        } else {
            s.slider = None;
        }
    } else if let Some((idx, off_x, off_y)) = s.drag {
        if left {
            let old = s.windows[idx].rect;
            let max_x = fb::width() as i32 - old.w as i32;
            let max_y = fb::height() as i32 - old.h as i32;
            s.windows[idx].rect.x = clamp(x as i32 - off_x as i32, 0, max_x);
            // Keep the title bar below the top bar so it stays grabbable.
            s.windows[idx].rect.y = clamp(y as i32 - off_y as i32, TOP_H as i32, max_y.max(TOP_H as i32));
            damage = damage.union(old).union(s.windows[idx].rect);
        } else {
            s.drag = None;
        }
    } else if let Some(idx) = s.resizing {
        if left {
            let old = s.windows[idx].rect;
            // The grip follows the pointer, so the corner sits under it.
            let want_w = x as i32 - old.x as i32 + (GRIP / 2) as i32;
            let want_h = y as i32 - old.y as i32 + (GRIP / 2) as i32;
            let max_w = fb::width() as i32 - old.x as i32;
            let max_h = fb::height() as i32 - old.y as i32;
            s.windows[idx].rect.w = clamp(want_w, MIN_W as i32, max_w.max(MIN_W as i32));
            s.windows[idx].rect.h = clamp(want_h, MIN_H as i32, max_h.max(MIN_H as i32));
            damage = damage.union(old).union(s.windows[idx].rect);
        } else {
            s.resizing = None;
        }
    } else if pressed {
        if y < TOP_H {
            if let Some(i) = top_button_at(x, y) {
                toggle(s, i);
                damage = damage.union(top_rect()).union(s.windows[i].rect);
            }
        } else if let Some(pos) = hit_test(s, x, y) {
            let idx = s.order[pos];
            let rect = s.windows[idx].rect;
            damage = damage.union(rect);
            if close_hit(rect, x, y) {
                remove_at(s, pos);
                s.windows[idx].open = false;
            } else {
                raise(s, pos);
                if s.windows[idx].kind == Kind::Settings {
                    if let Some(i) = slider_hit(rect, x, y) {
                        s.slider = Some(i);
                        set_opacity(s, i, x, slider_track(rect, i));
                        damage = damage.union(top_rect()).union(dock_rect());
                    }
                }
                if s.slider.is_none() {
                    if grip_hit(rect, x, y) {
                        s.resizing = Some(idx);
                    } else if y < rect.y + TITLE_H {
                        s.drag = Some((idx, x - rect.x, y - rect.y));
                    }
                }
            }
        } else if let Some(i) = dock_tile_at(x, y) {
            toggle(s, i);
            damage = damage.union(dock_rect()).union(s.windows[i].rect);
        }
    }

    render(s, damage);
}

/// Route one decoded key to the focused window, then repaint it.
pub fn key(c: u8) {
    let s = state();
    if s.count == 0 {
        return;
    }
    let idx = s.order[s.count - 1];
    match s.windows[idx].kind {
        Kind::Keys => ps2::type_key(c),
        Kind::Terminal => terminal::key(c),
        Kind::Welcome | Kind::Settings => return,
    }
    render(s, s.windows[idx].rect);
}

fn clamp(v: i32, lo: i32, hi: i32) -> u32 {
    v.max(lo).min(hi.max(lo)) as u32
}

fn index_of(s: &State, kind: Kind) -> usize {
    (0..MAX_WINDOWS).find(|&i| s.windows[i].kind == kind).unwrap_or(0)
}

fn alpha_of(percent: u32) -> u32 {
    percent.min(100) * 255 / 100
}

fn top_rect() -> Rect {
    Rect::new(0, 0, fb::width(), TOP_H)
}

fn dock_w() -> u32 {
    DOCK_PAD * 2 + MAX_WINDOWS as u32 * TILE + (MAX_WINDOWS as u32 - 1) * TILE_GAP
}

fn dock_rect() -> Rect {
    let w = dock_w();
    Rect::new(
        (fb::width().saturating_sub(w)) / 2,
        fb::height().saturating_sub(DOCK_H + DOCK_BOTTOM_GAP),
        w,
        DOCK_H,
    )
}

fn tile_x(i: usize) -> u32 {
    dock_rect().x + DOCK_PAD + i as u32 * (TILE + TILE_GAP)
}

fn btn_x(i: usize) -> u32 {
    BTN_X0 + i as u32 * (BTN_W + BTN_GAP)
}

fn top_button_at(x: u32, y: u32) -> Option<usize> {
    if y < BTN_Y || y >= BTN_Y + BTN_H {
        return None;
    }
    (0..MAX_WINDOWS).find(|&i| x >= btn_x(i) && x < btn_x(i) + BTN_W)
}

fn dock_tile_at(x: u32, y: u32) -> Option<usize> {
    let d = dock_rect();
    if !d.contains(x, y) {
        return None;
    }
    (0..MAX_WINDOWS).find(|&i| {
        let tx = tile_x(i);
        x >= tx && x < tx + TILE && y >= d.y + 10 && y < d.y + 10 + TILE
    })
}

/// Track rectangle for slider `i` inside the Settings window.
fn slider_track(r: Rect, i: usize) -> Rect {
    Rect::new(r.x + 24, r.y + TITLE_H + 24 + i as u32 * 80 + 34, r.w - 48, 6)
}

fn slider_hit(r: Rect, x: u32, y: u32) -> Option<usize> {
    (0..SLIDERS).find(|&i| {
        let t = slider_track(r, i);
        x >= t.x && x < t.x + t.w && y + 12 >= t.y && y < t.y + t.h + 12
    })
}

fn set_opacity(s: &mut State, i: usize, x: u32, track: Rect) {
    let v = (x as i32 - track.x as i32) * 100 / track.w.max(1) as i32;
    s.opacity[i] = v.max(0).min(100) as u32;
}

fn cursor_rect(x: u32, y: u32) -> Rect {
    Rect::new(x, y, 8, 12)
}

fn close_center(rect: Rect) -> (u32, u32) {
    (rect.x + 18, rect.y + 15)
}

fn close_hit(rect: Rect, x: u32, y: u32) -> bool {
    let (cx, cy) = close_center(rect);
    (x as i32 - cx as i32).abs() <= 8 && (y as i32 - cy as i32).abs() <= 8
}

fn grip_hit(rect: Rect, x: u32, y: u32) -> bool {
    x >= rect.x + rect.w - GRIP
        && x < rect.x + rect.w
        && y >= rect.y + rect.h - GRIP
        && y < rect.y + rect.h
}

/// Position in `order` of the topmost window under the pointer.
fn hit_test(s: &State, x: u32, y: u32) -> Option<usize> {
    (0..s.count).rev().find(|&pos| s.windows[s.order[pos]].rect.contains(x, y))
}

/// Taskbar-style behaviour, shared by the top bar and dock: a closed window
/// opens on top, a background window is raised, and the focused window is
/// closed.
fn toggle(s: &mut State, idx: usize) {
    if !s.windows[idx].open {
        s.windows[idx].open = true;
        s.order[s.count] = idx;
        s.count += 1;
        return;
    }
    let pos = (0..s.count).find(|&p| s.order[p] == idx).unwrap_or(0);
    if pos == s.count - 1 {
        remove_at(s, pos);
        s.windows[idx].open = false;
    } else {
        raise(s, pos);
    }
}

fn remove_at(s: &mut State, pos: usize) {
    for i in pos..s.count - 1 {
        s.order[i] = s.order[i + 1];
    }
    s.count -= 1;
}

fn raise(s: &mut State, pos: usize) {
    let idx = s.order[pos];
    for i in pos..s.count - 1 {
        s.order[i] = s.order[i + 1];
    }
    s.order[s.count - 1] = idx;
}

/// Repaint one damaged rectangle: wallpaper, dock, open windows back to
/// front, top bar, then the pointer. Everything is clipped to `r`.
fn render(s: &State, r: Rect) {
    fb::set_clip(r.x, r.y, r.w, r.h);
    scene::draw_background();
    draw_dock(s);

    let focused = s.count.saturating_sub(1);
    for pos in 0..s.count {
        let win = s.windows[s.order[pos]];
        if win.rect.overlaps(r) {
            draw_window(&win, pos == focused, s.opacity);
        }
    }

    draw_topbar(s);
    draw_cursor(s.cur_x, s.cur_y);
    fb::reset_clip();
    fb::present(r.x, r.y, r.w, r.h);
}

/// Translucent dock with one tile per app. A dot under a tile means open.
fn draw_dock(s: &State) {
    let d = dock_rect();
    fb::blend_rounded_rect(d.x, d.y, d.w, d.h, 22, 0x0021_2A38, alpha_of(s.opacity[1]));
    for i in 0..MAX_WINDOWS {
        let win = s.windows[i];
        let tx = tile_x(i);
        let ty = d.y + 10;
        fb::fill_rounded_rect(tx, ty, TILE, TILE, 12, DOCK_COLORS[i]);
        fb::draw_text(tx + (TILE - 24) / 2, ty + (TILE - 24) / 2, &win.title[..1], 0x00FF_FFFF, 3);
        if win.open {
            fb::fill_rect(tx + TILE / 2 - 2, d.y + DOCK_H - 6, 4, 4, TEXT);
        }
    }
}

/// Translucent top bar: brand, one button per window, and the uptime clock.
fn draw_topbar(s: &State) {
    let w = fb::width();
    fb::blend_rect(0, 0, w, TOP_H, 0x0030_3A4E, alpha_of(s.opacity[0]));
    fb::draw_text(14, 12, "AnarchyOS", TEXT, 2);

    let focused = if s.count > 0 { Some(s.order[s.count - 1]) } else { None };
    for i in 0..MAX_WINDOWS {
        let win = s.windows[i];
        let bx = btn_x(i);
        let color = if Some(i) == focused {
            ACCENT
        } else if win.open {
            0x0034_3F52
        } else {
            0x0018_1E2A
        };
        let text_color = if win.open { TEXT } else { 0x0068_7488 };
        fb::fill_rounded_rect(bx, BTN_Y, BTN_W, BTN_H, 9, color);
        let tw = win.title.len() as u32 * 16;
        fb::draw_text(bx + (BTN_W - tw) / 2, BTN_Y + 6, win.title, text_color, 2);
    }

    let secs = pit::ticks() / pit::HZ;
    let mut clock = *b"up 00:00";
    two_digits(&mut clock, 3, (secs / 60) % 100);
    two_digits(&mut clock, 6, secs % 60);
    let text = core::str::from_utf8(&clock).unwrap_or("");
    fb::draw_text(w.saturating_sub(16 + 8 * 16), 12, text, MUTED, 2);
}

fn two_digits(buf: &mut [u8], at: usize, n: u64) {
    buf[at] = b'0' + ((n / 10) % 10) as u8;
    buf[at + 1] = b'0' + (n % 10) as u8;
}

fn draw_window(win: &Window, focused: bool, opacity: [u32; SLIDERS]) {
    let Rect { x, y, w, h } = win.rect;

    // Clip the window's contents to its own frame, so long lines and
    // large content never spill onto neighbouring windows.
    let prev = fb::current_clip();
    let x0 = x.max(prev.0);
    let y0 = y.max(prev.1);
    let x1 = (x + w).min(prev.2);
    let y1 = (y + h).min(prev.3);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    fb::set_clip(x0, y0, x1 - x0, y1 - y0);

    let title_color = if focused { 0x002C_3646 } else { 0x0021_2A38 };
    fb::fill_rounded_rect(x, y, w, h, RADIUS, 0x0014_1A28);
    fb::fill_rounded_rect(x, y, w, TITLE_H, RADIUS, title_color);
    // Square off the bottom of the title bar so it meets the body cleanly.
    fb::fill_rect(x, y + RADIUS, w, TITLE_H - RADIUS, title_color);

    let (cx, cy) = close_center(win.rect);
    fb::fill_rounded_rect(cx - 6, cy - 6, 12, 12, 6, 0x00FF_5F57);
    fb::draw_text(x + 36, y + 7, win.title, TEXT, 2);

    match win.kind {
        Kind::Welcome => welcome_content(win.rect),
        Kind::Keys => keys_content(win.rect),
        Kind::Terminal => terminal::draw(x, y, w, h),
        Kind::Settings => settings_content(win.rect, opacity),
    }

    draw_grip(win.rect, focused);
    fb::restore_clip(prev);
}

/// Two sliders, one for the top bar and one for the dock.
fn settings_content(r: Rect, opacity: [u32; SLIDERS]) {
    let labels = ["Top bar opacity", "Dock opacity"];
    for i in 0..SLIDERS {
        let ly = r.y + TITLE_H + 24 + i as u32 * 80;
        fb::draw_text(r.x + 24, ly, labels[i], 0x00C8_D3E0, 2);

        let value = format!("{}%", opacity[i]);
        let vx = r.x + r.w - 24 - value.len() as u32 * 16;
        fb::draw_text(vx, ly, &value, TEXT, 2);

        let t = slider_track(r, i);
        fb::fill_rounded_rect(t.x, t.y, t.w, t.h, 3, 0x0030_3A4E);
        let filled = t.w * opacity[i] / 100;
        fb::fill_rounded_rect(t.x, t.y, filled, t.h, 3, ACCENT);
        let knob = t.x + filled;
        fb::fill_rounded_rect(knob.saturating_sub(8), t.y - 5, 16, 16, 8, 0x00FF_FFFF);
    }
}

fn welcome_content(r: Rect) {
    let x = r.x + 32;
    let y = r.y + TITLE_H + 28;
    fb::draw_text(x, y, "AnarchyOS", TEXT, 5);
    fb::draw_text(x, y + 60, "A clean, fun kernel and desktop", MUTED, 2);
    fb::draw_text(x, y + 110, "Drag by the title bar. Resize from the corner.", 0x00C8_D3E0, 2);
    fb::draw_text(x, y + 140, "The dock opens apps. The top bar lists open windows.", 0x00C8_D3E0, 2);
}

fn keys_content(r: Rect) {
    let x = r.x + 20;
    let y = r.y + TITLE_H + 20;
    fb::draw_text(x, y, "Type on the keyboard.", MUTED, 2);

    let mut buf = [0u8; ps2::TYPED_MAX];
    let n = ps2::copy_typed(&mut buf);
    // Show the most recent characters that fit in the window.
    let max_chars = (r.w.saturating_sub(60) / 16) as usize;
    let start = n.saturating_sub(max_chars);
    let text = core::str::from_utf8(&buf[start..n]).unwrap_or("");
    fb::draw_text(x, y + 40, "> ", 0x0070_D694, 2);
    fb::draw_text(x + 32, y + 40, text, TEXT, 2);
}

/// Three diagonal dots in the bottom-right corner, the resize affordance.
fn draw_grip(rect: Rect, focused: bool) {
    let color = if focused { 0x008A_96A8 } else { 0x0045_4F60 };
    let Rect { x, y, w, h } = rect;
    for i in 1..=3u32 {
        let d = i * 5;
        fb::fill_rect(x + w - d, y + h - 4, 2, 2, color);
        fb::fill_rect(x + w - 4, y + h - d, 2, 2, color);
    }
}

fn draw_cursor(x: u32, y: u32) {
    for row in 0..10u32 {
        let width = (row + 1).min(7);
        for col in 0..width {
            fb::put_pixel(x + col, y + row, 0x00FF_FFFF);
        }
    }
}
