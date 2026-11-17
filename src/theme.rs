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
pub const MAX_BORDER: u32 = 4;
/// Largest blur radius in pixels, reached at 100 percent.
pub const MAX_BLUR_RADIUS: u32 = 36;
pub const MAX_TINT: u32 = 40;
pub const MAX_DESK_TINT: u32 = 40;

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

    // Rice: window styling.
    /// Border thickness in pixels. Zero means no border.
    pub border_width: u32,
    /// Index into ACCENTS for the border colour.
    pub border_color: usize,
    /// Draw the border only on the focused window.
    pub border_focus_only: bool,
    /// Shadow darkness as a percentage of the normal shadow.
    pub shadow_strength: u32,
    /// Frosted-glass blur, 0..=100 percent of MAX_BLUR_RADIUS.
    pub blur: u32,
    /// Accent colour mixed into the window glass, in percent.
    pub tint: u32,
    /// A soft accent glow around the focused window.
    pub focus_glow: bool,

    // Rice: dock and menu bar effects.
    pub dock_magnify: bool,
    pub dock_hover: bool,
    pub dock_bounce: bool,
    pub bar_hover: bool,

    // Rice: wallpaper mods, baked into the desktop picture.
    pub scanlines: bool,
    pub vignette: bool,
    pub grain: bool,
    /// Accent colour washed over the wallpaper, in percent.
    pub desk_tint: u32,
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
    border_width: 0,
    border_color: 0,
    border_focus_only: true,
    shadow_strength: 100,
    blur: 39,
    tint: 0,
    focus_glow: false,
    dock_magnify: false,
    dock_hover: true,
    dock_bounce: true,
    bar_hover: true,
    scanlines: false,
    vignette: false,
    grain: false,
    desk_tint: 0,
};

// Only the main loop reads or writes the config.
pub fn cfg() -> &'static mut Config {
    unsafe { &mut *addr_of_mut!(CONFIG) }
}

pub fn accent() -> u32 {
    ACCENTS[cfg().accent.min(ACCENTS.len() - 1)].1
}

/// The accent colour chosen for window borders.
pub fn border_colour() -> u32 {
    ACCENTS[cfg().border_color.min(ACCENTS.len() - 1)].1
}

/// Blur radius in pixels for the current blur setting.
pub fn blur_radius() -> usize {
    (cfg().blur.min(100) * MAX_BLUR_RADIUS / 100) as usize
}

/// 0..=100 percent to 0..=255 alpha.
pub fn alpha(percent: u32) -> u32 {
    percent.min(100) * 255 / 100
}

/// Whether any wallpaper mod is switched on, so the desktop needs rebuilding.
pub fn wallpaper_mods() -> bool {
    let c = cfg();
    c.scanlines || c.vignette || c.grain || c.desk_tint > 0
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

    c.border_width = settings::get_u32("border_width", 0).min(MAX_BORDER);
    c.border_color = settings::get("border_colour")
        .and_then(|name| ACCENTS.iter().position(|a| a.0.eq_ignore_ascii_case(&name)))
        .unwrap_or(0);
    c.border_focus_only = flag("border_focus_only", true);
    c.shadow_strength = settings::get_u32("shadow_strength", 100).min(100);
    c.blur = settings::get_u32("blur", 39).min(100);
    c.tint = settings::get_u32("tint", 0).min(MAX_TINT);
    c.focus_glow = flag("focus_glow", false);

    c.dock_magnify = flag("dock_magnify", false);
    c.dock_hover = flag("dock_hover", true);
    c.dock_bounce = flag("dock_bounce", true);
    c.bar_hover = flag("bar_hover", true);

    c.scanlines = flag("scanlines", false);
    c.vignette = flag("vignette", false);
    c.grain = flag("grain", false);
    c.desk_tint = settings::get_u32("desk_tint", 0).min(MAX_DESK_TINT);
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

    settings::set("border_width", &format!("{}", c.border_width));
    settings::set("border_colour", ACCENTS[c.border_color.min(ACCENTS.len() - 1)].0);
    settings::set("border_focus_only", onoff(c.border_focus_only));
    settings::set("shadow_strength", &format!("{}", c.shadow_strength));
    settings::set("blur", &format!("{}", c.blur));
    settings::set("tint", &format!("{}", c.tint));
    settings::set("focus_glow", onoff(c.focus_glow));

    settings::set("dock_magnify", onoff(c.dock_magnify));
    settings::set("dock_hover", onoff(c.dock_hover));
    settings::set("dock_bounce", onoff(c.dock_bounce));
    settings::set("bar_hover", onoff(c.bar_hover));

    settings::set("scanlines", onoff(c.scanlines));
    settings::set("vignette", onoff(c.vignette));
    settings::set("grain", onoff(c.grain));
    settings::set("desk_tint", &format!("{}", c.desk_tint));
    settings::flush_quietly();
}
