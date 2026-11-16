//! PS/2 keyboard and mouse through the 8042 controller. Decoded keys go to
//! the window manager, and mouse packets become pointer events.

use crate::arch::x86_64::idt::{self, InterruptStackFrame};
use crate::framebuffer as fb;
use crate::pic;
use crate::wm;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

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

pub const TYPED_MAX: usize = 40;
static SHIFT: AtomicBool = AtomicBool::new(false);
static TYPED: [AtomicU8; TYPED_MAX] = [const { AtomicU8::new(0) }; TYPED_MAX];
static TYPED_LEN: AtomicUsize = AtomicUsize::new(0);

/// Copy the Keys window's text into `buf`, returning its length.
pub fn copy_typed(buf: &mut [u8; TYPED_MAX]) -> usize {
    let len = TYPED_LEN.load(Ordering::Relaxed);
    for i in 0..len {
        buf[i] = TYPED[i].load(Ordering::Relaxed);
    }
    len
}

/// Keys typed while the Keys window is focused.
pub fn type_key(c: u8) {
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
}

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

extern "x86-interrupt" fn keyboard_irq(_frame: InterruptStackFrame) {
    let sc = unsafe { inb(DATA) };
    match sc {
        0x2A | 0x36 => SHIFT.store(true, Ordering::Relaxed),
        0xAA | 0xB6 => SHIFT.store(false, Ordering::Relaxed),
        _ if sc & 0x80 == 0 => {
            let c = decode(sc, SHIFT.load(Ordering::Relaxed));
            if c != 0 {
                wm::key(c);
            }
        }
        _ => {}
    }
    pic::eoi(1);
}

// ---- Mouse -------------------------------------------------------------

static PACKET: [AtomicU8; 3] = [const { AtomicU8::new(0) }; 3];
static PKT_IDX: AtomicUsize = AtomicUsize::new(0);

fn clamp_axis(v: i32, max: u32) -> u32 {
    v.max(0).min(max as i32 - 1) as u32
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

    let (cx, cy) = wm::cursor();
    // PS/2 reports Y increasing upward, screen Y increases downward.
    let x = clamp_axis(cx as i32 + dx, fb::width());
    let y = clamp_axis(cy as i32 - dy, fb::height());
    wm::pointer(x, y, flags & 0b001 != 0);
}

extern "x86-interrupt" fn mouse_irq(_frame: InterruptStackFrame) {
    let byte = unsafe { inb(DATA) };
    let idx = PKT_IDX.load(Ordering::Relaxed);

    // The first byte of every packet has bit 3 set. If it isn't, we have
    // lost sync, so drop this byte and wait for a valid first byte.
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

    idt::set_irq(KEYBOARD_VECTOR, keyboard_irq as *const () as u64);
    idt::set_irq(MOUSE_VECTOR, mouse_irq as *const () as u64);
    pic::remap();

    crate::kprintln!("ps2: keyboard and mouse ready");
}
