use crate::fs;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

pub const FILE: &str = "settings.cfg";
pub const TOP_OPACITY: &str = "top_opacity";
pub const DOCK_OPACITY: &str = "dock_opacity";
pub const OPACITY_KEYS: [&str; 2] = [TOP_OPACITY, DOCK_OPACITY];

const DEFAULTS: [(&str, &str); 2] = [(TOP_OPACITY, "60"), (DOCK_OPACITY, "60")];

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
    for &(k, v) in DEFAULTS.iter() {
        set_in(s, k, v);
    }
    *dirty() = false;

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

pub fn opacity() -> [u32; 2] {
    [get_u32(TOP_OPACITY, 60).min(100), get_u32(DOCK_OPACITY, 60).min(100)]
}

pub fn all() -> Vec<(String, String)> {
    store().clone()
}
