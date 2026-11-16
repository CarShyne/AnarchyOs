//! Terminal app: a line-based shell inside a window. Output is kept in a
//! small table of lines, and input is one editable line at the bottom.
//! Commands run when Enter is pressed.

use crate::framebuffer as fb;
use crate::heap;
use alloc::format;
use alloc::string::String;
use core::ptr::addr_of_mut;

const COLS: usize = 120;
const ROWS: usize = 128;
const INPUT_MAX: usize = 100;
const LINE_H: u32 = 18;
const SCALE: u32 = 2;
const TEXT: u32 = 0x00C8_D3E0;
const ACCENT: u32 = 0x0070_D694;

struct Term {
    lines: [[u8; COLS]; ROWS],
    lens: [usize; ROWS],
    count: usize,
    input: [u8; INPUT_MAX],
    input_len: usize,
}

static mut TERM: Term = Term {
    lines: [[0; COLS]; ROWS],
    lens: [0; ROWS],
    count: 0,
    input: [0; INPUT_MAX],
    input_len: 0,
};

// Only the interrupt path and init touch this, and interrupts don't nest.
fn term() -> &'static mut Term {
    unsafe { &mut *addr_of_mut!(TERM) }
}

pub fn init() {
    print("AnarchyOS terminal. Type help.");
}

/// Append a line. Long lines are cut to the column width, and the oldest
/// line is dropped when the table is full.
fn print(text: &str) {
    let t = term();
    if t.count == ROWS {
        for i in 1..ROWS {
            t.lines[i - 1] = t.lines[i];
            t.lens[i - 1] = t.lens[i];
        }
        t.count -= 1;
    }
    let bytes = text.as_bytes();
    let n = bytes.len().min(COLS);
    let row = t.count;
    t.lines[row][..n].copy_from_slice(&bytes[..n]);
    t.lens[row] = n;
    t.count += 1;
}

/// Handle one decoded key. Enter runs the typed command.
pub fn key(c: u8) {
    match c {
        b'\n' => {
            let cmd = take_input();
            print(&format!("> {}", cmd));
            run(cmd.trim());
        }
        0x08 => {
            let t = term();
            if t.input_len > 0 {
                t.input_len -= 1;
            }
        }
        0x20..=0x7E => {
            let t = term();
            if t.input_len < INPUT_MAX {
                t.input[t.input_len] = c;
                t.input_len += 1;
            }
        }
        _ => {}
    }
}

fn take_input() -> String {
    let t = term();
    let s = String::from(core::str::from_utf8(&t.input[..t.input_len]).unwrap_or(""));
    t.input_len = 0;
    s
}

fn run(cmd: &str) {
    let mut parts = cmd.splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    match name {
        "" => {}
        "help" => print("commands: help, about, mem, echo, clear"),
        "about" => {
            print("AnarchyOS v0.1.0");
            print("A clean, fun kernel and desktop.");
        }
        "mem" => {
            let free = heap::free_bytes() / 1024;
            print(&format!("heap free: {} KiB", free));
        }
        "echo" => print(rest),
        "clear" => term().count = 0,
        other => print(&format!("unknown command: {}", other)),
    }
}

/// Draw the output and the prompt inside a window body. The window frame
/// and background are already drawn, and the clip is set to the window.
pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    let t = term();
    let top = y + 40;
    let prompt_y = y + h - 26;
    let visible = ((prompt_y - top) / LINE_H) as usize;
    let shown = t.count.min(visible);
    let start = t.count - shown;

    for (i, idx) in (start..t.count).enumerate() {
        let text = core::str::from_utf8(&t.lines[idx][..t.lens[idx]]).unwrap_or("");
        fb::draw_text(x + 16, top + i as u32 * LINE_H, text, TEXT, SCALE);
    }

    fb::fill_rect(x + 12, prompt_y - 6, w.saturating_sub(24), 1, 0x0030_3A4E);
    let input = core::str::from_utf8(&t.input[..t.input_len]).unwrap_or("");
    fb::draw_text(x + 16, prompt_y, "> ", ACCENT, SCALE);
    fb::draw_text(x + 48, prompt_y, input, TEXT, SCALE);
    let cursor_x = x + 48 + t.input_len as u32 * 16;
    fb::fill_rect(cursor_x, prompt_y, 8, 14, ACCENT);
}
