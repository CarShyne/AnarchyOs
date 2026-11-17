//! AnarchyAi, a native version of the AnarchyAi web interface. The layout
//! follows the web GUI: a header with the Local, Hybrid and Cloud switch and a
//! context meter, a collapsible sidebar with the same sections, the chat, and
//! a composer. There is no model behind it yet, because AnarchyOS has no
//! network stack, so replies are built in. Notes saved with `remember` are
//! kept on the encrypted drive.

use crate::fs;
use crate::gfx;
use crate::image::{self, icon, Font, BODY, LABEL, MARK_22, MARK_48, SMALL, STRONG, TITLE};
use crate::theme::{self, FAINT, MUTED, TEXT, WHITE};
use crate::ui::Rect;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const MEMORY_DIR: &str = "/Documents/AnarchyAi";
const MEMORY: &str = "/Documents/AnarchyAi/memory.txt";
const RECALL_MAX: usize = 8;
const LOG_MAX: usize = 200;
const INPUT_MAX: usize = 300;
/// Chat size, in characters, at which the context meter reads 100 percent.
const CONTEXT_CHARS: usize = 6000;

const HEADER_H: i32 = 52;
const SIDEBAR_W: i32 = 196;
const NAV_H: i32 = 34;
const COMPOSER_H: i32 = 70;
const PAD: i32 = 18;
const BUBBLE_GAP: i32 = 12;
const MODE_W: i32 = 66;
const MODE_GAP: i32 = 4;

const MODES: [&str; 3] = ["Local", "Hybrid", "Cloud"];

const HELP: &str = "There is no model connected yet, so I can't chat freely. I can remember \
things for you: remember <note>, recall, forget, clear, help.";

struct Nav {
    name: &'static str,
    icon: usize,
    about: &'static str,
}

/// The sidebar sections, in the same order as the web GUI.
const NAV: [Nav; 12] = [
    Nav { name: "Profiles", icon: icon::PALETTE, about: "Saved looks for AnarchyAi." },
    Nav { name: "Wallpaper", icon: icon::IMAGE, about: "The desktop wallpaper." },
    Nav { name: "Core", icon: icon::MEMORY, about: "The core identity and rules files." },
    Nav { name: "Obsidian vault", icon: icon::NOTES, about: "Your Obsidian notes vault." },
    Nav { name: "Snapshots", icon: icon::DRIVE, about: "Saved copies of AnarchyAi's state." },
    Nav { name: "AnarchyAi", icon: icon::SPARKLES, about: "Chat with AnarchyAi." },
    Nav { name: "Skills", icon: icon::GRID, about: "Skills AnarchyAi can use." },
    Nav { name: "Tasks", icon: icon::CHECK, about: "Scheduled and background tasks." },
    Nav { name: "Channels", icon: icon::GLOBE, about: "Chat channels AnarchyAi can post to." },
    Nav { name: "Gmail", icon: icon::CALENDAR, about: "Gmail access for AnarchyAi." },
    Nav { name: "Codeline", icon: icon::CPU, about: "Code projects AnarchyAi can work on." },
    Nav { name: "TUI", icon: icon::TERMINAL, about: "The terminal interface for AnarchyAi." },
];
const WALLPAPER: usize = 1;
const CHAT: usize = 5;

struct Message {
    from_user: bool,
    text: String,
}

pub struct State {
    log: Vec<Message>,
    input: String,
    /// Index into MODES.
    mode: usize,
    /// Index into NAV.
    nav: usize,
    sidebar: bool,
}

impl State {
    pub const fn new() -> State {
        State { log: Vec::new(), input: String::new(), mode: 0, nav: CHAT, sidebar: true }
    }
}

/// Where everything sits in the window body. Drawing and clicks both use it.
struct Layout {
    menu: Rect,
    brand_x: i32,
    modes: [Rect; 3],
    meter: Rect,
    side: Rect,
    chat: Rect,
    composer: Rect,
    input: Rect,
    send: Rect,
}

fn layout(body: Rect, sidebar: bool) -> Layout {
    let btn_y = body.y + (HEADER_H - 30) / 2;
    let menu = Rect::new(body.x + 12, btn_y, 34, 30);
    let brand_x = menu.x + menu.w + 12;
    let mode_x = body.x + 188;
    let modes = core::array::from_fn(|i| Rect::new(mode_x + i as i32 * (MODE_W + MODE_GAP), btn_y, MODE_W, 30));
    let meter = Rect::new(body.x + body.w - 150 - 14, body.y + 14, 150, 26);

    let side_w = if sidebar { SIDEBAR_W } else { 0 };
    let content_y = body.y + HEADER_H;
    let side = Rect::new(body.x, content_y, side_w, body.h - HEADER_H);
    let main_x = body.x + side_w;
    let main_w = body.w - side_w;
    let composer = Rect::new(main_x, body.y + body.h - COMPOSER_H, main_w, COMPOSER_H);
    let chat = Rect::new(main_x, content_y, main_w, body.h - HEADER_H - COMPOSER_H);
    let send = Rect::new(composer.x + composer.w - PAD - 84, composer.y + 15, 84, 40);
    let input = Rect::new(composer.x + PAD, composer.y + 15, send.x - composer.x - PAD - 10, 40);
    Layout { menu, brand_x, modes, meter, side, chat, composer, input, send }
}

fn nav_rect(side: Rect, i: usize) -> Rect {
    Rect::new(side.x + 10, side.y + 14 + i as i32 * NAV_H, side.w - 20, NAV_H - 4)
}

/// Break `text` into lines no wider than `max_w` in `font`.
fn wrap_lines(out: &mut Vec<String>, text: &str, font: &Font, max_w: i32) {
    let mut line = String::new();
    for ch in text.chars() {
        line.push(ch);
        if gfx::text_w(font, &line) > max_w && line.chars().count() > 1 {
            let last = line.pop().unwrap_or(' ');
            out.push(core::mem::take(&mut line));
            line.push(last);
        }
    }
    out.push(line);
}

/// The end of `s` that fits in `max_w`, so a long prompt shows what is typed last.
fn tail(s: &str, max_w: i32) -> &str {
    let mut start = 0;
    while start < s.len() && gfx::text_w(&BODY, &s[start..]) > max_w {
        start += s[start..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    &s[start..]
}

/// A press at (x, y) inside the window body. Returns true if the window needs a repaint.
pub fn press(body: Rect, st: &mut State, x: i32, y: i32) -> bool {
    let l = layout(body, st.sidebar);
    if l.menu.contains(x, y) {
        st.sidebar = !st.sidebar;
        return true;
    }
    for (i, r) in l.modes.iter().enumerate() {
        if r.contains(x, y) {
            st.mode = i;
            return true;
        }
    }
    if st.sidebar {
        for i in 0..NAV.len() {
            if nav_rect(l.side, i).contains(x, y) {
                st.nav = i;
                return true;
            }
        }
    }
    if st.nav == CHAT && l.send.contains(x, y) {
        send_input(st);
        return true;
    }
    false
}

/// A typed character. Enter sends the message.
pub fn key(st: &mut State, c: u8) {
    match c {
        b'\n' | b'\r' => send_input(st),
        0x08 => {
            st.input.pop();
        }
        0x20..=0x7E => {
            if st.input.len() < INPUT_MAX {
                st.input.push(c as char);
            }
        }
        _ => {}
    }
}

fn send_input(st: &mut State) {
    let typed = core::mem::take(&mut st.input);
    let text = typed.trim();
    if !text.is_empty() {
        submit(st, text);
    }
}

/// Run a message through the commands and record the reply.
fn submit(st: &mut State, text: &str) {
    st.log.push(Message { from_user: true, text: String::from(text) });

    let (cmd, arg) = match text.find(' ') {
        Some(i) => (&text[..i], text[i + 1..].trim()),
        None => (text, ""),
    };
    let replies: Vec<String> = match cmd.to_ascii_lowercase().as_str() {
        "help" => alloc::vec![String::from(HELP)],
        "clear" => {
            st.log.clear();
            return;
        }
        "remember" if arg.is_empty() => {
            alloc::vec![String::from("Tell me what to remember, for example: remember my bike is blue.")]
        }
        "remember" => alloc::vec![match save_note(arg) {
            Ok(()) => String::from("Saved to memory."),
            Err(e) => String::from(e),
        }],
        "recall" => recall(),
        "forget" => alloc::vec![match forget() {
            Ok(()) => String::from("Memory cleared."),
            Err(e) => String::from(e),
        }],
        _ if st.mode != 0 => alloc::vec![format!(
            "{} mode needs networking, which AnarchyOS doesn't have yet. Switch to Local, or type help.",
            MODES[st.mode]
        )],
        _ => alloc::vec![String::from(
            "I can't answer that yet, because no model is connected. Type help to see what I can do.",
        )],
    };

    for r in replies {
        st.log.push(Message { from_user: false, text: r });
    }
    if st.log.len() > LOG_MAX {
        let extra = st.log.len() - LOG_MAX;
        st.log.drain(0..extra);
    }
}

/// Every saved note, oldest first. Empty when there is none.
fn notes() -> Vec<String> {
    match fs::read(MEMORY) {
        Some(data) => String::from_utf8_lossy(&data)
            .lines()
            .map(String::from)
            .filter(|l| !l.trim().is_empty())
            .collect(),
        None => Vec::new(),
    }
}

fn save_note(note: &str) -> Result<(), &'static str> {
    if !fs::mounted() {
        return Err("The drive is locked, so I can't save that. Unlock it and try again.");
    }
    // The folder may already exist, which is fine.
    let _ = fs::mkdir(MEMORY_DIR);
    let mut data = fs::read(MEMORY).unwrap_or_default();
    data.extend_from_slice(note.as_bytes());
    data.push(b'\n');
    fs::write(MEMORY, &data).map_err(|_| "I couldn't save that note. The drive may be full.")
}

fn recall() -> Vec<String> {
    if !fs::mounted() {
        return alloc::vec![String::from("The drive is locked, so I can't read my memory.")];
    }
    let all = notes();
    if all.is_empty() {
        return alloc::vec![String::from("I don't remember anything yet.")];
    }
    let skip = all.len().saturating_sub(RECALL_MAX);
    let mut out = alloc::vec![format!("I remember {} note(s). Here are the latest:", all.len())];
    for n in &all[skip..] {
        out.push(format!("- {}", n));
    }
    out
}

fn forget() -> Result<(), &'static str> {
    if !fs::mounted() {
        return Err("The drive is locked, so there is nothing I can clear right now.");
    }
    fs::write(MEMORY, &[]).map_err(|_| "I couldn't clear my memory.")
}

pub fn draw(body: Rect, st: &mut State) {
    let l = layout(body, st.sidebar);
    header(body, &l, st);
    if st.sidebar {
        sidebar(l.side, st.nav);
    }
    if st.nav == CHAT {
        chat(l.chat, st);
        composer(&l, st);
    } else {
        section(l.chat, st.nav);
    }
}

fn header(body: Rect, l: &Layout, st: &State) {
    let m = l.menu;
    gfx::round_a(m.x, m.y, m.w, m.h, 8, WHITE, theme::CARD_A);
    for i in 0..3 {
        gfx::rect(m.x + 9, m.y + 8 + i * 6, 16, 2, TEXT);
    }

    gfx::image_a(l.brand_x, body.y + (HEADER_H - MARK_22.h as i32) / 2, &MARK_22, 255);
    gfx::text(
        l.brand_x + MARK_22.w as i32 + 8,
        body.y + (HEADER_H - STRONG.height) / 2,
        &STRONG,
        "AnarchyAi",
        TEXT,
    );

    for (i, r) in l.modes.iter().enumerate() {
        let on = i == st.mode;
        if on {
            gfx::round(r.x, r.y, r.w, r.h, 8, theme::accent());
        } else {
            gfx::round_a(r.x, r.y, r.w, r.h, 8, WHITE, theme::CARD_A);
        }
        let color = if on { WHITE } else { MUTED };
        gfx::text_center(r.x + r.w / 2, r.y + (r.h - LABEL.height) / 2, &LABEL, MODES[i], color);
    }
    gfx::rect_a(body.x, body.y + HEADER_H - 1, body.w, 1, WHITE, theme::LINE_A);

    let used: usize = st.log.iter().map(|m| m.text.len()).sum::<usize>().min(CONTEXT_CHARS);
    let pct = used * 100 / CONTEXT_CHARS;
    let mr = l.meter;
    gfx::text_right(mr.x + mr.w, mr.y, &SMALL, &format!("{}% context", pct), MUTED);
    gfx::round_a(mr.x, mr.y + 20, mr.w, 4, 2, WHITE, 40);
    gfx::round(mr.x, mr.y + 20, mr.w * pct as i32 / 100, 4, 2, theme::accent());
}

fn sidebar(side: Rect, selected: usize) {
    gfx::rect_a(side.x, side.y, side.w, side.h, theme::PANEL, 110);
    gfx::rect_a(side.x + side.w - 1, side.y, 1, side.h, WHITE, theme::LINE_A);
    for (i, item) in NAV.iter().enumerate() {
        let r = nav_rect(side, i);
        let on = i == selected;
        if on {
            gfx::round(r.x, r.y, r.w, r.h, 8, theme::accent());
        }
        let color = if on { WHITE } else { MUTED };
        gfx::icon(r.x + 10, r.y + (r.h - 16) / 2, image::icon16(item.icon), color);
        let text_color = if on { WHITE } else { TEXT };
        gfx::text(r.x + 34, r.y + (r.h - LABEL.height) / 2, &LABEL, item.name, text_color);
    }
}

/// A section the Python backend would provide. It cannot run on AnarchyOS yet.
fn section(area: Rect, nav: usize) {
    let item = &NAV[nav];
    let card = Rect::new(area.x + PAD, area.y + 24, area.w - 2 * PAD, 170);
    gfx::round_a(card.x, card.y, card.w, card.h, 14, WHITE, theme::CARD_A);
    gfx::stroke(card.x, card.y, card.w, card.h, 14, WHITE, theme::LINE_A);
    gfx::text(card.x + 22, card.y + 20, &TITLE, item.name, TEXT);
    gfx::text(card.x + 22, card.y + 62, &BODY, item.about, MUTED);

    let note = if nav == WALLPAPER {
        "Change the wallpaper in Settings > Wallpaper."
    } else {
        "Not available on AnarchyOS yet. It needs the AnarchyAi backend, which needs networking."
    };
    let mut lines = Vec::new();
    wrap_lines(&mut lines, note, &SMALL, card.w - 44);
    for (i, line) in lines.iter().enumerate() {
        gfx::text(card.x + 22, card.y + 104 + i as i32 * (SMALL.height + 4), &SMALL, line, FAINT);
    }
}

fn welcome(area: Rect) {
    let cx = area.x + area.w / 2;
    let top = area.y + area.h / 2 - 80;
    gfx::image_a(cx - MARK_48.w as i32 / 2, top, &MARK_48, 255);
    gfx::text_center(cx, top + MARK_48.h as i32 + 18, &TITLE, "Hi, I'm AnarchyAi", TEXT);
    gfx::text_center(cx, top + MARK_48.h as i32 + 56, &BODY, "Type help to see what I can do.", MUTED);
}

fn chat(area: Rect, st: &State) {
    if st.log.is_empty() {
        welcome(area);
        return;
    }
    let max_w = (area.w * 72 / 100).min(520);
    let line_h = BODY.height + 5;
    let mut y = area.y + area.h - 14;
    for m in st.log.iter().rev() {
        let mut lines: Vec<String> = Vec::new();
        wrap_lines(&mut lines, &m.text, &BODY, max_w - 28);
        let h = lines.len() as i32 * line_h + 22;
        if y - h < area.y + 8 {
            break;
        }
        let w = lines.iter().map(|s| gfx::text_w(&BODY, s)).max().unwrap_or(0) + 28;
        let x = if m.from_user { area.x + area.w - PAD - w } else { area.x + PAD };
        if m.from_user {
            gfx::round_a(x, y - h, w, h, 14, theme::accent(), 46);
        } else {
            gfx::round_a(x, y - h, w, h, 14, WHITE, theme::CARD_A);
        }
        gfx::stroke(x, y - h, w, h, 14, WHITE, theme::LINE_A);
        let color = if m.from_user { TEXT } else { MUTED };
        for (i, line) in lines.iter().enumerate() {
            gfx::text(x + 14, y - h + 11 + i as i32 * line_h, &BODY, line, color);
        }
        y = y - h - BUBBLE_GAP;
    }
}

fn composer(l: &Layout, st: &State) {
    let c = l.composer;
    gfx::rect_a(c.x, c.y, c.w, 1, WHITE, theme::LINE_A);
    let r = l.input;
    gfx::round_a(r.x, r.y, r.w, r.h, 12, WHITE, theme::CARD_A);
    gfx::stroke(r.x, r.y, r.w, r.h, 12, WHITE, theme::LINE_A);
    let ty = r.y + (r.h - BODY.height) / 2;
    if st.input.is_empty() {
        gfx::text(r.x + 16, ty, &BODY, "Message AnarchyAi, or type help", FAINT);
        gfx::rect(r.x + 16, ty + 1, 2, BODY.height - 2, theme::accent());
    } else {
        let shown = tail(&st.input, r.w - 32);
        let end = gfx::text(r.x + 16, ty, &BODY, shown, TEXT);
        gfx::rect(end + 2, ty + 1, 2, BODY.height - 2, theme::accent());
    }

    let s = l.send;
    gfx::round(s.x, s.y, s.w, s.h, 12, theme::accent());
    gfx::text_center(s.x + s.w / 2, s.y + (s.h - STRONG.height) / 2, &STRONG, "Send", WHITE);
}
