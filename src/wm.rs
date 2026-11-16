//! Window manager. Windows are rounded rectangles with a title bar and a
//! resize grip in the bottom-right corner. Windows stay in back-to-front
//! order, and any change repaints the damaged rectangle from the background
//! up. Keyboard input goes to the focused window, which is the one on top.

use crate::framebuffer as fb;
use crate::ps2;
use crate::scene;
use crate::terminal;
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
}

#[derive(Clone, Copy)]
struct Window {
    rect: Rect,
    title: &'static str,
    kind: Kind,
}

const MAX_WINDOWS: usize = 3;
const TITLE_H: u32 = 30;
const RADIUS: u32 = 14;
const MIN_W: u32 = 260;
const MIN_H: u32 = 160;
/// Size of the square resize grip in the bottom-right corner.
const GRIP: u32 = 18;

struct State {
    windows: [Window; MAX_WINDOWS],
    /// Indices into `windows`, back to front. The last entry is focused.
    order: [usize; MAX_WINDOWS],
    count: usize,
    cur_x: u32,
    cur_y: u32,
    left: bool,
    /// Window being dragged, with the pointer's offset inside it.
    drag: Option<(usize, u32, u32)>,
    /// Window being resized.
    resizing: Option<usize>,
}

static mut STATE: State = State {
    windows: [
        Window { rect: Rect::new(80, 100, 560, 320), title: "Welcome", kind: Kind::Welcome },
        Window { rect: Rect::new(680, 140, 480, 240), title: "Keys", kind: Kind::Keys },
        Window { rect: Rect::new(60, 420, 760, 340), title: "Terminal", kind: Kind::Terminal },
    ],
    order: [0, 1, 2],
    count: 3,
    cur_x: 0,
    cur_y: 0,
    left: false,
    drag: None,
    resizing: None,
};

// Only the interrupt path and init touch this, and interrupts don't nest.
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

/// Called for every pointer update: new position and whether the left
/// button is held.
pub fn pointer(x: u32, y: u32, left: bool) {
    let s = state();
    let mut damage = cursor_rect(s.cur_x, s.cur_y).union(cursor_rect(x, y));
    s.cur_x = x;
    s.cur_y = y;
    let pressed = left && !s.left;
    s.left = left;

    if let Some((idx, off_x, off_y)) = s.drag {
        if left {
            let old = s.windows[idx].rect;
            let max_x = fb::width() as i32 - old.w as i32;
            let max_y = fb::height() as i32 - old.h as i32;
            s.windows[idx].rect.x = clamp(x as i32 - off_x as i32, 0, max_x);
            s.windows[idx].rect.y = clamp(y as i32 - off_y as i32, 0, max_y);
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
        if let Some(pos) = hit_test(s, x, y) {
            let idx = s.order[pos];
            let rect = s.windows[idx].rect;
            damage = damage.union(rect);
            if close_hit(rect, x, y) {
                remove_at(s, pos);
            } else {
                raise(s, pos);
                if grip_hit(rect, x, y) {
                    s.resizing = Some(idx);
                } else if y < rect.y + TITLE_H {
                    s.drag = Some((idx, x - rect.x, y - rect.y));
                }
            }
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
        Kind::Welcome => return,
    }
    render(s, s.windows[idx].rect);
}

fn clamp(v: i32, lo: i32, hi: i32) -> u32 {
    v.max(lo).min(hi.max(lo)) as u32
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

/// Repaint one damaged rectangle: background, windows back to front, top
/// bar, then the pointer. Everything is clipped to `r`.
fn render(s: &State, r: Rect) {
    fb::set_clip(r.x, r.y, r.w, r.h);
    scene::draw_background();

    let focused = s.count.saturating_sub(1);
    for pos in 0..s.count {
        let win = s.windows[s.order[pos]];
        if win.rect.overlaps(r) {
            draw_window(&win, pos == focused);
        }
    }

    scene::draw_topbar();
    draw_cursor(s.cur_x, s.cur_y);
    fb::reset_clip();
    fb::present(r.x, r.y, r.w, r.h);
}

fn draw_window(win: &Window, focused: bool) {
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
    fb::draw_text(x + 36, y + 7, win.title, 0x00E6_ECF5, 2);

    match win.kind {
        Kind::Welcome => welcome_content(win.rect),
        Kind::Keys => keys_content(win.rect),
        Kind::Terminal => terminal::draw(x, y, w, h),
    }

    draw_grip(win.rect, focused);
    fb::restore_clip(prev);
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

fn welcome_content(r: Rect) {
    let x = r.x + 32;
    let y = r.y + TITLE_H + 28;
    fb::draw_text(x, y, "AnarchyOS", 0x00E6_ECF5, 5);
    fb::draw_text(x, y + 60, "A clean, fun kernel and desktop", 0x0095_A3B8, 2);
    fb::draw_text(x, y + 110, "Drag by the title bar. Resize from the corner.", 0x00C8_D3E0, 2);
    fb::draw_text(x, y + 140, "Click any window to bring it forward.", 0x00C8_D3E0, 2);
}

fn keys_content(r: Rect) {
    let x = r.x + 20;
    let y = r.y + TITLE_H + 20;
    fb::draw_text(x, y, "Type on the keyboard.", 0x0095_A3B8, 2);

    let mut buf = [0u8; ps2::TYPED_MAX];
    let n = ps2::copy_typed(&mut buf);
    // Show the most recent characters that fit in the window.
    let max_chars = (r.w.saturating_sub(60) / 16) as usize;
    let start = n.saturating_sub(max_chars);
    let text = core::str::from_utf8(&buf[start..n]).unwrap_or("");
    fb::draw_text(x, y + 40, "> ", 0x0070_D694, 2);
    fb::draw_text(x + 32, y + 40, text, 0x00E6_ECF5, 2);
}

fn draw_cursor(x: u32, y: u32) {
    for row in 0..10u32 {
        let width = (row + 1).min(7);
        for col in 0..width {
            fb::put_pixel(x + col, y + row, 0x00FF_FFFF);
        }
    }
}
