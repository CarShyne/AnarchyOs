//! Small widget kit for app windows: cards with rows of labels, switches,
//! sliders and buttons.
//!
//! It is immediate mode. A page is one function that lists its widgets, and
//! that same function runs to draw the page and to handle the mouse. Each
//! widget draws itself in the Draw phase and checks the pointer otherwise,
//! so what you see and what you can click can never drift apart.

use crate::gfx;
use crate::image::{self, icon, BODY, LABEL, SMALL, TITLE};
use crate::theme::{self, CARD_A, FAINT, LINE_A, MUTED, TEXT, WHITE};

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn union(self, o: Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    pub fn overlaps(self, o: Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    /// Grow by `d` on every side (shrink when negative).
    pub fn grow(self, d: i32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2 * d, self.h + 2 * d)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Phase {
    Draw,
    /// The mouse button just went down.
    Press,
    /// The button is held and the pointer moved.
    Drag,
}

pub const ROW_H: i32 = 44;
const PAD: i32 = 16;
const CARD_R: i32 = 10;
const CARD_GAP: i32 = 18;
const SLIDER_W: i32 = 200;

pub struct Ui<'a> {
    pub phase: Phase,
    pub mx: i32,
    pub my: i32,
    /// Left edge and width of the content column.
    pub x: i32,
    pub w: i32,
    /// Where the next widget goes.
    pub y: i32,
    /// The slider being dragged, kept by the caller between events.
    pub active: &'a mut Option<u32>,
    /// Set when a widget changed a value during this pass.
    pub changed: bool,
    next_id: u32,
    rows_left: u32,
    first_row: bool,
}

impl<'a> Ui<'a> {
    pub fn new(phase: Phase, mx: i32, my: i32, area: Rect, active: &'a mut Option<u32>) -> Ui<'a> {
        Ui {
            phase,
            mx,
            my,
            x: area.x,
            w: area.w,
            y: area.y,
            active,
            changed: false,
            next_id: 0,
            rows_left: 0,
            first_row: true,
        }
    }

    pub fn drawing(&self) -> bool {
        self.phase == Phase::Draw
    }

    /// True when the button went down inside `r` on this pass.
    pub fn pressed(&self, r: Rect) -> bool {
        self.phase == Phase::Press && r.contains(self.mx, self.my)
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    pub fn gap(&mut self, h: i32) {
        self.y += h;
    }

    /// Page title with a one-line description under it.
    pub fn heading(&mut self, title: &str, subtitle: &str) {
        if self.drawing() {
            gfx::text(self.x, self.y, &TITLE, title, TEXT);
            gfx::text(self.x, self.y + 32, &BODY, subtitle, MUTED);
        }
        self.y += 68;
    }

    /// Small caption above a card.
    pub fn section(&mut self, label: &str) {
        if self.drawing() {
            gfx::text(self.x + 4, self.y, &SMALL, label, FAINT);
        }
        self.y += 22;
    }

    /// A line of quiet explanatory text.
    pub fn note(&mut self, text: &str) {
        if self.drawing() {
            gfx::text(self.x + 4, self.y, &SMALL, text, FAINT);
        }
        self.y += 18;
    }

    /// Start a card that holds the next `rows` rows.
    pub fn card(&mut self, rows: u32) {
        if self.drawing() {
            let h = rows as i32 * ROW_H;
            gfx::round_a(self.x, self.y, self.w, h, CARD_R, WHITE, CARD_A);
            gfx::stroke(self.x, self.y, self.w, h, CARD_R, WHITE, LINE_A);
        }
        self.rows_left = rows;
        self.first_row = true;
    }

    /// Claim the next row of the current card.
    pub fn row(&mut self) -> Rect {
        let r = Rect::new(self.x, self.y, self.w, ROW_H);
        if self.drawing() && !self.first_row {
            gfx::rect_a(self.x + PAD, self.y, self.w - 2 * PAD, 1, WHITE, LINE_A);
        }
        self.first_row = false;
        self.y += ROW_H;
        if self.rows_left > 0 {
            self.rows_left -= 1;
            if self.rows_left == 0 {
                self.y += CARD_GAP;
            }
        }
        r
    }

    fn label(&self, r: Rect, text: &str) {
        gfx::text(r.x + PAD, r.y + (ROW_H - LABEL.height) / 2, &LABEL, text, TEXT);
    }

    /// Label on the left, read-only value on the right.
    pub fn info(&mut self, label: &str, value: &str) {
        let r = self.row();
        if self.drawing() {
            self.label(r, label);
            gfx::text_right(r.x + r.w - PAD, r.y + (ROW_H - BODY.height) / 2, &BODY, value, MUTED);
        }
    }

    /// Like `info`, with a coloured status pill instead of plain text.
    pub fn status(&mut self, label: &str, value: &str, color: u32) {
        let r = self.row();
        if self.drawing() {
            self.label(r, label);
            let tw = gfx::text_w(&SMALL, value);
            let pw = tw + 20;
            let px = r.x + r.w - PAD - pw;
            let py = r.y + (ROW_H - 22) / 2;
            gfx::round_a(px, py, pw, 22, 11, color, 46);
            gfx::text(px + 10, py + (22 - SMALL.height) / 2, &SMALL, value, color);
        }
    }

    /// On/off switch. Returns true when it was flipped.
    pub fn toggle(&mut self, label: &str, value: &mut bool) -> bool {
        let r = self.row();
        let sw = Rect::new(r.x + r.w - PAD - 40, r.y + (ROW_H - 22) / 2, 40, 22);
        if self.drawing() {
            self.label(r, label);
            if *value {
                gfx::round(sw.x, sw.y, sw.w, sw.h, 11, theme::accent());
            } else {
                gfx::round_a(sw.x, sw.y, sw.w, sw.h, 11, WHITE, 40);
            }
            let knob_x = if *value { sw.x + sw.w - 20 } else { sw.x + 2 };
            gfx::round(knob_x, sw.y + 2, 18, 18, 9, WHITE);
        } else if self.pressed(r) {
            *value = !*value;
            self.changed = true;
            return true;
        }
        false
    }

    /// Slider over `min..=max`. Returns true while it is being moved.
    pub fn slider(&mut self, label: &str, value: &mut u32, min: u32, max: u32, unit: &str) -> bool {
        let id = self.id();
        let r = self.row();
        let track = Rect::new(r.x + r.w - PAD - 52 - SLIDER_W, r.y + ROW_H / 2 - 2, SLIDER_W, 4);
        let span = (max - min).max(1) as i32;

        if self.drawing() {
            self.label(r, label);
            let mut buf = [0u8; 16];
            let text = fmt_value(&mut buf, *value, unit);
            gfx::text_right(r.x + r.w - PAD, r.y + (ROW_H - BODY.height) / 2, &BODY, text, MUTED);

            let filled = track.w * (*value).clamp(min, max).saturating_sub(min) as i32 / span;
            gfx::round_a(track.x, track.y, track.w, track.h, 2, WHITE, 40);
            gfx::round(track.x, track.y, filled, track.h, 2, theme::accent());
            gfx::round(track.x + filled - 8, track.y - 6, 16, 16, 8, WHITE);
            return false;
        }

        let grab = Rect::new(track.x - 10, r.y, track.w + 20, ROW_H);
        if self.pressed(grab) {
            *self.active = Some(id);
        }
        let held = *self.active == Some(id);
        if held {
            let t = (self.mx - track.x).clamp(0, track.w);
            let v = min + ((t * span + track.w / 2) / track.w) as u32;
            if v != *value {
                *value = v;
                self.changed = true;
            }
        }
        held
    }

    /// Minus and plus buttons around a value. Returns true on a change.
    pub fn stepper(&mut self, label: &str, value: &mut i32, min: i32, max: i32, text: &str) -> bool {
        let r = self.row();
        let by = r.y + (ROW_H - 26) / 2;
        let plus = Rect::new(r.x + r.w - PAD - 26, by, 26, 26);
        let minus = Rect::new(plus.x - 84, by, 26, 26);
        if self.drawing() {
            self.label(r, label);
            for (b, sign) in [(minus, "-"), (plus, "+")] {
                gfx::round_a(b.x, b.y, b.w, b.h, 7, WHITE, 30);
                gfx::text_center(b.x + b.w / 2, b.y + (26 - LABEL.height) / 2, &LABEL, sign, TEXT);
            }
            let mid = (minus.x + minus.w + plus.x) / 2;
            gfx::text_center(mid, r.y + (ROW_H - BODY.height) / 2, &BODY, text, TEXT);
            return false;
        }
        let before = *value;
        if self.pressed(minus) {
            *value = (*value - 1).max(min);
        } else if self.pressed(plus) {
            *value = (*value + 1).min(max);
        }
        if *value != before {
            self.changed = true;
        }
        *value != before
    }

    /// Label with a push button on the right. Returns true when clicked.
    pub fn button(&mut self, label: &str, caption: &str) -> bool {
        let r = self.row();
        let bw = gfx::text_w(&LABEL, caption) + 28;
        let b = Rect::new(r.x + r.w - PAD - bw, r.y + (ROW_H - 28) / 2, bw, 28);
        if self.drawing() {
            self.label(r, label);
            gfx::round(b.x, b.y, b.w, b.h, 8, theme::accent());
            gfx::text_center(b.x + b.w / 2, b.y + (28 - LABEL.height) / 2, &LABEL, caption, WHITE);
            return false;
        }
        self.pressed(b)
    }

    /// A row of colour dots. Returns true when a different one was picked.
    pub fn swatches(&mut self, label: &str, colors: &[u32], selected: &mut usize) -> bool {
        let r = self.row();
        let n = colors.len() as i32;
        let mut picked = false;
        for (i, color) in colors.iter().enumerate() {
            let dx = r.x + r.w - PAD - (n - i as i32) * 34 + 8;
            let dot = Rect::new(dx, r.y + (ROW_H - 26) / 2, 26, 26);
            if self.drawing() {
                gfx::round(dot.x, dot.y, 26, 26, 13, *color);
                if i == *selected {
                    gfx::stroke(dot.x - 3, dot.y - 3, 32, 32, 16, WHITE, 230);
                    gfx::icon(dot.x + 5, dot.y + 5, image::icon16(icon::CHECK), WHITE);
                }
            } else if self.pressed(dot.grow(3)) && i != *selected {
                *selected = i;
                self.changed = true;
                picked = true;
            }
        }
        if self.drawing() {
            self.label(r, label);
        }
        picked
    }

    /// A thin progress bar inside its own row, with a caption above it.
    pub fn meter(&mut self, label: &str, value: &str, used: u64, total: u64) {
        let r = self.row();
        if self.drawing() {
            gfx::text(r.x + PAD, r.y + 6, &LABEL, label, TEXT);
            gfx::text_right(r.x + r.w - PAD, r.y + 6, &BODY, value, MUTED);
            let tw = r.w - 2 * PAD;
            let fill = if total == 0 { 0 } else { (tw as u64 * used.min(total) / total) as i32 };
            gfx::round_a(r.x + PAD, r.y + ROW_H - 12, tw, 4, 2, WHITE, 40);
            gfx::round(r.x + PAD, r.y + ROW_H - 12, fill.max(4), 4, 2, theme::accent());
        }
    }

    /// Outline used to mark the chosen item in a grid.
    pub fn ring(&self, r: Rect, radius: i32) {
        let a = theme::accent();
        gfx::stroke(r.x - 3, r.y - 3, r.w + 6, r.h + 6, radius + 3, a, 255);
        gfx::stroke(r.x - 2, r.y - 2, r.w + 4, r.h + 4, radius + 2, a, 255);
    }
}

/// Format "42%" style values without allocating.
fn fmt_value<'b>(buf: &'b mut [u8; 16], value: u32, unit: &str) -> &'b str {
    let mut digits = [0u8; 10];
    let mut n = 0;
    let mut v = value;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let mut len = 0;
    for i in (0..n).rev() {
        buf[len] = digits[i];
        len += 1;
    }
    for b in unit.bytes().take(buf.len() - len) {
        buf[len] = b;
        len += 1;
    }
    core::str::from_utf8(&buf[..len]).unwrap_or("")
}
