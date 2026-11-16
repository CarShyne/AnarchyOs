//! Text console on the framebuffer. Mirrors everything kprintln writes,
//! wraps at the right edge, and scrolls at the bottom.

use crate::framebuffer as fb;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const SCALE: u32 = 2;
const CELL_W: u32 = 8 * SCALE;
const CELL_H: u32 = 8 * SCALE;
const FG: u32 = 0x00C8_D3E0;
const BG: u32 = 0x000B_0D12;

static ENABLED: AtomicBool = AtomicBool::new(false);
static CURSOR_X: AtomicU32 = AtomicU32::new(0);
static CURSOR_Y: AtomicU32 = AtomicU32::new(0);

/// Clear the screen and start mirroring output.
pub fn start() {
    fb::clear(BG);
    CURSOR_X.store(0, Ordering::Relaxed);
    CURSOR_Y.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
}

/// Stop mirroring. Serial output continues. Used before drawing a scene
/// over the console.
pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}

fn newline() {
    CURSOR_X.store(0, Ordering::Relaxed);
    let y = CURSOR_Y.load(Ordering::Relaxed) + CELL_H;
    if y + CELL_H > fb::height() {
        fb::scroll_up(CELL_H, BG);
        CURSOR_Y.store(fb::height() - CELL_H, Ordering::Relaxed);
    } else {
        CURSOR_Y.store(y, Ordering::Relaxed);
    }
}

fn put_byte(b: u8) {
    if b == b'\n' {
        newline();
        return;
    }
    if CURSOR_X.load(Ordering::Relaxed) + CELL_W > fb::width() {
        newline();
    }
    let x = CURSOR_X.load(Ordering::Relaxed);
    let y = CURSOR_Y.load(Ordering::Relaxed);
    fb::draw_char(x, y, b, FG, SCALE);
    CURSOR_X.store(x + CELL_W, Ordering::Relaxed);
}

struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            put_byte(b);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    if ENABLED.load(Ordering::Relaxed) {
        let _ = Console.write_fmt(args);
    }
}
