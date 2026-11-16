use crate::framebuffer as fb;
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
const LINE_H: u32 = 18;
const SCALE: u32 = 2;
const TEXT: u32 = 0x00C8_D3E0;
const ACCENT: u32 = 0x0070_D694;

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
    if BLINK_ON.load(Ordering::Relaxed) {
        let cursor_x = x + 48 + t.input_len as u32 * 16;
        fb::fill_rect(cursor_x, prompt_y, 8, 14, ACCENT);
    }
}
