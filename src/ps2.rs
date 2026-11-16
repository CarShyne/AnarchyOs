//! PS/2 keyboard and mouse through the 8042 controller. Keyboard input is
//! decoded from scan code set 1 and shown on the screen. Mouse packets move
//! a software cursor, and left clicks are logged to serial.

use crate::arch::x86_64::idt::{self, InterruptStackFrame};
use crate::framebuffer as fb;
use crate::pic;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};

const DATA: u16 = 0x60;
const STATUS_CMD: u16 = 0x64;

const KEYBOARD_VECTOR: u8 = 33; // IRQ1
const MOUSE_VECTOR: u8 = 44; // IRQ12

#[inline]
unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value,
         options(nomem, nostack, preserves_flags));
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!("in al, dx", out("al") value, in("dx") port,
         options(nomem, nostack, preserves_flags));
    value
}

fn wait_write() {
    while unsafe { inb(STATUS_CMD) } & 0b10 != 0 {}
}

fn wait_read() {
    while unsafe { inb(STATUS_CMD) } & 0b1 == 0 {}
}

fn controller_cmd(cmd: u8) {
    wait_write();
    unsafe { outb(STATUS_CMD, cmd) };
}

fn write_data(value: u8) {
    wait_write();
    unsafe { outb(DATA, value) };
}

fn read_data() -> u8 {
    wait_read();
    unsafe { inb(DATA) }
}

/// Send a byte to the mouse (second PS/2 port) through the controller.
fn mouse_write(value: u8) {
    controller_cmd(0xD4);
    write_data(value);
}

// ---- Keyboard ----------------------------------------------------------

static SHIFT: AtomicBool = AtomicBool::new(false);
const TYPED_MAX: usize = 40;
static TYPED: [AtomicU8; TYPED_MAX] = [const { AtomicU8::new(0) }; TYPED_MAX];
static TYPED_LEN: AtomicUsize = AtomicUsize::new(0);

/// Map a make scan code (set 1) to ASCII. Zero means no printable character.
fn decode(sc: u8, shift: bool) -> u8 {
    fn letter(c: u8, shift: bool) -> u8 {
        if shift { c.to_ascii_uppercase() } else { c }
    }
    match sc {
        0x02..=0x0D => {
            let i = (sc - 0x02) as usize;
            if shift { b"!@#$%^&*()_+"[i] } else { b"1234567890-="[i] }
        }
        0x10..=0x19 => letter(b"qwertyuiop"[(sc - 0x10) as usize], shift),
        0x1E..=0x26 => letter(b"asdfghjkl"[(sc - 0x1E) as usize], shift),
        0x2C..=0x32 => letter(b"zxcvbnm"[(sc - 0x2C) as usize], shift),
        0x39 => b' ',
        0x1C => b'\n',
        0x0E => 0x08, // backspace
        _ => 0,
    }
}

fn redraw_typed() {
    fb::fill_rect(14, 44, 900, 20, 0x0010_1828);
    let len = TYPED_LEN.load(Ordering::Relaxed);
    let mut buf = [0u8; TYPED_MAX];
    for (i, slot) in buf.iter_mut().enumerate().take(len) {
        *slot = TYPED[i].load(Ordering::Relaxed);
    }
    fb::draw_text(14, 46, "> ", 0x0070_D694, 2);
    let text = core::str::from_utf8(&buf[..len]).unwrap_or("");
    fb::draw_text(46, 46, text, 0x00E6_ECF5, 2);
}

fn push_key(c: u8) {
    let len = TYPED_LEN.load(Ordering::Relaxed);
    if c == 0x08 {
        if len > 0 {
            TYPED_LEN.store(len - 1, Ordering::Relaxed);
        }
    } else if c == b'\n' {
        TYPED_LEN.store(0, Ordering::Relaxed);
    } else if len < TYPED_MAX {
        TYPED[len].store(c, Ordering::Relaxed);
        TYPED_LEN.store(len + 1, Ordering::Relaxed);
    }
    redraw_typed();
}

extern "x86-interrupt" fn keyboard_irq(_frame: InterruptStackFrame) {
    let sc = unsafe { inb(DATA) };
    match sc {
        0x2A | 0x36 => SHIFT.store(true, Ordering::Relaxed),  // shift pressed
        0xAA | 0xB6 => SHIFT.store(false, Ordering::Relaxed), // shift released
        _ if sc & 0x80 == 0 => {
            let c = decode(sc, SHIFT.load(Ordering::Relaxed));
            if c != 0 {
                push_key(c);
            }
        }
        _ => {} // other key releases are ignored for now
    }
    pic::eoi(1);
}

// ---- Mouse -------------------------------------------------------------

const CUR_BOX: u32 = 12;
static SAVED: [AtomicU32; (CUR_BOX * CUR_BOX) as usize] =
    [const { AtomicU32::new(0) }; (CUR_BOX * CUR_BOX) as usize];
static CUR_X: AtomicU32 = AtomicU32::new(0);
static CUR_Y: AtomicU32 = AtomicU32::new(0);
static LEFT_DOWN: AtomicBool = AtomicBool::new(false);
static PACKET: [AtomicU8; 3] = [const { AtomicU8::new(0) }; 3];
static PKT_IDX: AtomicUsize = AtomicUsize::new(0);

fn save_under(x: u32, y: u32) {
    for dy in 0..CUR_BOX {
        for dx in 0..CUR_BOX {
            let px = fb::get_pixel(x + dx, y + dy);
            SAVED[(dy * CUR_BOX + dx) as usize].store(px, Ordering::Relaxed);
        }
    }
}

fn restore(x: u32, y: u32) {
    for dy in 0..CUR_BOX {
        for dx in 0..CUR_BOX {
            let px = SAVED[(dy * CUR_BOX + dx) as usize].load(Ordering::Relaxed);
            fb::put_pixel(x + dx, y + dy, px);
        }
    }
}

/// Draw a simple white arrow in the top-left of the cursor box.
fn paint(x: u32, y: u32) {
    for row in 0..10u32 {
        let width = (row + 1).min(7);
        for col in 0..width {
            fb::put_pixel(x + col, y + row, 0x00FF_FFFF);
        }
    }
}

fn move_cursor(nx: u32, ny: u32) {
    let ox = CUR_X.load(Ordering::Relaxed);
    let oy = CUR_Y.load(Ordering::Relaxed);
    restore(ox, oy);
    save_under(nx, ny);
    paint(nx, ny);
    CUR_X.store(nx, Ordering::Relaxed);
    CUR_Y.store(ny, Ordering::Relaxed);
}

fn handle_packet(p: [u8; 3]) {
    let flags = p[0];
    // The sign bits in the flags byte extend the 8-bit movement deltas.
    let mut dx = p[1] as i32;
    if flags & 0x10 != 0 {
        dx -= 256;
    }
    let mut dy = p[2] as i32;
    if flags & 0x20 != 0 {
        dy -= 256;
    }

    let max_x = (fb::width() - CUR_BOX) as i32;
    let max_y = (fb::height() - CUR_BOX) as i32;
    // PS/2 reports Y increasing upward, screen Y increases downward.
    let x = (CUR_X.load(Ordering::Relaxed) as i32 + dx).clamp(0, max_x) as u32;
    let y = (CUR_Y.load(Ordering::Relaxed) as i32 - dy).clamp(0, max_y) as u32;
    move_cursor(x, y);

    let left = flags & 0b001 != 0;
    if left && !LEFT_DOWN.load(Ordering::Relaxed) {
        crate::kprintln!("click at ({}, {})", x, y);
    }
    LEFT_DOWN.store(left, Ordering::Relaxed);
}

extern "x86-interrupt" fn mouse_irq(_frame: InterruptStackFrame) {
    let byte = unsafe { inb(DATA) };
    let idx = PKT_IDX.load(Ordering::Relaxed);

    // The first byte of every packet has bit 3 set. If it isn't, we have
    // lost sync, so skip this byte and wait for a valid first byte.
    if idx == 0 && byte & 0x08 == 0 {
        // out of sync, drop the byte
    } else {
        PACKET[idx].store(byte, Ordering::Relaxed);
        if idx == 2 {
            PKT_IDX.store(0, Ordering::Relaxed);
            let p = [
                PACKET[0].load(Ordering::Relaxed),
                PACKET[1].load(Ordering::Relaxed),
                PACKET[2].load(Ordering::Relaxed),
            ];
            handle_packet(p);
        } else {
            PKT_IDX.store(idx + 1, Ordering::Relaxed);
        }
    }
    pic::eoi(12);
}

// ---- Setup -------------------------------------------------------------

pub fn init() {
    // Disable both ports while we configure the controller.
    controller_cmd(0xAD);
    controller_cmd(0xA7);

    // Flush any stale bytes from the output buffer.
    while unsafe { inb(STATUS_CMD) } & 0b1 != 0 {
        let _ = unsafe { inb(DATA) };
    }

    // Enable keyboard (IRQ1) and mouse (IRQ12) interrupts, and make sure
    // both port clocks are running.
    controller_cmd(0x20);
    let mut config = read_data();
    config |= 0b0000_0011;
    config &= !0b0011_0000;
    controller_cmd(0x60);
    write_data(config);

    // Re-enable both ports.
    controller_cmd(0xAE);
    controller_cmd(0xA8);

    // Put the mouse in its defaults, then start streaming packets.
    mouse_write(0xF6);
    let _ = read_data(); // ACK
    mouse_write(0xF4);
    let _ = read_data(); // ACK

    // Park the cursor in the middle of the screen before IRQs can move it.
    let cx = fb::width() / 2;
    let cy = fb::height() / 2;
    CUR_X.store(cx, Ordering::Relaxed);
    CUR_Y.store(cy, Ordering::Relaxed);
    save_under(cx, cy);
    paint(cx, cy);

    idt::set_irq(KEYBOARD_VECTOR, keyboard_irq as *const () as u64);
    idt::set_irq(MOUSE_VECTOR, mouse_irq as *const () as u64);
    pic::remap();

    crate::kprintln!("ps2: keyboard and mouse ready");
}
