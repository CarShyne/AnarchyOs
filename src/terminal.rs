use crate::fs;
use crate::gfx;
use crate::heap;
use crate::image::MONO;
use crate::lock;
use crate::pit;
use crate::settings;
use crate::theme;
use crate::volume;
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
    cwd: String,
}

static mut TERM: Term = Term {
    lines: [[0; COLS]; ROWS],
    lens: [0; ROWS],
    count: 0,
    input: [0; INPUT_MAX],
    input_len: 0,
    cwd: String::new(),
};

fn term() -> &'static mut Term {
    unsafe { &mut *addr_of_mut!(TERM) }
}

/// The folder commands work in, always an absolute path.
fn cwd() -> String {
    let c = &term().cwd;
    if c.is_empty() {
        String::from("/")
    } else {
        c.clone()
    }
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
            print(&format!("{} > {}", cwd(), cmd));
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

fn path_of(rest: &str) -> String {
    fs::normalize(&cwd(), rest.trim())
}

fn drive_error() -> &'static str {
    if volume::state() == volume::State::NoDisk {
        "no drive attached"
    } else {
        "the drive is locked"
    }
}

fn run_program(image: &[u8]) {
    match crate::userspace::run(image) {
        Ok(code) => print(&format!("program exited with code {}", code)),
        Err(e) => print(e),
    }
}

fn run(cmd: &str) {
    let mut parts = cmd.splitn(2, ' ');
    let name = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    match name {
        "" => {}
        "help" => {
            print("commands: help, about, mem, uptime, echo, clear, drive, lock");
            print("folders:  pwd, cd <dir>, ls [dir], mkdir <dir>, rm <path>, mv <from> <to>");
            print("files:    cat <file>, write <file> <text>");
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
        "pwd" => print(&cwd()),
        "cd" => {
            let target = if rest.trim().is_empty() { String::from("/") } else { path_of(rest) };
            if fs::is_dir(&target) {
                term().cwd = target;
            } else if !fs::mounted() {
                print(drive_error());
            } else {
                print("no such folder");
            }
        }
        "ls" => {
            let target = if rest.trim().is_empty() { cwd() } else { path_of(rest) };
            match fs::list(&target) {
                Some(entries) if entries.is_empty() => print("(empty)"),
                Some(entries) => {
                    for e in entries {
                        let label = if e.dir { format!("{}/", e.name) } else { e.name.clone() };
                        let size = if e.dir { String::from("folder") } else { format!("{} B", e.size) };
                        print(&format!("{:<28}{}", label, size));
                    }
                }
                None if !fs::mounted() => print(drive_error()),
                None => print("no such folder"),
            }
        }
        "mkdir" if rest.trim().is_empty() => print("usage: mkdir <folder>"),
        "mkdir" => match fs::mkdir(&path_of(rest)) {
            Ok(()) => print(&format!("created {}", path_of(rest))),
            Err(e) => print(e),
        },
        "rm" if rest.trim().is_empty() => print("usage: rm <path>"),
        "rm" => match fs::remove(&path_of(rest)) {
            Ok(()) => print(&format!("removed {}", path_of(rest))),
            Err(e) => print(e),
        },
        "mv" => {
            let mut p = rest.splitn(2, ' ');
            let from = p.next().unwrap_or("");
            let to = p.next().unwrap_or("");
            if from.is_empty() || to.is_empty() {
                print("usage: mv <from> <to>");
            } else {
                match fs::rename(&path_of(from), &path_of(to)) {
                    Ok(()) => print("moved"),
                    Err(e) => print(e),
                }
            }
        }
        "run" if rest.trim().is_empty() => print("usage: run <file> | run hello"),
        "run" if rest.trim() == "hello" => run_program(&crate::userspace::hello_image()),
        "run" => match fs::read(&path_of(rest)) {
            Some(image) => run_program(&image),
            None => print("no such file"),
        },
        "cat" => match fs::read(&path_of(rest)) {
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
            if file.is_empty() {
                print("usage: write <file> <text>");
            } else {
                match fs::write(&path_of(file), body.as_bytes()) {
                    Ok(()) => print(&format!("saved {} ({} B)", file, body.len())),
                    Err(e) => print(e),
                }
            }
        }
        "drive" => {
            let state = match volume::state() {
                volume::State::NoDisk => "no drive attached",
                volume::State::Blank => "blank, not set up yet",
                volume::State::Legacy => "old unencrypted layout, not set up yet",
                volume::State::Locked => "locked",
                volume::State::Open => "unlocked",
            };
            print(&format!("state:   {}", state));
            print("cipher:  ChaCha20 on every sector, keys from PBKDF2-HMAC-SHA256");
            print(&format!("rounds:  {}", volume::iterations_on_disk()));
            if fs::mounted() {
                print(&format!(
                    "space:   {} KiB used of {} KiB",
                    fs::used_bytes() / 1024,
                    fs::total_bytes() / 1024
                ));
            }
        }
        "lock" => {
            if lock::lock_now() {
                print("drive locked");
            } else {
                print("there is no unlocked drive to lock");
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
        let color = if text.contains(" > ") { theme::MUTED } else { theme::TEXT };
        gfx::text(x + 18, top + i as i32 * LINE_H, &MONO, text, color);
    }

    gfx::rect_a(x + 14, prompt_y - 8, w - 28, 1, theme::WHITE, theme::LINE_A);
    let prompt = format!("{} > ", cwd());
    let input = core::str::from_utf8(&t.input[..t.input_len]).unwrap_or("");
    let after_prompt = gfx::text(x + 18, prompt_y, &MONO, &prompt, theme::accent());
    let end = gfx::text(after_prompt, prompt_y, &MONO, input, theme::TEXT);
    if BLINK_ON.load(Ordering::Relaxed) {
        gfx::rect(end + 1, prompt_y + 1, 8, MONO.height - 2, theme::accent());
    }
}
