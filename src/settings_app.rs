//! The Settings app: a sidebar of categories on the left and one page of
//! cards on the right. Pages are written with the widget kit in ui.rs.

use crate::clock;
use crate::cpu;
use crate::framebuffer as fb;
use crate::fs;
use crate::gfx;
use crate::heap;
use crate::image::{self, icon, BODY, HERO, LABEL, SMALL, STRONG, TITLE, WALLPAPERS, WORDMARK};
use crate::net;
use crate::pci;
use crate::pit;
use crate::pmm;
use crate::scene;
use crate::theme::{self, ACCENTS, FAINT, GOOD, MUTED, PANEL, TEXT, WHITE};
use crate::ui::{Phase, Rect, Ui};
use crate::virtio_blk;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const ABOUT: usize = 0;
pub const DATE_TIME: usize = 1;
pub const NETWORK: usize = 2;
pub const WIFI: usize = 3;
pub const BLUETOOTH: usize = 4;
pub const THEME: usize = 5;
pub const WALLPAPER: usize = 6;
pub const DOCK: usize = 7;
pub const DISPLAY: usize = 8;
pub const STORAGE: usize = 9;

struct Page {
    name: &'static str,
    icon: usize,
    color: u32,
}

const PAGES: [Page; 10] = [
    Page { name: "About", icon: icon::INFO, color: 0x0064_6C7A },
    Page { name: "Date & Time", icon: icon::CLOCK, color: 0x00E5_484D },
    Page { name: "Network", icon: icon::ETHERNET, color: 0x003B_82F6 },
    Page { name: "Wi-Fi", icon: icon::WIFI, color: 0x000E_A5E9 },
    Page { name: "Bluetooth", icon: icon::BLUETOOTH, color: 0x0025_63EB },
    Page { name: "Theme", icon: icon::PALETTE, color: 0x00A8_55F7 },
    Page { name: "Wallpaper", icon: icon::IMAGE, color: 0x0014_B8A6 },
    Page { name: "Dock & Menu Bar", icon: icon::DOCK, color: 0x00F5_9E0B },
    Page { name: "Display", icon: icon::MONITOR, color: 0x0063_66F1 },
    Page { name: "Storage", icon: icon::DRIVE, color: 0x0064_748B },
];

/// Sidebar groups: a caption and the pages listed under it.
const GROUPS: [(&str, &[usize]); 4] = [
    ("GENERAL", &[ABOUT, DATE_TIME]),
    ("CONNECTIVITY", &[NETWORK, WIFI, BLUETOOTH]),
    ("PERSONALISATION", &[THEME, WALLPAPER, DOCK]),
    ("SYSTEM", &[DISPLAY, STORAGE]),
];

const SIDEBAR_W: i32 = 224;
const ITEM_H: i32 = 32;
const MARGIN: i32 = 28;

pub struct State {
    page: usize,
    /// Slider currently held by the mouse.
    active: Option<u32>,
    /// A value changed and has not been written to disk yet.
    dirty: bool,
    /// File list for the Storage page, read when the page is opened so
    /// that redrawing the window never touches the disk.
    files: Vec<(String, u32)>,
}

impl State {
    pub const fn new() -> State {
        State { page: ABOUT, active: None, dirty: false, files: Vec::new() }
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn set_page(&mut self, page: usize) {
        self.page = page;
        if page == STORAGE {
            self.files.clear();
            if virtio_blk::ready() {
                let files = &mut self.files;
                fs::list(|name, size| files.push((String::from(name), size)));
            }
        }
    }
}

/// What the window manager has to repaint after an event.
#[derive(PartialEq)]
pub enum Outcome {
    Nothing,
    Window,
    /// A setting changed that affects the whole desktop.
    Everything,
}

/// Pages that show live values and want a repaint every second.
pub fn is_live(page: usize) -> bool {
    page == ABOUT || page == DATE_TIME
}

/// Draw the app into the body of its window.
pub fn draw(body: Rect, st: &mut State) {
    sidebar(body, st, Phase::Draw, 0, 0);
    let mut active = st.active;
    let mut ui = Ui::new(Phase::Draw, 0, 0, content_area(body), &mut active);
    page(&mut ui, st);
}

/// Mouse button went down at (mx, my).
pub fn press(body: Rect, st: &mut State, mx: i32, my: i32) -> Outcome {
    if let Some(p) = sidebar(body, st, Phase::Press, mx, my) {
        st.set_page(p);
        return Outcome::Window;
    }
    run(body, st, Phase::Press, mx, my)
}

/// Pointer moved with the button held.
pub fn drag(body: Rect, st: &mut State, mx: i32, my: i32) -> Outcome {
    if st.active.is_none() {
        return Outcome::Nothing;
    }
    run(body, st, Phase::Drag, mx, my)
}

/// Mouse button went up. Saves anything that changed.
pub fn release(st: &mut State) {
    st.active = None;
    if st.dirty {
        st.dirty = false;
        theme::save();
    }
}

fn run(body: Rect, st: &mut State, phase: Phase, mx: i32, my: i32) -> Outcome {
    let mut active = st.active;
    let mut ui = Ui::new(phase, mx, my, content_area(body), &mut active);
    page(&mut ui, st);
    let changed = ui.changed;
    st.active = active;
    if changed {
        st.dirty = true;
        // Switches and buttons save at once. A slider saves on release.
        if st.active.is_none() {
            st.dirty = false;
            theme::save();
        }
        Outcome::Everything
    } else {
        Outcome::Nothing
    }
}

fn content_area(body: Rect) -> Rect {
    Rect::new(
        body.x + SIDEBAR_W + MARGIN,
        body.y + 22,
        (body.w - SIDEBAR_W - 2 * MARGIN).max(200),
        body.h,
    )
}

/// Draws the sidebar, or in the Press phase returns the page that was hit.
fn sidebar(body: Rect, st: &State, phase: Phase, mx: i32, my: i32) -> Option<usize> {
    let drawing = phase == Phase::Draw;
    if drawing {
        gfx::rect_a(body.x, body.y, SIDEBAR_W, body.h, PANEL, 110);
        gfx::rect_a(body.x + SIDEBAR_W, body.y, 1, body.h, WHITE, theme::LINE_A);
    }

    let mut y = body.y + 14;
    for (caption, pages) in GROUPS.iter() {
        if drawing {
            gfx::text(body.x + 20, y + 4, &SMALL, caption, FAINT);
        }
        y += 26;
        for &p in pages.iter() {
            let item = Rect::new(body.x + 10, y, SIDEBAR_W - 20, ITEM_H - 2);
            if drawing {
                let selected = p == st.page;
                if selected {
                    gfx::round(item.x, item.y, item.w, item.h, 8, theme::accent());
                }
                let entry = &PAGES[p];
                gfx::round(item.x + 8, item.y + 4, 22, 22, 6, entry.color);
                gfx::icon(item.x + 11, item.y + 7, image::icon16(entry.icon), WHITE);
                let font = if selected { &STRONG } else { &LABEL };
                let color = if selected { WHITE } else { TEXT };
                gfx::text(item.x + 40, item.y + (item.h - font.height) / 2, font, entry.name, color);
            } else if phase == Phase::Press && item.contains(mx, my) {
                return Some(p);
            }
            y += ITEM_H;
        }
        y += 8;
    }
    None
}

fn page(ui: &mut Ui, st: &State) {
    match st.page {
        ABOUT => about(ui),
        DATE_TIME => date_time(ui),
        NETWORK => network(ui),
        WIFI => wifi(ui),
        BLUETOOTH => bluetooth(ui),
        THEME => theme_page(ui),
        WALLPAPER => wallpaper(ui),
        DOCK => dock(ui),
        DISPLAY => display(ui),
        _ => storage(ui, &st.files),
    }
}

// ---- General -------------------------------------------------------------

fn uptime() -> String {
    let secs = pit::ticks() / pit::HZ;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{} h {} min", h, m)
    } else if m > 0 {
        format!("{} min {} s", m, s)
    } else {
        format!("{} s", s)
    }
}

fn about(ui: &mut Ui) {
    if ui.drawing() {
        gfx::image_a(ui.x, ui.y, &WORDMARK, 255);
        let tx = ui.x + WORDMARK.w as i32 + 24;
        gfx::text(tx, ui.y + 22, &TITLE, "AnarchyOS", TEXT);
        gfx::text(tx, ui.y + 54, &BODY, "Version 0.1.0", MUTED);
    }
    ui.gap(WORDMARK.h as i32 + 22);

    ui.card(6);
    ui.info("Kernel", "anarchy-kernel 0.1.0, x86_64");
    ui.info("Processor", cpu::brand());
    ui.info("Memory", &format!("{} MiB", pmm::total_bytes() / (1024 * 1024)));
    ui.info(
        "Kernel heap",
        &format!("{} of {} MiB free", heap::free_bytes() / (1024 * 1024), heap::HEAP_SIZE / (1024 * 1024)),
    );
    ui.info("Display", &format!("{} x {}", fb::width(), fb::height()));
    ui.info("Uptime", &uptime());
}

fn date_time(ui: &mut Ui) {
    if ui.drawing() {
        gfx::text(ui.x, ui.y - 6, &HERO, &clock::time_string(true), TEXT);
        gfx::text(ui.x + 2, ui.y + 50, &BODY, &clock::long_date(), MUTED);
    }
    ui.gap(92);

    let c = theme::cfg();
    ui.section("Clock");
    ui.card(3);
    ui.toggle("24-hour time", &mut c.clock_24h);
    ui.toggle("Show seconds in the menu bar", &mut c.show_seconds);
    ui.toggle("Show the date in the menu bar", &mut c.show_date);

    ui.section("Adjustment");
    ui.card(1);
    let text = format!("{}{} h", if c.tz_offset >= 0 { "+" } else { "" }, c.tz_offset);
    ui.stepper("Offset from the hardware clock", &mut c.tz_offset, -12, 14, &text);
    ui.note("Time comes from the machine's battery-backed clock. If that clock is");
    ui.note("set to UTC, add your time zone's offset here.");
}

// ---- Connectivity --------------------------------------------------------

fn network(ui: &mut Ui) {
    ui.heading("Network", "Wired Ethernet through the virtio network card.");
    let info = net::info();

    ui.card(1);
    if info.up {
        ui.status("Ethernet", "Link up", GOOD);
    } else {
        ui.status("Ethernet", "No adapter", FAINT);
    }

    if info.up {
        ui.section("Details");
        ui.card(5);
        ui.info("Hardware address", &info.mac);
        ui.info("IPv4 address", &info.ip);
        ui.info("Gateway", &info.gateway);
        ui.info("Known neighbours", &format!("{}", info.arp_entries));
        if ui.button("Ask the gateway to identify itself", "Renew") {
            net::renew();
        }
        ui.note("Addresses are fixed for now. Automatic setup (DHCP), DNS and TCP are not built yet.");
    } else {
        ui.note("No supported network card was found at boot.");
    }
}

fn radio(ui: &mut Ui, name: &str, why: &str) {
    let found: Vec<String> = pci::wireless_found();
    ui.card(1);
    if found.is_empty() {
        ui.status(name, "No adapter found", FAINT);
    } else {
        ui.status(name, "No driver yet", theme::accent());
    }

    if !found.is_empty() {
        ui.section("Detected hardware");
        ui.card(found.len() as u32);
        for id in &found {
            ui.info("Wireless controller", id);
        }
    }
    ui.note(why);
}

fn wifi(ui: &mut Ui) {
    ui.heading("Wi-Fi", "Wireless networks.");
    radio(ui, "Wi-Fi", "Wi-Fi drivers are not built yet. Adapters are detected but cannot connect.");
}

fn bluetooth(ui: &mut Ui) {
    ui.heading("Bluetooth", "Wireless keyboards, mice and audio.");
    radio(ui, "Bluetooth", "Bluetooth controllers sit behind USB, and USB support is not built yet.");
}

// ---- Personalisation -----------------------------------------------------

fn theme_page(ui: &mut Ui) {
    ui.heading("Theme", "Colours and the look of windows.");
    let c = theme::cfg();

    ui.card(2);
    ui.info("Appearance", "Dark");
    let colors: [u32; 6] = core::array::from_fn(|i| ACCENTS[i].1);
    if ui.swatches("Accent colour", &colors, &mut c.accent) && c.wallpaper >= WALLPAPERS.len() {
        // The gradient wallpaper is tinted with the accent.
        scene::apply();
    }

    ui.section("Windows");
    ui.card(3);
    let mut see_through = 100 - c.window_opacity;
    if ui.slider("Transparency", &mut see_through, 0, 100 - theme::MIN_WINDOW_OPACITY, "%") {
        c.window_opacity = 100 - see_through;
    }
    ui.slider("Corner radius", &mut c.radius, 0, theme::MAX_RADIUS, " px");
    ui.toggle("Shadows", &mut c.shadows);
}

fn wallpaper(ui: &mut Ui) {
    ui.heading("Wallpaper", "Pictures come from assets/wallpaper and are built into the system.");
    let c = theme::cfg();

    const GAP: i32 = 16;
    let (tw, th) = match WALLPAPERS.first() {
        Some(w) => (w.thumb.w as i32, w.thumb.h as i32),
        None => (192, 120),
    };
    let cols = ((ui.w + GAP) / (tw + GAP)).max(1);
    let count = WALLPAPERS.len() + 1;

    for i in 0..count {
        let col = i as i32 % cols;
        let row = i as i32 / cols;
        let tile = Rect::new(ui.x + col * (tw + GAP), ui.y + row * (th + 34), tw, th);
        let name = WALLPAPERS.get(i).map(|w| w.name).unwrap_or("Gradient");
        if ui.drawing() {
            match WALLPAPERS.get(i) {
                Some(w) => gfx::image_round(tile.x, tile.y, &w.thumb, 10),
                None => gfx::round_grad(tile.x, tile.y, tw, th, 10, scene::GRADIENT_TOP, scene::gradient_bottom(), 255),
            }
            gfx::stroke(tile.x, tile.y, tw, th, 10, WHITE, theme::EDGE_A);
            if c.wallpaper == i {
                ui.ring(tile, 10);
            }
            let color = if c.wallpaper == i { TEXT } else { MUTED };
            gfx::text_center(tile.x + tw / 2, tile.y + th + 8, &SMALL, name, color);
        } else if ui.pressed(tile) && c.wallpaper != i {
            c.wallpaper = i;
            scene::apply();
            ui.changed = true;
        }
    }
    let rows = (count as i32 + cols - 1) / cols;
    ui.gap(rows * (th + 34));
}

fn dock(ui: &mut Ui) {
    ui.heading("Dock & Menu Bar", "How solid the bars at the top and bottom of the screen are.");
    let c = theme::cfg();
    ui.card(2);
    ui.slider("Menu bar opacity", &mut c.top_opacity, 0, 100, "%");
    ui.slider("Dock opacity", &mut c.dock_opacity, 0, 100, "%");
    ui.note("Clock options are under Date & Time.");
}

// ---- System --------------------------------------------------------------

fn display(ui: &mut Ui) {
    ui.heading("Display", "The screen the desktop is drawn on.");
    ui.card(4);
    ui.info("Resolution", &format!("{} x {}", fb::width(), fb::height()));
    ui.info("Colour depth", "32-bit");
    ui.info("Rendering", "Software, double buffered");
    ui.info("Wallpaper", if scene::is_scaled() { "Scaled to fit" } else { "Native size" });
    ui.note("The bootloader picks the resolution. Change it with the resolution line in limine.conf.");
}

fn storage(ui: &mut Ui, files: &[(String, u32)]) {
    ui.heading("Storage", "The virtio disk and the files on it.");
    if !virtio_blk::ready() {
        ui.card(1);
        ui.status("Disk", "Not attached", FAINT);
        ui.note("Settings cannot be saved without a disk, so they reset at reboot.");
        return;
    }

    let total = virtio_blk::sectors() * 512;
    // One superblock sector, eight directory sectors, then whole sectors per file.
    let used = 9 * 512 + files.iter().map(|f| (f.1 as u64 + 511) / 512 * 512).sum::<u64>();

    ui.card(2);
    ui.meter("AnarchyFS", &format!("{} KiB of {} KiB used", used / 1024, total / 1024), used, total);
    ui.info("Files", &format!("{}", files.len()));

    if !files.is_empty() {
        ui.section("Files");
        let shown = files.len().min(6);
        ui.card(shown as u32);
        for (name, size) in files.iter().take(shown) {
            ui.info(name, &format!("{} B", size));
        }
        if files.len() > shown {
            ui.note(&format!("and {} more. Use ls in the Terminal to see them all.", files.len() - shown));
        }
    }
}
