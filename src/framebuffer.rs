//! The screen itself. Everything draws into a back buffer on the heap, and
//! `present` copies a damaged rectangle to the real screen. Before the heap
//! exists, drawing goes straight to the screen.
//!
//! This file owns the buffers and the clip rectangle, plus the few blocky
//! primitives the boot console needs. The desktop draws through gfx.rs.

use alloc::vec;
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use font8x8::legacy::BASIC_LEGACY;

static ADDR: AtomicU64 = AtomicU64::new(0); // real screen
static BACK: AtomicU64 = AtomicU64::new(0); // off-screen buffer, 0 until allocated
static WIDTH: AtomicU64 = AtomicU64::new(0);
static HEIGHT: AtomicU64 = AtomicU64::new(0);
static PITCH: AtomicU64 = AtomicU64::new(0);

static CLIP_X0: AtomicU32 = AtomicU32::new(0);
static CLIP_Y0: AtomicU32 = AtomicU32::new(0);
static CLIP_X1: AtomicU32 = AtomicU32::new(0);
static CLIP_Y1: AtomicU32 = AtomicU32::new(0);

pub fn init(addr: u64, width: u64, height: u64, pitch: u64) {
    ADDR.store(addr, Ordering::Relaxed);
    WIDTH.store(width, Ordering::Relaxed);
    HEIGHT.store(height, Ordering::Relaxed);
    PITCH.store(pitch, Ordering::Relaxed);
    reset_clip();
}

/// Allocate the back buffer. Call once the heap is ready.
pub fn alloc_backbuffer() {
    let size = (PITCH.load(Ordering::Relaxed) * HEIGHT.load(Ordering::Relaxed)) as usize;
    let buf = vec![0u8; size].into_boxed_slice();
    let addr = alloc::boxed::Box::leak(buf).as_ptr() as u64;
    BACK.store(addr, Ordering::Relaxed);
}

pub fn width() -> u32 {
    WIDTH.load(Ordering::Relaxed) as u32
}

pub fn height() -> u32 {
    HEIGHT.load(Ordering::Relaxed) as u32
}

/// Restrict all drawing to a rectangle. Clamped to the screen.
pub fn set_clip(x: u32, y: u32, w: u32, h: u32) {
    let x0 = x.min(width());
    let y0 = y.min(height());
    let x1 = x.saturating_add(w).min(width());
    let y1 = y.saturating_add(h).min(height());
    CLIP_X0.store(x0, Ordering::Relaxed);
    CLIP_Y0.store(y0, Ordering::Relaxed);
    CLIP_X1.store(x1, Ordering::Relaxed);
    CLIP_Y1.store(y1, Ordering::Relaxed);
}

pub fn reset_clip() {
    set_clip(0, 0, width(), height());
}

fn clip() -> (u32, u32, u32, u32) {
    (
        CLIP_X0.load(Ordering::Relaxed),
        CLIP_Y0.load(Ordering::Relaxed),
        CLIP_X1.load(Ordering::Relaxed),
        CLIP_Y1.load(Ordering::Relaxed),
    )
}

/// The buffer drawing goes to: the back buffer once it exists.
fn target() -> u64 {
    match BACK.load(Ordering::Relaxed) {
        0 => ADDR.load(Ordering::Relaxed),
        b => b,
    }
}

fn offset(x: u32, y: u32) -> u64 {
    y as u64 * PITCH.load(Ordering::Relaxed) + x as u64 * 4
}

/// Write without a clip check. Callers must already be inside the clip.
fn raw_put(x: u32, y: u32, color: u32) {
    unsafe { ptr::write_volatile((target() + offset(x, y)) as *mut u32, color) };
}

/// Copy a rectangle from the back buffer to the screen. Without a back
/// buffer, drawing already went to the screen, so this does nothing.
pub fn present(x: u32, y: u32, w: u32, h: u32) {
    let back = BACK.load(Ordering::Relaxed);
    if back == 0 {
        return;
    }
    let front = ADDR.load(Ordering::Relaxed);
    let x1 = x.saturating_add(w).min(width());
    let y1 = y.saturating_add(h).min(height());
    let bytes = ((x1.saturating_sub(x)) * 4) as usize;
    for py in y..y1 {
        let off = offset(x, py);
        unsafe {
            ptr::copy_nonoverlapping(
                (back + off) as *const u8,
                (front + off) as *mut u8,
                bytes,
            );
        }
    }
}

pub fn fill_rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    let (cx0, cy0, cx1, cy1) = clip();
    let x0 = x.max(cx0);
    let y0 = y.max(cy0);
    let x1 = x.saturating_add(w).min(cx1);
    let y1 = y.saturating_add(h).min(cy1);
    for py in y0..y1 {
        for px in x0..x1 {
            raw_put(px, py, color);
        }
    }
}

pub fn clear(color: u32) {
    fill_rect(0, 0, width(), height(), color);
}

pub fn draw_char(x: u32, y: u32, c: u8, color: u32, scale: u32) {
    let glyph = if (c as usize) < BASIC_LEGACY.len() {
        BASIC_LEGACY[c as usize]
    } else {
        BASIC_LEGACY[b'?' as usize]
    };
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..8u32 {
            if (*bits >> col) & 1 == 1 {
                fill_rect(x + col * scale, y + row as u32 * scale, scale, scale, color);
            }
        }
    }
}

/// Move everything up by `rows` pixel rows on the screen, then clear the
/// freed bottom rows. Used by the boot console only.
pub fn scroll_up(rows: u32, color: u32) {
    let h = height();
    let rows = rows.min(h);
    let pitch = PITCH.load(Ordering::Relaxed);
    let base = ADDR.load(Ordering::Relaxed);
    unsafe {
        ptr::copy(
            (base + rows as u64 * pitch) as *const u8,
            base as *mut u8,
            ((h - rows) as u64 * pitch) as usize,
        );
    }
    fill_rect(0, h - rows, width(), rows, color);
}

/// The current clip rectangle as (x0, y0, x1, y1).
pub fn current_clip() -> (u32, u32, u32, u32) {
    clip()
}

/// Restore a clip rectangle returned by `current_clip`.
pub fn restore_clip(c: (u32, u32, u32, u32)) {
    CLIP_X0.store(c.0, Ordering::Relaxed);
    CLIP_Y0.store(c.1, Ordering::Relaxed);
    CLIP_X1.store(c.2, Ordering::Relaxed);
    CLIP_Y1.store(c.3, Ordering::Relaxed);
}

/// The buffer drawing goes to, as (first pixel, pixels per row). Used by
/// gfx.rs, which does its own clipping against `current_clip`.
pub fn surface() -> (*mut u32, usize) {
    (target() as *mut u32, (PITCH.load(Ordering::Relaxed) / 4) as usize)
}
