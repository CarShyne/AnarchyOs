//! COM1 serial output. This is our only debugging channel until the
//! framebuffer exists, so it has to be reliable.

use core::arch::asm;
use core::fmt::{self, Write};

const COM1: u16 = 0x3F8;

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

/// Set up COM1 at 115200 baud, 8N1, with FIFOs enabled.
pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // disable interrupts
        outb(COM1 + 3, 0x80); // enable DLAB
        outb(COM1 + 0, 0x01); // divisor low  (115200 baud)
        outb(COM1 + 1, 0x00); // divisor high
        outb(COM1 + 3, 0x03); // 8 bits, no parity, one stop bit
        outb(COM1 + 2, 0xC7); // enable and clear FIFOs
        outb(COM1 + 4, 0x0B); // RTS/DSR set
    }
}

struct Serial;

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            unsafe {
                // Wait until the transmit holding register is empty.
                while inb(COM1 + 5) & 0x20 == 0 {}
                outb(COM1, byte);
            }
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = Serial.write_fmt(args);
}

#[macro_export]
macro_rules! kprintln {
    () => ($crate::serial::_print(format_args!("\n")));
    ($($arg:tt)*) => ($crate::serial::_print(format_args!("{}\n", format_args!($($arg)*))));
}

