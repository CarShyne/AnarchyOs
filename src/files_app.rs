//! The Files app: browse folders on the encrypted drive, go up, and make
//! new folders. Folders use their icon from assets/icons/folders when one
//! matches their name.

use crate::fs::{self, Entry};
use crate::gfx;
use crate::image::{self, icon, BODY, LABEL, SMALL, STRONG};
use crate::settings_app::Outcome;
use crate::theme::{self, FAINT, MUTED, TEXT, WHITE};
use crate::ui::Rect;
use alloc::format;
use alloc::string::String;

const PAD: i32 = 20;
const TOOL_H: i32 = 32;
const TOP_GAP: i32 = 14;
const FOOT_H: i32 = 40;
const CELL_W: i32 = 120;
const CELL_H: i32 = 112;
const ART: i32 = 64;

pub struct State {
    /// Empty means the root folder.
    cwd: String,
    page: usize,
    selected: Option<String>,
    note: String,
}

impl State {
    pub const fn new() -> State {
        State { cwd: String::new(), page: 0, selected: None, note: String::new() }
    }

    fn here(&self) -> &str {
        if self.cwd.is_empty() {
            "/"
        } else {
            &self.cwd
        }
    }

    fn go(&mut self, path: String) {
        self.cwd = path;
        self.page = 0;
        self.selected = None;
    }
}

struct Geo {
    up: Rect,
    new_folder: Rect,
    grid: Rect,
    grid_x: i32,
    cols: usize,
    rows: usize,
    prev: Rect,
    next: Rect,
    foot_y: i32,
}

impl Geo {
    fn per_page(&self) -> usize {
        self.cols * self.rows
    }

    fn cell(&self, k: usize) -> Rect {
        let col = (k % self.cols) as i32;
        let row = (k / self.cols) as i32;
        Rect::new(self.grid_x + col * CELL_W, self.grid.y + row * CELL_H, CELL_W, CELL_H)
    }
}

fn geo(body: Rect) -> Geo {
    let tool_y = body.y + TOP_GAP;
    let up = Rect::new(body.x + PAD, tool_y, 58, TOOL_H);
    let new_folder = Rect::new(body.x + body.w - PAD - 116, tool_y, 116, TOOL_H);
    let foot_y = body.y + body.h - FOOT_H;
    let grid_top = tool_y + TOOL_H + 12;
    let grid = Rect::new(body.x + PAD, grid_top, body.w - 2 * PAD, (foot_y - 8 - grid_top).max(CELL_H));
    let cols = (grid.w / CELL_W).max(1) as usize;
    let rows = (grid.h / CELL_H).max(1) as usize;
    let grid_x = grid.x + (grid.w - cols as i32 * CELL_W) / 2;
    let next = Rect::new(body.x + body.w - PAD - 64, foot_y + 6, 64, 28);
    let prev = Rect::new(next.x - 72, foot_y + 6, 64, 28);
    Geo { up, new_folder, grid, grid_x, cols, rows, prev, next, foot_y }
}

fn fit(s: &str, font: &crate::image::Font, max: i32) -> String {
    if gfx::text_w(font, s) <= max {
        return String::from(s);
    }
    let mut out = String::new();
    for c in s.chars() {
        out.push(c);
        if gfx::text_w(font, &format!("{}..", out)) > max {
            out.pop();
            break;
        }
    }
    out.push_str("..");
    out
}

fn button(r: Rect, label: &str, enabled: bool) {
    gfx::round_a(r.x, r.y, r.w, r.h, 9, WHITE, if enabled { 24 } else { 10 });
    gfx::stroke(r.x, r.y, r.w, r.h, 9, WHITE, theme::LINE_A);
    let color = if enabled { TEXT } else { FAINT };
    gfx::text_center(r.x + r.w / 2, r.y + (r.h - LABEL.height) / 2, &LABEL, label, color);
}

fn tile(x: i32, y: i32, size: i32, top: u32, bottom: u32, glyph: usize) {
    gfx::round_grad(x, y, size, size, size * 27 / 100, top, bottom, 255);
    gfx::stroke(x, y, size, size, size * 27 / 100, WHITE, 38);
    let mask = image::icon34(glyph);
    let off = (size - mask.size as i32) / 2;
    gfx::icon_a(x + off, y + off, mask, WHITE, 255);
}

fn draw_entry_icon(x: i32, y: i32, e: &Entry) {
    if e.dir {
        match image::folder_art(&e.name) {
            Some(art) => gfx::art_round(x, y, &art.s64, ART * 27 / 100, 255),
            None => tile(x, y, ART, 0x0060_A5FA, 0x001D_4ED8, icon::FOLDER),
        }
    } else {
        tile(x, y, ART, 0x008B_93A1, 0x004B_5563, icon::NOTES);
    }
}

fn size_text(bytes: u32) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else {
        format!("{} KiB", bytes / 1024)
    }
}

pub fn draw(body: Rect, st: &mut State) {
    let g = geo(body);
    let ty = g.up.y + (TOOL_H - LABEL.height) / 2;
    button(g.up, "Up", st.here() != "/");
    let path = fit(st.here(), &STRONG, g.new_folder.x - g.up.x - g.up.w - 40);
    gfx::text(g.up.x + g.up.w + 14, ty, &STRONG, &path, TEXT);
    button(g.new_folder, "New folder", true);

    if !fs::mounted() {
        gfx::text_center(body.x + body.w / 2, body.y + body.h / 2, &BODY, "The drive is locked.", MUTED);
        return;
    }
    let entries = match fs::list(st.here()) {
        Some(e) => e,
        None => {
            gfx::text_center(body.x + body.w / 2, body.y + body.h / 2, &BODY, "This folder cannot be opened.", MUTED);
            return;
        }
    };
    if entries.is_empty() {
        gfx::text_center(body.x + body.w / 2, g.grid.y + 30, &BODY, "This folder is empty.", MUTED);
    }

    let per = g.per_page();
    let pages = ((entries.len() + per - 1) / per).max(1);
    st.page = st.page.min(pages - 1);
    for (k, e) in entries.iter().skip(st.page * per).take(per).enumerate() {
        let cell = g.cell(k);
        if st.selected.as_deref() == Some(e.name.as_str()) {
            gfx::round_a(cell.x + 6, cell.y + 2, cell.w - 12, cell.h - 6, 14, theme::accent(), 40);
        }
        let ix = cell.x + (CELL_W - ART) / 2;
        draw_entry_icon(ix, cell.y + 10, e);
        let label = fit(&e.name, &LABEL, CELL_W - 12);
        gfx::text_center(cell.x + CELL_W / 2, cell.y + 10 + ART + 12, &LABEL, &label, TEXT);
        if !e.dir {
            gfx::text_center(cell.x + CELL_W / 2, cell.y + 10 + ART + 30, &SMALL, &size_text(e.size), FAINT);
        }
    }

    let free = fs::total_bytes().saturating_sub(fs::used_bytes());
    let info = if !st.note.is_empty() {
        st.note.clone()
    } else if let Some(name) = st.selected.as_ref().and_then(|n| entries.iter().find(|e| &e.name == n)) {
        format!("{} selected, {}", name.name, size_text(name.size))
    } else {
        format!("{} items, {} KiB free", entries.len(), free / 1024)
    };
    gfx::text(body.x + PAD, g.foot_y + (FOOT_H - SMALL.height) / 2 + 2, &SMALL, &info, MUTED);

    if pages > 1 {
        button(g.prev, "Prev", st.page > 0);
        button(g.next, "Next", st.page + 1 < pages);
        let label = format!("{} / {}", st.page + 1, pages);
        gfx::text_right(g.prev.x - 10, g.foot_y + (FOOT_H - SMALL.height) / 2 + 2, &SMALL, &label, MUTED);
    }
}

/// Create "New Folder", or the first "New Folder N" that is free.
fn new_folder(st: &mut State) -> Result<(), &'static str> {
    for n in 1..100 {
        let label = if n == 1 { String::from("New Folder") } else { format!("New Folder {}", n) };
        let path = fs::normalize(st.here(), &label);
        if !fs::exists(&path) {
            return fs::mkdir(&path);
        }
    }
    Err("no free name for a new folder")
}

/// Mouse button went down at (mx, my) inside the Files body.
pub fn press(body: Rect, st: &mut State, mx: i32, my: i32) -> Outcome {
    if !fs::mounted() {
        return Outcome::Nothing;
    }
    let g = geo(body);
    if g.up.contains(mx, my) {
        if st.here() != "/" {
            let up = fs::normalize(st.here(), "..");
            st.go(up);
        }
        st.note.clear();
        return Outcome::Window;
    }
    if g.new_folder.contains(mx, my) {
        st.note = match new_folder(st) {
            Ok(()) => String::new(),
            Err(e) => String::from(e),
        };
        return Outcome::Window;
    }
    let entries = match fs::list(st.here()) {
        Some(e) => e,
        None => return Outcome::Nothing,
    };
    let per = g.per_page();
    let pages = ((entries.len() + per - 1) / per).max(1);
    if pages > 1 {
        if g.prev.contains(mx, my) && st.page > 0 {
            st.page -= 1;
            return Outcome::Window;
        }
        if g.next.contains(mx, my) && st.page + 1 < pages {
            st.page += 1;
            return Outcome::Window;
        }
    }
    for (k, e) in entries.iter().skip(st.page * per).take(per).enumerate() {
        if g.cell(k).contains(mx, my) {
            if e.dir {
                let path = fs::normalize(st.here(), &e.name);
                st.go(path);
            } else {
                st.selected = if st.selected.as_deref() == Some(e.name.as_str()) {
                    None
                } else {
                    Some(e.name.clone())
                };
            }
            st.note.clear();
            return Outcome::Window;
        }
    }
    Outcome::Nothing
}
