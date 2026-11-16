use crate::fs;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

pub const FILE: &str = "settings.cfg";

static mut DISK_OK: bool = false;
static mut WARNED: bool = false;

static mut STORE: Vec<(String, String)> = Vec::new();
static mut DIRTY: bool = false;

fn store() -> &'static mut Vec<(String, String)> {
    unsafe { &mut *addr_of_mut!(STORE) }
}

fn dirty() -> &'static mut bool {
    unsafe { &mut *addr_of_mut!(DIRTY) }
}

fn set_in(s: &mut Vec<(String, String)>, key: &str, value: &str) {
    match s.iter().position(|e| e.0 == key) {
        Some(i) => s[i].1 = value.to_string(),
        None => s.push((key.to_string(), value.to_string())),
    }
}

pub fn load() -> bool {
    let s = store();
    s.clear();
    *dirty() = false;
    unsafe { *addr_of_mut!(DISK_OK) = true };

    let data = match fs::read(FILE) {
        Some(d) => d,
        None => {
            kprintln!("settings: no saved file, using defaults");
            return false;
        }
    };
    let text = String::from_utf8_lossy(&data);
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            set_in(s, k.trim(), v.trim());
        }
    }
    kprintln!("settings: loaded {} entries", s.len());
    true
}

pub fn get(key: &str) -> Option<String> {
    store().iter().find(|e| e.0 == key).map(|e| e.1.clone())
}

pub fn get_u32(key: &str, default: u32) -> u32 {
    get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn set(key: &str, value: &str) {
    set_in(store(), key, value);
    *dirty() = true;
}

pub fn flush() -> Result<(), &'static str> {
    if !*dirty() {
        return Ok(());
    }
    let mut out = String::new();
    for (k, v) in store().iter() {
        out.push_str(k);
        out.push('=');
        out.push_str(v);
        out.push('\n');
    }
    fs::write(FILE, out.as_bytes())?;
    *dirty() = false;
    Ok(())
}

/// Save if there is a disk to save to. Without one, settings still apply
/// for this session, and the reason is logged once instead of every time.
pub fn flush_quietly() {
    let disk_ok = unsafe { *addr_of_mut!(DISK_OK) };
    let result = if disk_ok { flush() } else { Err("no disk attached") };
    if let Err(e) = result {
        let warned = unsafe { &mut *addr_of_mut!(WARNED) };
        if !*warned {
            *warned = true;
            kprintln!("settings: not saved ({}), changes last until reboot", e);
        }
    }
}

pub fn all() -> Vec<(String, String)> {
    store().clone()
}
