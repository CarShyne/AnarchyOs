use crate::gfx;
use crate::image::MONO;
use crate::theme;
use crate::fs;
use crate::heap;
use crate::pit;
use crate::settings;
use alloc::format;
use alloc::string::String;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};

const COLS: usize = 120;
const ROWS: usize = 128;
const INPUT_MAX: usize = 100;
const LINE_H: i32 = 19;
/// Height of the strip at the bottom of the window that holds the prompt.
pub const PROMPT_H: i32 = 40;

static BLINK_ON: AtomicBool = AtomicBool::new(true);

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

fn term() -> &'static mut Term {
    unsafe { &mut *addr_of_mut!(TERM) }
}

pub fn init() {
    print("AnarchyOS terminal. Type help.");
}

pub fn toggle_blink() {
    let on = BLINK_ON.load(Ordering::Relaxed);
    BLINK_ON.store(!on, Ordering::Relaxed);
}

pub fn print(text: &str) {
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

pub fn key(c: u8) {
    BLINK_ON.store(true, Ordering::Relaxed);
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
        "help" => {
            print("commands: help, about, mem, uptime, echo, clear");
            print("files:    ls, cat <file>, write <file> <text>");
            print("settings: settings, get <key>, set <key> <value>");
        }
        "about" => {
            print("AnarchyOS v0.1.0");
            print("A clean, fun kernel and desktop.");
        }
        "mem" => {
            let free = heap::free_bytes() / 1024;
            print(&format!("heap free: {} KiB", free));
        }
        "uptime" => {
            let secs = pit::ticks() / pit::HZ;
            print(&format!("up {} s", secs));
        }
        "echo" => print(rest),
        "clear" => term().count = 0,
        "ls" => {
            let mut any = false;
            fs::list(|n, size| {
                any = true;
                print(&format!("{:<24} {} B", n, size));
            });
            if !any {
                print("(no files)");
            }
        }
        "cat" => match fs::read(rest) {
            Some(data) => {
                let text = String::from_utf8_lossy(&data);
                for line in text.lines().take(12) {
                    print(line);
                }
            }
            None => print("no such file"),
        },
        "write" => {
            let mut p = rest.splitn(2, ' ');
            let file = p.next().unwrap_or("");
            let body = p.next().unwrap_or("");
            match fs::write(file, body.as_bytes()) {
                Ok(()) => print(&format!("saved {} ({} B)", file, body.len())),
                Err(e) => print(e),
            }
        }
        "settings" => {
            for (k, v) in settings::all() {
                print(&format!("{:<14} {}", k, v));
            }
        }
        "get" => match settings::get(rest) {
            Some(v) => print(&format!("{} = {}", rest, v)),
            None => print("no such setting"),
        },
        "set" => {
            let mut p = rest.splitn(2, ' ');
            let key = p.next().unwrap_or("");
            let value = p.next().unwrap_or("");
            if key.is_empty() || value.is_empty() {
                print("usage: set <key> <value>");
            } else {
                settings::set(key, value);
                match settings::flush() {
                    Ok(()) => print(&format!("{} = {} (saved)", key, value)),
                    Err(e) => print(e),
                }
            }
        }
        other => {
            if !crate::net::command(other, rest) {
                print(&format!("unknown command: {}", other));
            }
        }
    }
}

/// Draw the scrollback and the prompt into the body of the window.
pub fn draw(x: i32, y: i32, w: i32, h: i32) {
    let t = term();
    let top = y + 12;
    let prompt_y = y + h - 32;
    let visible = ((prompt_y - 8 - top) / LINE_H).max(0) as usize;
    let shown = t.count.min(visible);
    let start = t.count - shown;

    for (i, idx) in (start..t.count).enumerate() {
        let text = core::str::from_utf8(&t.lines[idx][..t.lens[idx]]).unwrap_or("");
        let color = if text.starts_with("> ") { theme::MUTED } else { theme::TEXT };
        gfx::text(x + 18, top + i as i32 * LINE_H, &MONO, text, color);
    }

    gfx::rect_a(x + 14, prompt_y - 8, w - 28, 1, theme::WHITE, theme::LINE_A);
    let input = core::str::from_utf8(&t.input[..t.input_len]).unwrap_or("");
    let after_prompt = gfx::text(x + 18, prompt_y, &MONO, "> ", theme::accent());
    let end = gfx::text(after_prompt, prompt_y, &MONO, input, theme::TEXT);
    if BLINK_ON.load(Ordering::Relaxed) {
        gfx::rect(end + 1, prompt_y + 1, 8, MONO.height - 2, theme::accent());
    }
}
