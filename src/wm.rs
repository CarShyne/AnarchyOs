//! Window manager and desktop shell: the menu bar, the dock, the app
//! drawer, and the windows in between.
//!
//! Drawing is painter's style. Any change repaints a damaged rectangle
//! from the wallpaper up into the back buffer, then copies that rectangle
//! to the screen, so nothing ever leaves a trail.

use crate::anarchyai;
use crate::clock;
use crate::files_app;
use crate::framebuffer as fb;
use crate::gfx;
use crate::image::{self, icon, BODY, LABEL, MARK_22, MARK_48, SMALL, STRONG, TITLE, WORDMARK};
use crate::lock;
use crate::net;
use crate::pci;
use crate::ps2;
use crate::scene;
use crate::settings;
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
    Files,
    AnarchyAi,
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
    /// Name of a full-colour icon in assets/icons/apps. Empty uses the gradient tile.
    art: &'static str,
    /// Tile gradient, top and bottom.
    top: u32,
    bottom: u32,
}

const WELCOME: usize = 0;
const KEYS: usize = 1;
const TERMINAL: usize = 2;
const SETTINGS: usize = 3;
const FILES: usize = 4;
const ANARCHYAI: usize = 5;
const MAX_WINDOWS: usize = 6;

const TITLE_H: i32 = 36;
const GRIP: i32 = 18;
/// How far a window's shadow reaches past its frame.
const SHADOW: i32 = 13;

const TOP_H: i32 = 30;

/// Most apps the dock can hold, pinned or running, not counting the launcher.
const DOCK_MAX: usize = 8;
const TILE: i32 = 48;
const DOCK_PAD: i32 = 10;
const TILE_GAP: i32 = 10;
const DOCK_H: i32 = TILE + 2 * DOCK_PAD + 4;
const DOCK_BOTTOM_GAP: i32 = 12;
/// How far a press may move before it counts as a drag, not a click.
const DRAG_START: i32 = 6;
/// How far above the dock's top edge a dragged icon must be to be removed.
const DROP_REACH: i32 = 28;
const MENU_W: i32 = 196;
const MENU_ROW: i32 = 30;
const MENU_PAD: i32 = 6;

const DRAWER_W: i32 = 600;
const DRAWER_H: i32 = 420;
const DRAWER_COLS: usize = 4;
const CELL_W: i32 = 132;
const CELL_H: i32 = 116;
const BIG_TILE: i32 = 64;

const CURSOR_W: i32 = 12;
const CURSOR_H: i32 = 19;
/// Timer ticks between cursor blinks: 50 ticks at 100 Hz is half a second.
const BLINK_TICKS: u64 = 50;

/// Dock magnification: how much an icon can grow, and how far from it the
/// pointer still reaches.
const MAG_GROW: i32 = 20;
const MAG_RANGE: i32 = 90;
/// How far a hovered icon lifts.
const HOVER_LIFT: i32 = 6;
/// How far above the dock its effects can reach. Damage uses this too.
const BAND_UP: i32 = 48;
/// The launch bounce moves one step every BOUNCE_STEP ticks, in pixels up.
const BOUNCE_STEP: u64 = 5;
const BOUNCE: [i32; 16] = [0, 6, 12, 18, 22, 24, 22, 18, 12, 8, 4, 8, 4, 2, 1, 0];
const BOUNCE_TICKS: u64 = BOUNCE_STEP * BOUNCE.len() as u64;

/// Every app, in drawer order. An app with no window is shown but not ready yet.
const APPS: [App; 9] = [
    App { name: "Welcome", window: Some(WELCOME), icon: icon::SPARKLES, art: "", top: 0x00FF_4D6D, bottom: 0x00C9_0A38 },
    App { name: "Keys", window: Some(KEYS), icon: icon::KEYBOARD, art: "", top: 0x002D_D4BF, bottom: 0x000F_766E },
    App { name: "Terminal", window: Some(TERMINAL), icon: icon::TERMINAL, art: "", top: 0x003A_3D46, bottom: 0x0016_171C },
    App { name: "Settings", window: Some(SETTINGS), icon: icon::SETTINGS, art: "", top: 0x008B_93A1, bottom: 0x004B_5563 },
    App { name: "Files", window: Some(FILES), icon: icon::FOLDER, art: "Finder", top: 0x0060_A5FA, bottom: 0x001D_4ED8 },
    App { name: "Monitor", window: None, icon: icon::ACTIVITY, art: "Task manager", top: 0x004A_DE80, bottom: 0x0015_803D },
    App { name: "Network", window: None, icon: icon::GLOBE, art: "Connections", top: 0x0038_BDF8, bottom: 0x0003_69A1 },
    App { name: "Notes", window: None, icon: icon::NOTES, art: "Lightbulb", top: 0x00FB_BF24, bottom: 0x00B4_5309 },
    App { name: "AnarchyAi", window: Some(ANARCHYAI), icon: icon::SPARKLES, art: "AnarchyAi", top: 0x00FF_1F4B, bottom: 0x00A0_0C2E },
];

/// Apps pinned to the dock by default, as indexes into APPS.
const DEFAULT_PINS: [usize; DOCK_MAX] = [4, 0, 2, 3, 0, 0, 0, 0];
const DEFAULT_PIN_COUNT: usize = 4;

/// A press on the dock, held until the button is released. It is a click
/// if the pointer stayed put, and a drag if it moved.
#[derive(Clone, Copy)]
struct DockPress {
    /// Which dock slot was pressed. Slot 0 is the app drawer button.
    slot: usize,
    /// The app under the press, or None for the app drawer button.
    app: Option<usize>,
    origin_x: i32,
    origin_y: i32,
    x: i32,
    y: i32,
    moved: bool,
}

/// The right-click menu for a dock icon, opened at the pointer.
#[derive(Clone, Copy)]
struct Menu {
    app: usize,
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Open,
    Hide,
    Pin,
    Unpin,
    Close,
}

fn app_window(app: usize) -> usize {
    APPS[app].window.unwrap_or(0)
}

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
    files: files_app::State,
    anarchyai: anarchyai::State,
    /// Last clock value drawn in the menu bar.
    clock_stamp: u64,
    /// The latest timer tick, used by the animations.
    now: u64,
    /// A dock icon is bouncing: its slot, and the tick it started on.
    bounce: Option<(usize, u64)>,
    /// Apps pinned to the dock, in order, as indexes into APPS.
    pins: [usize; DOCK_MAX],
    pin_count: usize,
    dock_press: Option<DockPress>,
    menu: Option<Menu>,
    right: bool,
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
        window(Rect::new(300, 110, 640, 420), "Files", Kind::Files, false, 420, 300),
        window(Rect::new(200, 96, 900, 600), "AnarchyAi", Kind::AnarchyAi, false, 560, 380),
    ],
    order: [WELCOME, 0, 0, 0, 0, 0],
    count: 1,
    cur_x: 0,
    cur_y: 0,
    left: false,
    drag: None,
    resizing: None,
    in_settings: false,
    drawer: false,
    settings: settings_app::State::new(),
    files: files_app::State::new(),
    anarchyai: anarchyai::State::new(),
    clock_stamp: 0,
    now: 0,
    bounce: None,
    pins: DEFAULT_PINS,
    pin_count: DEFAULT_PIN_COUNT,
    dock_press: None,
    menu: None,
    right: false,
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
    if !lock::showing() {
        let s = state();
        s.now = t;
        if let Some((_, start)) = s.bounce {
            let done = t.wrapping_sub(start) >= BOUNCE_TICKS;
            if done {
                s.bounce = None;
            }
            // Repaint the dock on each bounce step, and once more when it lands.
            if t % BOUNCE_STEP == 0 || done {
                render(s, dock_band(s));
            }
        }
    }
    if t % BLINK_TICKS != 0 || lock::showing() {
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
/// `buttons` has bit 0 for the left button and bit 1 for the right one.
pub fn mouse(dx: i32, dy: i32, buttons: u8) {
    let s = state();
    let x = (s.cur_x + dx).clamp(0, fb::width() as i32 - 1);
    let y = (s.cur_y - dy).clamp(0, fb::height() as i32 - 1);
    pointer(x, y, buttons & 1 != 0, buttons & 2 != 0);
}

fn pointer(x: i32, y: i32, left: bool, right: bool) {
    let s = state();
    if lock::showing() {
        s.cur_x = x;
        s.cur_y = y;
        s.left = left;
        s.right = right;
        s.menu = None;
        s.dock_press = None;
        render(s, full_damage());
        return;
    }
    let mut damage = cursor_rect(s.cur_x, s.cur_y).union(cursor_rect(x, y));
    {
        // Icons and menu items that react to the pointer must repaint
        // when it moves into or out of their area.
        let c = theme::cfg();
        let (old_x, old_y) = (s.cur_x, s.cur_y);
        let dock_fx = c.dock_magnify || c.dock_hover;
        if dock_fx && (dock_band(s).contains(old_x, old_y) || dock_band(s).contains(x, y)) {
            damage = damage.union(dock_band(s));
        }
        if c.bar_hover && (old_y < TOP_H || y < TOP_H) {
            damage = damage.union(top_rect());
        }
    }
    s.cur_x = x;
    s.cur_y = y;
    let pressed = left && !s.left;
    let right_pressed = right && !s.right;
    s.left = left;
    s.right = right;

    // An open menu closes on any press. A press on one of its rows runs that
    // row. A press anywhere else still goes on to the normal handling below.
    if let Some(menu) = s.menu {
        damage = damage.union(menu_rect(s, menu));
        if pressed || right_pressed {
            s.menu = None;
        }
        if pressed {
            if let Some(action) = menu_action_at(s, menu, x, y) {
                run_action(s, menu.app, action);
                render(s, full_damage());
                return;
            }
        }
    }

    // A dock press in progress: follow the pointer until the button is up.
    if let Some(prev) = s.dock_press {
        let mut dp = prev;
        track_dock_press(&mut dp, x, y);
        if left {
            damage = damage.union(dock_band(s)).union(ghost_rect(prev.x, prev.y)).union(ghost_rect(x, y));
            s.dock_press = Some(dp);
        } else {
            s.dock_press = None;
            damage = damage.union(finish_dock_press(s, dp));
        }
        render(s, damage);
        return;
    }

    if right_pressed {
        if let Some(slot) = dock_slot_at(s, x, y) {
            if let Some(app) = dock_app(s, slot) {
                let menu = Menu { app, x, y };
                s.menu = Some(menu);
                damage = damage.union(menu_rect(s, menu));
                render(s, damage);
                return;
            }
        }
    }

    if pressed {
        if let Some(slot) = dock_slot_at(s, x, y) {
            let app = dock_app(s, slot);
            // Pressing an app leaves the app drawer; the launcher toggles it on release.
            if app.is_some() {
                s.drawer = false;
            }
            let mut dp = DockPress { slot, app, origin_x: x, origin_y: y, x, y, moved: false };
            track_dock_press(&mut dp, x, y);
            s.dock_press = Some(dp);
            damage = damage.union(dock_band(s));
            render(s, damage);
            return;
        }
    }

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
    if lock::showing() {
        if lock::key(c) {
            lock::set_busy(true);
            render(s, full_damage());
            let outcome = lock::submit();
            lock::set_busy(false);
            if outcome == lock::Outcome::Unlocked {
                unlocked(s);
                return;
            }
        }
        render(s, full_damage());
        return;
    }
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
        Kind::AnarchyAi => anarchyai::key(&mut s.anarchyai, c),
        Kind::Welcome | Kind::Settings | Kind::Files => return,
    }
    render(s, s.windows[idx].rect);
}

/// The drive opened: load the saved settings and redraw the desktop.
fn unlocked(s: &mut State) {
    settings::load();
    load_dock(s);
    theme::load();
    scene::apply();
    terminal::print("Drive unlocked.");
    render(s, full_damage());
}

/// Handle a mouse press. Returns the area that needs repainting.
fn click(s: &mut State, x: i32, y: i32) -> Option<Rect> {
    let full = Some(full_damage());

    if s.drawer {
        if let Some(a) = drawer_app_at(s, x, y) {
            s.drawer = false;
            if let Some(w) = APPS[a].window {
                show(s, w);
            }
            return full;
        }
        if drawer_rect(s).contains(x, y) {
            return None;
        }
        if dock_slot_at(s, x, y).is_none() {
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

    if dock_rect(s).contains(x, y) {
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
        let outcome = settings_app::press(body_of(rect), &mut s.settings, x, y);
        if s.settings.take_lock_request() {
            lock::lock_now();
            return Some(full_damage());
        }
        if outcome == Outcome::Everything {
            damage = full_damage();
        }
    } else if idx == FILES {
        files_app::press(body_of(rect), &mut s.files, x, y);
    } else if idx == ANARCHYAI {
        anarchyai::press(body_of(rect), &mut s.anarchyai, x, y);
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

/// The dock's apps after the launcher: pinned ones in order, then the
/// running apps that are not pinned, so an open app stays visible until it closes.
fn dock_items(s: &State) -> ([usize; DOCK_MAX], usize) {
    let mut items = [0usize; DOCK_MAX];
    let mut n = 0;
    for i in 0..s.pin_count {
        items[n] = s.pins[i];
        n += 1;
    }
    for app in 0..APPS.len() {
        if n == DOCK_MAX {
            break;
        }
        let running = match APPS[app].window {
            Some(win) => s.windows[win].open,
            None => false,
        };
        if running && !is_pinned(s, app) {
            items[n] = app;
            n += 1;
        }
    }
    (items, n)
}

/// Dock slots in use, counting the launcher in slot 0.
fn dock_slots(s: &State) -> usize {
    1 + dock_items(s).1
}

/// The app in a dock slot, or None for the launcher.
fn dock_app(s: &State, slot: usize) -> Option<usize> {
    if slot == 0 {
        return None;
    }
    let (items, _) = dock_items(s);
    Some(items[slot - 1])
}

fn is_pinned(s: &State, app: usize) -> bool {
    s.pins[..s.pin_count].contains(&app)
}

fn dock_rect(s: &State) -> Rect {
    let n = dock_slots(s) as i32;
    let w = DOCK_PAD * 2 + n * TILE + (n - 1) * TILE_GAP;
    let screen = full_damage();
    Rect::new((screen.w - w) / 2, screen.h - DOCK_H - DOCK_BOTTOM_GAP, w, DOCK_H)
}

fn tile_rect(s: &State, slot: usize) -> Rect {
    let d = dock_rect(s);
    Rect::new(d.x + DOCK_PAD + slot as i32 * (TILE + TILE_GAP), d.y + DOCK_PAD, TILE, TILE)
}

fn dock_slot_at(s: &State, x: i32, y: i32) -> Option<usize> {
    (0..dock_slots(s)).find(|&i| tile_rect(s, i).contains(x, y))
}

/// The dock and the space above it that its icon effects can reach into.
fn dock_band(s: &State) -> Rect {
    let d = dock_rect(s);
    Rect::new(d.x - 24, d.y - BAND_UP, d.w + 48, d.h + BAND_UP)
}

fn track_dock_press(dp: &mut DockPress, x: i32, y: i32) {
    dp.x = x;
    dp.y = y;
    if (x - dp.origin_x).abs() > DRAG_START || (y - dp.origin_y).abs() > DRAG_START {
        dp.moved = true;
    }
}

/// The icon-sized box that follows the pointer while a dock icon is dragged.
fn ghost_rect(x: i32, y: i32) -> Rect {
    Rect::new(x - TILE / 2, y - TILE / 2, TILE, TILE)
}

/// A dragged pinned icon that is dropped above the dock takes the app off it.
fn will_remove(s: &State, dp: DockPress) -> bool {
    match dp.app {
        Some(app) => dp.moved && is_pinned(s, app) && dp.y < dock_rect(s).y - DROP_REACH,
        None => false,
    }
}

/// Finish a dock press once the button is up. Returns the area to repaint.
fn finish_dock_press(s: &mut State, dp: DockPress) -> Rect {
    if dp.moved {
        if will_remove(s, dp) {
            if let Some(app) = dp.app {
                unpin(s, app);
            }
        }
        return full_damage();
    }
    match dp.app {
        None => s.drawer = !s.drawer,
        Some(app) => {
            let win = app_window(app);
            // Only an app that is being opened bounces, not one tucked away.
            if theme::cfg().dock_bounce && visible(s, win).is_none() {
                s.bounce = Some((dp.slot, s.now));
            }
            dock_click(s, win);
        }
    }
    full_damage()
}

fn pin(s: &mut State, app: usize) {
    if is_pinned(s, app) || s.pin_count == DOCK_MAX {
        return;
    }
    s.pins[s.pin_count] = app;
    s.pin_count += 1;
    save_dock(s);
}

fn unpin(s: &mut State, app: usize) {
    let found = s.pins[..s.pin_count].iter().position(|&a| a == app);
    if let Some(p) = found {
        for i in p..s.pin_count - 1 {
            s.pins[i] = s.pins[i + 1];
        }
        s.pin_count -= 1;
        save_dock(s);
    }
}

/// The dock layout is kept on the drive, so it survives a reboot.
fn save_dock(s: &State) {
    let mut text = alloc::string::String::new();
    for i in 0..s.pin_count {
        if i > 0 {
            text.push(',');
        }
        text.push_str(&alloc::format!("{}", s.pins[i]));
    }
    settings::set("dock", &text);
    settings::flush_quietly();
}

/// Read the pinned apps from settings. Without a saved list, the defaults stay.
fn load_dock(s: &mut State) {
    let text = match settings::get("dock") {
        Some(text) => text,
        None => {
            s.pins = DEFAULT_PINS;
            s.pin_count = DEFAULT_PIN_COUNT;
            return;
        }
    };
    s.pin_count = 0;
    for part in text.split(',') {
        let app = match part.trim().parse::<usize>() {
            Ok(app) => app,
            Err(_) => continue,
        };
        let usable = app < APPS.len() && APPS[app].window.is_some();
        if usable && !is_pinned(s, app) && s.pin_count < DOCK_MAX {
            s.pins[s.pin_count] = app;
            s.pin_count += 1;
        }
    }
}

fn menu_items(s: &State, app: usize) -> ([(Action, &'static str); 3], usize) {
    let win = app_window(app);
    let mut items = [(Action::Open, ""); 3];
    let mut n = 0;
    items[n] = if visible(s, win).is_some() {
        (Action::Hide, "Hide")
    } else {
        (Action::Open, "Open")
    };
    n += 1;
    items[n] = if is_pinned(s, app) {
        (Action::Unpin, "Remove from dock")
    } else {
        (Action::Pin, "Keep in dock")
    };
    n += 1;
    if s.windows[win].open {
        items[n] = (Action::Close, "Close window");
        n += 1;
    }
    (items, n)
}

fn menu_rect(s: &State, m: Menu) -> Rect {
    let (_, n) = menu_items(s, m.app);
    let h = 2 * MENU_PAD + n as i32 * MENU_ROW;
    let screen = full_damage();
    let x = (m.x - MENU_W / 2).clamp(8, (screen.w - MENU_W - 8).max(8));
    let y = (m.y - h - 12).max(TOP_H + 8);
    Rect::new(x, y, MENU_W, h)
}

/// The menu row under a point, if any.
fn menu_row_at(s: &State, m: Menu, x: i32, y: i32) -> Option<usize> {
    let r = menu_rect(s, m);
    if !r.contains(x, y) {
        return None;
    }
    let dy = y - r.y - MENU_PAD;
    if dy < 0 {
        return None;
    }
    let row = dy as usize / MENU_ROW as usize;
    let (_, n) = menu_items(s, m.app);
    if row < n {
        Some(row)
    } else {
        None
    }
}

fn menu_action_at(s: &State, m: Menu, x: i32, y: i32) -> Option<Action> {
    let row = menu_row_at(s, m, x, y)?;
    let (items, _) = menu_items(s, m.app);
    Some(items[row].0)
}

fn run_action(s: &mut State, app: usize, action: Action) {
    let win = app_window(app);
    match action {
        Action::Open => show(s, win),
        Action::Hide => hide(s, win),
        Action::Pin => pin(s, app),
        Action::Unpin => unpin(s, app),
        Action::Close => close(s, win),
    }
}

/// Size and lift of a dock icon right now. Magnification grows an icon
/// near the pointer, hover lifts the icon under it, and a launch bounce
/// lifts the icon that was opened.
fn tile_look(s: &State, slot: usize) -> (i32, i32) {
    let c = theme::cfg();
    let t = tile_rect(s, slot);
    let (px, py) = (s.cur_x, s.cur_y);
    let mut size = TILE;
    let mut lift = 0;
    if c.dock_magnify && dock_band(s).contains(px, py) {
        let dx = (px - (t.x + TILE / 2)).abs();
        if dx < MAG_RANGE {
            size += MAG_GROW * (MAG_RANGE - dx) / MAG_RANGE;
        }
    }
    if c.dock_hover && t.contains(px, py) {
        lift += HOVER_LIFT;
    }
    if let Some((bounce_slot, start)) = s.bounce {
        if bounce_slot == slot {
            let step = (s.now.wrapping_sub(start) / BOUNCE_STEP) as usize;
            lift += BOUNCE[step.min(BOUNCE.len() - 1)];
        }
    }
    (size, lift)
}

fn drawer_rect(s: &State) -> Rect {
    let d = dock_rect(s);
    let screen = full_damage();
    Rect::new((screen.w - DRAWER_W) / 2, d.y - 14 - DRAWER_H, DRAWER_W, DRAWER_H)
}

fn drawer_cell(s: &State, i: usize) -> Rect {
    let r = drawer_rect(s);
    let cols = DRAWER_COLS as i32;
    let gx = (DRAWER_W - cols * CELL_W) / 2;
    Rect::new(
        r.x + gx + (i as i32 % cols) * CELL_W,
        r.y + 64 + (i as i32 / cols) * CELL_H,
        CELL_W,
        CELL_H,
    )
}

fn drawer_app_at(s: &State, x: i32, y: i32) -> Option<usize> {
    (0..APPS.len()).find(|&i| drawer_cell(s, i).contains(x, y) && APPS[i].window.is_some())
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

    if lock::showing() {
        lock::draw(screen);
    } else {
        scene::draw_background();
        for pos in 0..s.count {
            let idx = s.order[pos];
            if s.windows[idx].rect.grow(SHADOW).overlaps(r) {
                draw_window(s, idx, pos + 1 == s.count);
            }
        }
        draw_dock(s);
        draw_drawer(s);
        draw_ghost(s);
        draw_menu(s);
        draw_topbar(s);
    }
    draw_cursor(s.cur_x, s.cur_y);

    fb::reset_clip();
    fb::present(r.x as u32, r.y as u32, r.w as u32, r.h as u32);
}

/// A rounded app icon: a shaded tile with a white symbol on it.
fn draw_tile(x: i32, y: i32, size: i32, app: &App, alpha: u32) {
    if let Some(art) = image::app_art(app.art) {
        let img = if size > TILE { &art.s64 } else { &art.s48 };
        gfx::art_scaled(x, y, img, size, size * 27 / 100, alpha);
        return;
    }
    gfx::round_grad(x, y, size, size, size * 27 / 100, app.top, app.bottom, alpha);
    gfx::stroke(x, y, size, size, size * 27 / 100, WHITE, 38 * alpha / 255);
    let mask = if size >= 56 { image::icon34(app.icon) } else { image::icon26(app.icon) };
    let off = (size - mask.size as i32) / 2;
    gfx::icon_a(x + off, y + off, mask, WHITE, alpha);
}

fn draw_dock(s: &State) {
    let d = dock_rect(s);
    let c = theme::cfg();
    gfx::round_a(d.x, d.y, d.w, d.h, 20, theme::BAR, theme::alpha(c.dock_opacity));
    gfx::stroke(d.x, d.y, d.w, d.h, 20, WHITE, theme::EDGE_A);

    let (items, _) = dock_items(s);
    for slot in 0..dock_slots(s) {
        let t = tile_rect(s, slot);
        let (size, lift) = tile_look(s, slot);
        // Icons grow upward from the dock's baseline, centred on the slot.
        let x = t.x + TILE / 2 - size / 2;
        let y = t.y + TILE - size - lift;
        if slot == 0 {
            if size == TILE && lift == 0 {
                gfx::image_a(t.x, t.y, &MARK_48, 255);
            } else {
                gfx::art_scaled(x, y, &MARK_48, size, 0, 255);
            }
            if s.drawer {
                gfx::round(t.x + TILE / 2 - 8, d.y + d.h - 6, 16, 3, 1, theme::accent());
            }
        } else {
            let app = items[slot - 1];
            // The icon being dragged out is shown under the pointer instead.
            if let Some(dp) = s.dock_press {
                if dp.app == Some(app) && will_remove(s, dp) {
                    continue;
                }
            }
            let win = app_window(app);
            draw_tile(x, y, size, &APPS[app], 255);
            if s.windows[win].open {
                let on_screen = visible(s, win).is_some();
                let color = if on_screen { WHITE } else { FAINT };
                gfx::round(t.x + TILE / 2 - 2, d.y + d.h - 7, 4, 4, 2, color);
            }
        }
    }
}

/// The icon that follows the pointer while a dock icon is being dragged.
fn draw_ghost(s: &State) {
    let Some(dp) = s.dock_press else { return };
    let Some(app) = dp.app else { return };
    if !dp.moved {
        return;
    }
    let alpha = if will_remove(s, dp) { 255 } else { 170 };
    let g = ghost_rect(dp.x, dp.y);
    draw_tile(g.x, g.y, TILE, &APPS[app], alpha);
}

/// The right-click menu for a dock icon.
fn draw_menu(s: &State) {
    let Some(m) = s.menu else { return };
    let r = menu_rect(s, m);
    let (items, n) = menu_items(s, m.app);
    let hover = menu_row_at(s, m, s.cur_x, s.cur_y);
    let alpha = theme::alpha(theme::cfg().dock_opacity).max(215);
    gfx::shadow(r.x, r.y, r.w, r.h, 16, 40);
    gfx::round_a(r.x, r.y, r.w, r.h, 16, theme::BAR, alpha);
    gfx::stroke(r.x, r.y, r.w, r.h, 16, WHITE, theme::EDGE_A);
    for i in 0..n {
        let row = Rect::new(r.x + 6, r.y + MENU_PAD + i as i32 * MENU_ROW, r.w - 12, MENU_ROW);
        let hot = hover == Some(i);
        let color = if hot {
            gfx::round_a(row.x, row.y, row.w, row.h, 8, theme::accent(), 220);
            WHITE
        } else {
            TEXT
        };
        gfx::text(row.x + 12, row.y + 6, &LABEL, items[i].1, color);
    }
}

fn draw_drawer(s: &State) {
    if !s.drawer {
        return;
    }
    let r = drawer_rect(s);
    let alpha = theme::alpha(theme::cfg().dock_opacity).max(215);
    gfx::shadow(r.x, r.y, r.w, r.h, 22, 40);
    gfx::round_a(r.x, r.y, r.w, r.h, 22, theme::BAR, alpha);
    gfx::stroke(r.x, r.y, r.w, r.h, 22, WHITE, theme::EDGE_A);
    gfx::text(r.x + 30, r.y + 22, &TITLE, "Applications", TEXT);

    for (i, app) in APPS.iter().enumerate() {
        let cell = drawer_cell(s, i);
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
    if c.bar_hover && s.cur_y < TOP_H {
        // A soft pill behind the part of the menu bar under the pointer.
        let zones = [
            Rect::new(4, 4, bar.name_x - 14, TOP_H - 8),
            Rect::new(bar.status_x - 8, 4, 58, TOP_H - 8),
            Rect::new(bar.clock_x - 8, 4, w - bar.clock_x - 4, TOP_H - 8),
        ];
        for z in zones {
            if z.contains(s.cur_x, s.cur_y) {
                gfx::round_a(z.x, z.y, z.w, z.h, 8, WHITE, 26);
            }
        }
    }
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
        let base = if focused { 46 } else { 26 };
        gfx::shadow(x, y, w, h, radius, base * c.shadow_strength / 100);
    }
    if c.focus_glow && focused && !win.full {
        // An accent glow that fades out away from the window.
        for k in 1..=5i32 {
            gfx::stroke(x - k, y - k, w + 2 * k, h + 2 * k, radius + k, theme::accent(), (90 / k) as u32);
        }
    }
    let tint = gfx::mix(theme::WINDOW, theme::accent(), c.tint * 255 / 100);
    gfx::round_frost(x, y, w, h, radius, tint, alpha, scene::frost(), fb::width() as i32);
    gfx::stroke(x, y, w, h, radius, WHITE, if focused { theme::EDGE_A } else { theme::LINE_A });
    if c.border_width > 0 && (focused || !c.border_focus_only) && !win.full {
        for i in 0..c.border_width as i32 {
            gfx::stroke(x + i, y + i, w - 2 * i, h - 2 * i, (radius - i).max(0), theme::border_colour(), 255);
        }
    }

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
        Kind::Files => files_app::draw(body, &mut s.files),
        Kind::AnarchyAi => anarchyai::draw(body, &mut s.anarchyai),
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
