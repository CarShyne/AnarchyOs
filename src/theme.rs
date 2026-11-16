//! Colours and the user's appearance choices. Everything the Settings app
//! can change lives in `Config`, which is saved to settings.cfg on disk.

use crate::image::WALLPAPERS;
use crate::settings;
use alloc::format;
use core::ptr::addr_of_mut;

pub const WINDOW: u32 = 0x0016_171C;
pub const PANEL: u32 = 0x000C_0D10;
pub const BAR: u32 = 0x000E_0F13;
pub const TEXT: u32 = 0x00F2_F3F5;
pub const MUTED: u32 = 0x00A3_A8B3;
pub const FAINT: u32 = 0x006E_7480;
pub const WHITE: u32 = 0x00FF_FFFF;
pub const GOOD: u32 = 0x003D_DC84;

/// Alpha for card backgrounds, hairlines and outlines drawn in white.
pub const CARD_A: u32 = 14;
pub const LINE_A: u32 = 20;
pub const EDGE_A: u32 = 30;

pub const ACCENTS: [(&str, u32); 6] = [
    ("Anarchy Red", 0x00FF_1F4B),
    ("Ember", 0x00FF_7A1A),
    ("Acid", 0x003D_DC84),
    ("Cyan", 0x0022_D3EE),
    ("Electric", 0x004C_8DFF),
    ("Violet", 0x00A7_8BFA),
];

pub const MIN_WINDOW_OPACITY: u32 = 30;
pub const MAX_RADIUS: u32 = 20;

pub struct Config {
    pub top_opacity: u32,
    pub dock_opacity: u32,
    pub window_opacity: u32,
    pub accent: usize,
    /// Index into WALLPAPERS. WALLPAPERS.len() means the built-in gradient.
    pub wallpaper: usize,
    pub radius: u32,
    pub shadows: bool,
    pub clock_24h: bool,
    pub show_seconds: bool,
    pub show_date: bool,
    pub tz_offset: i32,
}

static mut CONFIG: Config = Config {
    top_opacity: 55,
    dock_opacity: 55,
    window_opacity: 82,
    accent: 0,
    wallpaper: 0,
    radius: 12,
    shadows: true,
    clock_24h: true,
    show_seconds: false,
    show_date: true,
    tz_offset: 0,
};

// Only the main loop reads or writes the config.
pub fn cfg() -> &'static mut Config {
    unsafe { &mut *addr_of_mut!(CONFIG) }
}

pub fn accent() -> u32 {
    ACCENTS[cfg().accent.min(ACCENTS.len() - 1)].1
}

/// 0..=100 percent to 0..=255 alpha.
pub fn alpha(percent: u32) -> u32 {
    percent.min(100) * 255 / 100
}

fn flag(key: &str, default: bool) -> bool {
    match settings::get(key).as_deref() {
        Some("1") | Some("on") | Some("true") => true,
        Some("0") | Some("off") | Some("false") => false,
        _ => default,
    }
}

fn default_wallpaper() -> usize {
    // Prefer the darkest-looking option by name, else the first one.
    WALLPAPERS.iter().position(|w| w.name.eq_ignore_ascii_case("hkr")).unwrap_or(0)
}

/// Read the saved choices. Missing or invalid values fall back to defaults.
pub fn load() {
    let c = cfg();
    c.top_opacity = settings::get_u32("top_opacity", 55).min(100);
    c.dock_opacity = settings::get_u32("dock_opacity", 55).min(100);
    c.window_opacity = settings::get_u32("window_opacity", 82).clamp(MIN_WINDOW_OPACITY, 100);
    c.radius = settings::get_u32("corner_radius", 12).min(MAX_RADIUS);
    c.shadows = flag("shadows", true);
    c.clock_24h = flag("clock_24h", true);
    c.show_seconds = flag("clock_seconds", false);
    c.show_date = flag("clock_date", true);
    c.tz_offset = settings::get("tz_offset")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0)
        .clamp(-12, 14);

    c.accent = settings::get("accent")
        .and_then(|name| ACCENTS.iter().position(|a| a.0.eq_ignore_ascii_case(&name)))
        .unwrap_or(0);

    c.wallpaper = match settings::get("wallpaper") {
        Some(name) if name.eq_ignore_ascii_case("gradient") => WALLPAPERS.len(),
        Some(name) => WALLPAPERS
            .iter()
            .position(|w| w.name.eq_ignore_ascii_case(&name))
            .unwrap_or_else(default_wallpaper),
        None => default_wallpaper(),
    };
}

/// Write every choice back to disk.
pub fn save() {
    let c = cfg();
    let onoff = |b: bool| if b { "1" } else { "0" };
    settings::set("top_opacity", &format!("{}", c.top_opacity));
    settings::set("dock_opacity", &format!("{}", c.dock_opacity));
    settings::set("window_opacity", &format!("{}", c.window_opacity));
    settings::set("corner_radius", &format!("{}", c.radius));
    settings::set("shadows", onoff(c.shadows));
    settings::set("clock_24h", onoff(c.clock_24h));
    settings::set("clock_seconds", onoff(c.show_seconds));
    settings::set("clock_date", onoff(c.show_date));
    settings::set("tz_offset", &format!("{}", c.tz_offset));
    settings::set("accent", ACCENTS[c.accent.min(ACCENTS.len() - 1)].0);
    let wall = WALLPAPERS.get(c.wallpaper).map(|w| w.name).unwrap_or("Gradient");
    settings::set("wallpaper", wall);
    settings::flush_quietly();
}
