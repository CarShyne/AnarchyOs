//! Window manager and desktop shell: the menu bar, the dock, the app
//! drawer, and the windows in between.
//!
//! Drawing is painter's style. Any change repaints a damaged rectangle
//! from the wallpaper up into the back buffer, then copies that rectangle
//! to the screen, so nothing ever leaves a trail.

use crate::clock;
use crate::framebuffer as fb;
use crate::gfx;
use crate::image::{self, icon, BODY, LABEL, MARK_22, MARK_48, SMALL, STRONG, TITLE, WORDMARK};
use crate::net;
use crate::pci;
use crate::ps2;
use crate::scene;
use crate::settings_app::{self, Outcome};
use crate::terminal;
use crate::theme::{self, FAINT, MUTED, TEXT, WHITE};
use crate::ui::Rect;
use core::ptr::addr_of_mut;

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
    /// Size to return to when leaving full screen.
    saved: Rect,
    title: &'static str,
    kind: Kind,
    /// Running. A running window may still be hidden in the dock.
    open: bool,
    full: bool,
    min_w: i32,
    min_h: i32,
}

struct App {
    name: &'static str,
    window: Option<usize>,
    icon: usize,
    /// Tile gradient, top and bottom.
    top: u32,
    bottom: u32,
}

const WELCOME: usize = 0;
const KEYS: usize = 1;
const TERMINAL: usize = 2;
const SETTINGS: usize = 3;
const MAX_WINDOWS: usize = 4;

const TITLE_H: i32 = 36;
const GRIP: i32 = 18;
/// How far a window's shadow reaches past its frame.
const SHADOW: i32 = 13;

const TOP_H: i32 = 30;

const DOCK_SLOTS: usize = 5;
const TILE: i32 = 48;
const DOCK_PAD: i32 = 10;
const TILE_GAP: i32 = 10;
const DOCK_H: i32 = TILE + 2 * DOCK_PAD + 4;
const DOCK_BOTTOM_GAP: i32 = 12;

const DRAWER_W: i32 = 600;
const DRAWER_H: i32 = 320;
const DRAWER_COLS: usize = 4;
const CELL_W: i32 = 132;
const CELL_H: i32 = 116;
const BIG_TILE: i32 = 64;

const CURSOR_W: i32 = 12;
const CURSOR_H: i32 = 19;
/// Timer ticks between cursor blinks: 50 ticks at 100 Hz is half a second.
const BLINK_TICKS: u64 = 50;

const APPS: [App; 8] = [
    App { name: "Welcome", window: Some(WELCOME), icon: icon::SPARKLES, top: 0x00FF_4D6D, bottom: 0x00C9_0A38 },
    App { name: "Keys", window: Some(KEYS), icon: icon::KEYBOARD, top: 0x002D_D4BF, bottom: 0x000F_766E },
    App { name: "Terminal", window: Some(TERMINAL), icon: icon::TERMINAL, top: 0x003A_3D46, bottom: 0x0016_171C },
    App { name: "Settings", window: Some(SETTINGS), icon: icon::SETTINGS, top: 0x008B_93A1, bottom: 0x004B_5563 },
    App { name: "Files", window: None, icon: icon::FOLDER, top: 0x0060_A5FA, bottom: 0x001D_4ED8 },
    App { name: "Monitor", window: None, icon: icon::ACTIVITY, top: 0x004A_DE80, bottom: 0x0015_803D },
    App { name: "Network", window: None, icon: icon::GLOBE, top: 0x0038_BDF8, bottom: 0x0003_69A1 },
    App { name: "Notes", window: None, icon: icon::NOTES, top: 0x00FB_BF24, bottom: 0x00B4_5309 },
];

struct State {
    windows: [Window; MAX_WINDOWS],
    /// Visible windows, back to front. The last one has keyboard focus.
    order: [usize; MAX_WINDOWS],
    count: usize,
    cur_x: i32,
    cur_y: i32,
    left: bool,
    /// Window being moved, with the pointer's offset inside it.
    drag: Option<(usize, i32, i32)>,
    resizing: Option<usize>,
    /// The press began inside the Settings app, which now owns the drag.
    in_settings: bool,
    drawer: bool,
    settings: settings_app::State,
    /// Last clock value drawn in the menu bar.
    clock_stamp: u64,
}

const fn window(rect: Rect, title: &'static str, kind: Kind, open: bool, min_w: i32, min_h: i32) -> Window {
    Window { rect, saved: rect, title, kind, open, full: false, min_w, min_h }
}

static mut STATE: State = State {
    windows: [
        window(Rect::new(96, 84, 560, 356), "Welcome", Kind::Welcome, true, 420, 300),
        window(Rect::new(720, 120, 440, 210), "Keys", Kind::Keys, false, 320, 180),
        window(Rect::new(150, 380, 760, 340), "Terminal", Kind::Terminal, false, 380, 220),
        window(Rect::new(190, 80, 900, 620), "Settings", Kind::Settings, false, 760, 520),
    ],
    order: [WELCOME, 0, 0, 0],
    count: 1,
    cur_x: 0,
    cur_y: 0,
    left: false,
    drag: None,
    resizing: None,
    in_settings: false,
    drawer: false,
    settings: settings_app::State::new(),
    clock_stamp: 0,
};

// Only the main loop touches this state.
fn state() -> &'static mut State {
    unsafe { &mut *addr_of_mut!(STATE) }
}

pub fn init() {
    pci::init();
    theme::load();
    clock::refresh();
    scene::apply();
    terminal::init();

    let s = state();
    let screen = full_damage();
    for w in s.windows.iter_mut() {
        w.rect = fit(w.rect, screen);
        w.saved = w.rect;
    }
    s.cur_x = screen.w / 2;
    s.cur_y = screen.h / 2;
    s.clock_stamp = clock::stamp();
    render(s, screen);
}

/// Shrink and move a window so it lies inside the screen below the menu bar.
fn fit(r: Rect, screen: Rect) -> Rect {
    let w = r.w.min(screen.w);
    let h = r.h.min(screen.h - TOP_H);
    let x = r.x.clamp(0, screen.w - w);
    let y = r.y.clamp(TOP_H, screen.h - h);
    Rect::new(x, y, w, h)
}

/// Called from the main loop on every timer tick, 100 times a second.
pub fn tick(t: u64) {
    if t % BLINK_TICKS != 0 {
        return;
    }
    terminal::toggle_blink();
    clock::refresh();

    let s = state();
    if visible(s, TERMINAL).is_some() {
        // Only the prompt line changes when the cursor blinks.
        let r = s.windows[TERMINAL].rect;
        render(s, Rect::new(r.x, r.y + r.h - terminal::PROMPT_H, r.w, terminal::PROMPT_H));
    }
    let whole_second = t % (2 * BLINK_TICKS) == 0;
    if whole_second && visible(s, SETTINGS).is_some() && settings_app::is_live(s.settings.page()) {
        render(s, s.windows[SETTINGS].rect);
    }
    let stamp = clock::stamp();
    if stamp != s.clock_stamp {
        s.clock_stamp = stamp;
        render(s, top_rect());
    }
}

/// Relative mouse movement from the PS/2 driver. `dy` is positive upward.
pub fn mouse(dx: i32, dy: i32, left: bool) {
    let s = state();
    let x = (s.cur_x + dx).clamp(0, fb::width() as i32 - 1);
    let y = (s.cur_y - dy).clamp(0, fb::height() as i32 - 1);
    pointer(x, y, left);
}

fn pointer(x: i32, y: i32, left: bool) {
    let s = state();
    let mut damage = cursor_rect(s.cur_x, s.cur_y).union(cursor_rect(x, y));
    s.cur_x = x;
    s.cur_y = y;
    let pressed = left && !s.left;
    s.left = left;

    if s.in_settings {
        if left {
            let body = body_of(s.windows[SETTINGS].rect);
            match settings_app::drag(body, &mut s.settings, x, y) {
                Outcome::Everything => damage = full_damage(),
                Outcome::Window => damage = damage.union(s.windows[SETTINGS].rect),
                Outcome::Nothing => {}
            }
        } else {
            s.in_settings = false;
            settings_app::release(&mut s.settings);
        }
    } else if let Some((idx, ox, oy)) = s.drag {
        if left {
            let old = s.windows[idx].rect;
            let screen = full_damage();
            let w = &mut s.windows[idx].rect;
            w.x = (x - ox).clamp(0, (screen.w - w.w).max(0));
            w.y = (y - oy).clamp(TOP_H, (screen.h - w.h).max(TOP_H));
            damage = damage.union(old.grow(SHADOW)).union(w.grow(SHADOW));
        } else {
            s.drag = None;
        }
    } else if let Some(idx) = s.resizing {
        if left {
            let old = s.windows[idx].rect;
            let screen = full_damage();
            let (min_w, min_h) = (s.windows[idx].min_w, s.windows[idx].min_h);
            let w = &mut s.windows[idx].rect;
            // The grip follows the pointer, so the corner stays under it.
            w.w = (x - w.x + GRIP / 2).clamp(min_w, (screen.w - w.x).max(min_w));
            w.h = (y - w.y + GRIP / 2).clamp(min_h, (screen.h - w.y).max(min_h));
            damage = damage.union(old.grow(SHADOW)).union(w.grow(SHADOW));
        } else {
            s.resizing = None;
        }
    } else if pressed {
        if let Some(d) = click(s, x, y) {
            damage = damage.union(d);
        }
    }

    render(s, damage);
}

/// A decoded key press goes to the focused window.
pub fn key(c: u8) {
    let s = state();
    if s.count == 0 {
        return;
    }
    let idx = s.order[s.count - 1];
    match s.windows[idx].kind {
        Kind::Keys => ps2::type_key(c),
        Kind::Terminal => {
            terminal::key(c);
            if c == b'\n' {
                // A command may have changed a setting, so pick it up.
                let before = (theme::cfg().wallpaper, theme::cfg().accent);
                theme::load();
                if before != (theme::cfg().wallpaper, theme::cfg().accent) {
                    scene::apply();
                }
                render(s, full_damage());
                return;
            }
        }
        Kind::Welcome | Kind::Settings => return,
    }
    render(s, s.windows[idx].rect);
}

/// Handle a mouse press. Returns the area that needs repainting.
fn click(s: &mut State, x: i32, y: i32) -> Option<Rect> {
    let full = Some(full_damage());

    if s.drawer {
        if let Some(a) = drawer_app_at(x, y) {
            s.drawer = false;
            if let Some(w) = APPS[a].window {
                show(s, w);
            }
            return full;
        }
        if drawer_rect().contains(x, y) {
            return None;
        }
        if dock_slot_at(x, y) != Some(0) {
            s.drawer = false;
            return full;
        }
    }

    if y < TOP_H {
        let bar = bar_layout();
        if x < bar.name_x {
            open_settings(s, settings_app::ABOUT);
        } else if x >= bar.clock_x {
            open_settings(s, settings_app::DATE_TIME);
        } else if x >= bar.status_x {
            open_settings(s, settings_app::NETWORK);
        } else {
            return None;
        }
        return full;
    }

    if let Some(slot) = dock_slot_at(x, y) {
        if slot == 0 {
            s.drawer = !s.drawer;
        } else {
            dock_click(s, slot - 1);
        }
        return full;
    }
    if dock_rect().contains(x, y) {
        return None;
    }

    let pos = hit_test(s, x, y)?;
    let idx = s.order[pos];
    let rect = s.windows[idx].rect;

    if y < rect.y + TITLE_H {
        if dot_hit(rect, x, y, 0) {
            close(s, idx);
            return full;
        }
        if dot_hit(rect, x, y, 1) {
            hide(s, idx);
            return full;
        }
        if dot_hit(rect, x, y, 2) {
            toggle_full(s, idx);
            return full;
        }
    }

    raise(s, pos);
    let mut damage = rect.grow(SHADOW);

    if !s.windows[idx].full && grip_hit(rect, x, y) {
        s.resizing = Some(idx);
    } else if y < rect.y + TITLE_H {
        if !s.windows[idx].full {
            s.drag = Some((idx, x - rect.x, y - rect.y));
        }
    } else if idx == SETTINGS {
        s.in_settings = true;
        if settings_app::press(body_of(rect), &mut s.settings, x, y) == Outcome::Everything {
            damage = full_damage();
        }
    }
    Some(damage)
}

fn open_settings(s: &mut State, page: usize) {
    s.settings.set_page(page);
    show(s, SETTINGS);
}

// ---- Geometry --------------------------------------------------------------

fn visible(s: &State, idx: usize) -> Option<usize> {
    (0..s.count).find(|&p| s.order[p] == idx)
}

fn full_damage() -> Rect {
    Rect::new(0, 0, fb::width() as i32, fb::height() as i32)
}

fn cursor_rect(x: i32, y: i32) -> Rect {
    Rect::new(x, y, CURSOR_W, CURSOR_H)
}

fn top_rect() -> Rect {
    Rect::new(0, 0, fb::width() as i32, TOP_H)
}

/// The part of a window below its title bar.
fn body_of(r: Rect) -> Rect {
    Rect::new(r.x, r.y + TITLE_H, r.w, r.h - TITLE_H)
}

fn dock_rect() -> Rect {
    let n = DOCK_SLOTS as i32;
    let w = DOCK_PAD * 2 + n * TILE + (n - 1) * TILE_GAP;
    let screen = full_damage();
    Rect::new((screen.w - w) / 2, screen.h - DOCK_H - DOCK_BOTTOM_GAP, w, DOCK_H)
}

fn tile_rect(slot: usize) -> Rect {
    let d = dock_rect();
    Rect::new(d.x + DOCK_PAD + slot as i32 * (TILE + TILE_GAP), d.y + DOCK_PAD, TILE, TILE)
}

fn dock_slot_at(x: i32, y: i32) -> Option<usize> {
    (0..DOCK_SLOTS).find(|&i| tile_rect(i).contains(x, y))
}

fn drawer_rect() -> Rect {
    let d = dock_rect();
    let screen = full_damage();
    Rect::new((screen.w - DRAWER_W) / 2, d.y - 14 - DRAWER_H, DRAWER_W, DRAWER_H)
}

fn drawer_cell(i: usize) -> Rect {
    let r = drawer_rect();
    let cols = DRAWER_COLS as i32;
    let gx = (DRAWER_W - cols * CELL_W) / 2;
    Rect::new(
        r.x + gx + (i as i32 % cols) * CELL_W,
        r.y + 64 + (i as i32 / cols) * CELL_H,
        CELL_W,
        CELL_H,
    )
}

fn drawer_app_at(x: i32, y: i32) -> Option<usize> {
    (0..APPS.len()).find(|&i| drawer_cell(i).contains(x, y) && APPS[i].window.is_some())
}

fn dot_center(rect: Rect, n: i32) -> (i32, i32) {
    (rect.x + 20 + n * 20, rect.y + TITLE_H / 2)
}

fn dot_hit(rect: Rect, x: i32, y: i32, n: i32) -> bool {
    let (cx, cy) = dot_center(rect, n);
    (x - cx).abs() <= 8 && (y - cy).abs() <= 8
}

fn grip_hit(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x + rect.w - GRIP && x < rect.x + rect.w && y >= rect.y + rect.h - GRIP && y < rect.y + rect.h
}

/// Position in `order` of the topmost window under the pointer.
fn hit_test(s: &State, x: i32, y: i32) -> Option<usize> {
    (0..s.count).rev().find(|&pos| s.windows[s.order[pos]].rect.contains(x, y))
}

// ---- Window stack ----------------------------------------------------------

fn show(s: &mut State, idx: usize) {
    match visible(s, idx) {
        Some(p) => raise(s, p),
        None => {
            s.windows[idx].open = true;
            s.order[s.count] = idx;
            s.count += 1;
        }
    }
}

/// Dock behaviour: bring an app forward, or tuck the front app away.
fn dock_click(s: &mut State, idx: usize) {
    match visible(s, idx) {
        Some(p) if p + 1 == s.count => hide(s, idx),
        _ => show(s, idx),
    }
}

/// Send to the dock. The app keeps its state and its running dot.
fn hide(s: &mut State, idx: usize) {
    if let Some(p) = visible(s, idx) {
        remove_at(s, p);
    }
}

fn close(s: &mut State, idx: usize) {
    hide(s, idx);
    let w = &mut s.windows[idx];
    w.open = false;
    if w.full {
        w.rect = w.saved;
        w.full = false;
    }
}

fn toggle_full(s: &mut State, idx: usize) {
    let screen = full_damage();
    let w = &mut s.windows[idx];
    if w.full {
        w.rect = w.saved;
        w.full = false;
    } else {
        w.saved = w.rect;
        w.rect = Rect::new(0, TOP_H, screen.w, screen.h - TOP_H);
        w.full = true;
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

// ---- Drawing ---------------------------------------------------------------

/// Repaint one damaged rectangle: wallpaper, windows back to front, dock,
/// drawer, menu bar, then the pointer.
fn render(s: &mut State, r: Rect) {
    let screen = full_damage();
    let x0 = r.x.max(0);
    let y0 = r.y.max(0);
    let x1 = (r.x + r.w).min(screen.w);
    let y1 = (r.y + r.h).min(screen.h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let r = Rect::new(x0, y0, x1 - x0, y1 - y0);
    fb::set_clip(r.x as u32, r.y as u32, r.w as u32, r.h as u32);

    scene::draw_background();
    for pos in 0..s.count {
        let idx = s.order[pos];
        if s.windows[idx].rect.grow(SHADOW).overlaps(r) {
            draw_window(s, idx, pos + 1 == s.count);
        }
    }
    draw_dock(s);
    draw_drawer(s);
    draw_topbar(s);
    draw_cursor(s.cur_x, s.cur_y);

    fb::reset_clip();
    fb::present(r.x as u32, r.y as u32, r.w as u32, r.h as u32);
}

/// A rounded app icon: a shaded tile with a white symbol on it.
fn draw_tile(x: i32, y: i32, size: i32, app: &App, alpha: u32) {
    gfx::round_grad(x, y, size, size, size * 27 / 100, app.top, app.bottom, alpha);
    gfx::stroke(x, y, size, size, size * 27 / 100, WHITE, 38 * alpha / 255);
    let mask = if size >= BIG_TILE { image::icon34(app.icon) } else { image::icon26(app.icon) };
    let off = (size - mask.size as i32) / 2;
    gfx::icon_a(x + off, y + off, mask, WHITE, alpha);
}

fn draw_dock(s: &State) {
    let d = dock_rect();
    let c = theme::cfg();
    gfx::round_a(d.x, d.y, d.w, d.h, 20, theme::BAR, theme::alpha(c.dock_opacity));
    gfx::stroke(d.x, d.y, d.w, d.h, 20, WHITE, theme::EDGE_A);

    for slot in 0..DOCK_SLOTS {
        let t = tile_rect(slot);
        if slot == 0 {
            gfx::image_a(t.x, t.y, &MARK_48, 255);
            if s.drawer {
                gfx::round(t.x + TILE / 2 - 8, d.y + d.h - 6, 16, 3, 1, theme::accent());
            }
        } else {
            draw_tile(t.x, t.y, TILE, &APPS[slot - 1], 255);
            if s.windows[slot - 1].open {
                let on_screen = visible(s, slot - 1).is_some();
                let color = if on_screen { WHITE } else { FAINT };
                gfx::round(t.x + TILE / 2 - 2, d.y + d.h - 7, 4, 4, 2, color);
            }
        }
    }
}

fn draw_drawer(s: &State) {
    if !s.drawer {
        return;
    }
    let r = drawer_rect();
    let alpha = theme::alpha(theme::cfg().dock_opacity).max(215);
    gfx::shadow(r.x, r.y, r.w, r.h, 22, 40);
    gfx::round_a(r.x, r.y, r.w, r.h, 22, theme::BAR, alpha);
    gfx::stroke(r.x, r.y, r.w, r.h, 22, WHITE, theme::EDGE_A);
    gfx::text(r.x + 30, r.y + 22, &TITLE, "Applications", TEXT);

    for (i, app) in APPS.iter().enumerate() {
        let cell = drawer_cell(i);
        let ready = app.window.is_some();
        let tx = cell.x + (CELL_W - BIG_TILE) / 2;
        draw_tile(tx, cell.y + 6, BIG_TILE, app, if ready { 255 } else { 70 });
        let color = if ready { TEXT } else { FAINT };
        gfx::text_center(cell.x + CELL_W / 2, cell.y + BIG_TILE + 16, &LABEL, app.name, color);
        if !ready {
            gfx::text_center(cell.x + CELL_W / 2, cell.y + BIG_TILE + 34, &SMALL, "Coming soon", FAINT);
        }
    }
}

/// Where things sit in the menu bar, shared by drawing and clicking.
struct Bar {
    /// Left edge of the app name. Everything left of it is the logo.
    name_x: i32,
    /// Left edge of the status icons.
    status_x: i32,
    /// Left edge of the date and time.
    clock_x: i32,
}

fn clock_text() -> alloc::string::String {
    let c = theme::cfg();
    let time = clock::time_string(c.show_seconds);
    if c.show_date {
        alloc::format!("{}   {}", clock::short_date(), time)
    } else {
        time
    }
}

fn bar_layout() -> Bar {
    let w = fb::width() as i32;
    let clock_x = w - 16 - gfx::text_w(&LABEL, &clock_text());
    Bar { name_x: 14 + MARK_22.w as i32 + 12, status_x: clock_x - 18 - 2 * 26, clock_x: clock_x - 9 }
}

fn draw_topbar(s: &State) {
    let w = fb::width() as i32;
    let c = theme::cfg();
    gfx::rect_a(0, 0, w, TOP_H, theme::BAR, theme::alpha(c.top_opacity));
    gfx::rect_a(0, TOP_H - 1, w, 1, WHITE, 16);

    let bar = bar_layout();
    gfx::image_a(14, (TOP_H - MARK_22.h as i32) / 2, &MARK_22, 255);

    let name = if s.count > 0 { s.windows[s.order[s.count - 1]].title } else { "Desktop" };
    let ty = (TOP_H - STRONG.height) / 2;
    gfx::text(bar.name_x, ty, &STRONG, name, TEXT);

    let iy = (TOP_H - 16) / 2;
    let wired = net::info().up;
    gfx::icon(bar.status_x, iy, image::icon16(icon::ETHERNET), if wired { TEXT } else { FAINT });
    gfx::icon(bar.status_x + 26, iy, image::icon16(icon::WIFI_OFF), FAINT);

    gfx::text_right(w - 16, (TOP_H - LABEL.height) / 2, &LABEL, &clock_text(), TEXT);
}

fn draw_window(s: &mut State, idx: usize, focused: bool) {
    let win = s.windows[idx];
    let Rect { x, y, w, h } = win.rect;
    let c = theme::cfg();
    let radius = if win.full { 0 } else { c.radius as i32 };
    let alpha = theme::alpha(c.window_opacity);

    if c.shadows && !win.full {
        gfx::shadow(x, y, w, h, radius, if focused { 46 } else { 26 });
    }
    gfx::round_frost(x, y, w, h, radius, theme::WINDOW, alpha, scene::frost(), fb::width() as i32);
    gfx::stroke(x, y, w, h, radius, WHITE, if focused { theme::EDGE_A } else { theme::LINE_A });

    // Clip the contents to the frame so nothing spills onto other windows.
    let prev = fb::current_clip();
    let cx0 = x.max(prev.0 as i32);
    let cy0 = y.max(prev.1 as i32);
    let cx1 = (x + w).min(prev.2 as i32);
    let cy1 = (y + h).min(prev.3 as i32);
    if cx1 <= cx0 || cy1 <= cy0 {
        return;
    }
    fb::set_clip(cx0 as u32, cy0 as u32, (cx1 - cx0) as u32, (cy1 - cy0) as u32);

    let dots = [0x00FF_5F57, 0x00FE_BC2E, 0x0028_C840];
    for n in 0..3 {
        let (dx, dy) = dot_center(win.rect, n);
        let color = if focused { dots[n as usize] } else { 0x0048_4B55 };
        gfx::round(dx - 6, dy - 6, 12, 12, 6, color);
    }
    let title_color = if focused { TEXT } else { MUTED };
    gfx::text_center(x + w / 2, y + (TITLE_H - STRONG.height) / 2, &STRONG, win.title, title_color);
    gfx::rect_a(x + 1, y + TITLE_H - 1, w - 2, 1, WHITE, 14);

    let body = body_of(win.rect);
    match win.kind {
        Kind::Welcome => welcome_content(body),
        Kind::Keys => keys_content(body),
        Kind::Terminal => terminal::draw(body.x, body.y, body.w, body.h),
        Kind::Settings => settings_app::draw(body, &mut s.settings),
    }

    if !win.full {
        draw_grip(win.rect);
    }
    fb::restore_clip(prev);
}

fn welcome_content(r: Rect) {
    let x = r.x + 32;
    let mut y = r.y + 26;
    gfx::image_a(x - 6, y, &WORDMARK, 255);
    y += WORDMARK.h as i32 + 14;
    gfx::text(x, y, &BODY, "A clean, fun operating system, built from the kernel up.", MUTED);
    y += 38;

    let hints = [
        (icon::GRID, "Open the app drawer from the first icon in the dock."),
        (icon::SETTINGS, "Change the theme, wallpaper and transparency in Settings."),
        (icon::CLOCK, "Click the clock or the logo in the menu bar for shortcuts."),
    ];
    for (ic, text) in hints {
        gfx::round_a(x, y, 28, 28, 8, WHITE, 22);
        gfx::icon(x + 6, y + 6, image::icon16(ic), theme::accent());
        gfx::text(x + 42, y + (28 - BODY.height) / 2, &BODY, text, TEXT);
        y += 40;
    }
}

fn keys_content(r: Rect) {
    let x = r.x + 24;
    let y = r.y + 22;
    gfx::text(x, y, &BODY, "Type on the keyboard. Enter clears the line.", MUTED);

    let mut buf = [0u8; ps2::TYPED_MAX];
    let n = ps2::copy_typed(&mut buf);
    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
    let field = Rect::new(x, y + 34, r.w - 48, 40);
    gfx::round_a(field.x, field.y, field.w, field.h, 9, WHITE, theme::CARD_A);
    gfx::stroke(field.x, field.y, field.w, field.h, 9, WHITE, theme::LINE_A);
    let end = gfx::text(field.x + 14, field.y + (40 - image::MONO.height) / 2, &image::MONO, text, TEXT);
    gfx::rect(end + 2, field.y + 11, 2, 18, theme::accent());
}

/// Three diagonal dots in the bottom-right corner: the resize handle.
fn draw_grip(rect: Rect) {
    let Rect { x, y, w, h } = rect;
    for i in 1..=3 {
        let d = i * 4 + 2;
        gfx::rect_a(x + w - d, y + h - 6, 2, 2, WHITE, 90);
        if i < 3 {
            gfx::rect_a(x + w - 6, y + h - d - 4, 2, 2, WHITE, 90);
        }
    }
    gfx::rect_a(x + w - 10, y + h - 10, 2, 2, WHITE, 90);
}

/// The pointer: '#' is the black outline, '.' is the white fill.
const CURSOR: [&[u8; 12]; 19] = [
    b"#           ",
    b"##          ",
    b"#.#         ",
    b"#..#        ",
    b"#...#       ",
    b"#....#      ",
    b"#.....#     ",
    b"#......#    ",
    b"#.......#   ",
    b"#........#  ",
    b"#.........# ",
    b"#..........#",
    b"#......#####",
    b"#...#..#    ",
    b"#..# #..#   ",
    b"#.#  #..#   ",
    b"##    #..#  ",
    b"#     #..#  ",
    b"       ##   ",
];

fn draw_cursor(x: i32, y: i32) {
    for (row, line) in CURSOR.iter().enumerate() {
        for (col, &c) in line.iter().enumerate() {
            let color = match c {
                b'#' => 0x0000_0000,
                b'.' => WHITE,
                _ => continue,
            };
            gfx::rect(x + col as i32, y + row as i32, 1, 1, color);
        }
    }
}
