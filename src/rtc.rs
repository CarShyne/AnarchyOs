//! The CMOS real-time clock: the battery-backed clock every PC has. It
//! gives us wall-clock time without a network.

use crate::clock::DateTime;
use core::arch::asm;

const ADDRESS: u16 = 0x70;
const DATA: u16 = 0x71;

fn cmos(reg: u8) -> u8 {
    let value: u8;
    unsafe {
        asm!("out dx, al", in("dx") ADDRESS, in("al") reg,
             options(nomem, nostack, preserves_flags));
        asm!("in al, dx", out("al") value, in("dx") DATA,
             options(nomem, nostack, preserves_flags));
    }
    value
}

fn raw() -> [u8; 6] {
    // Wait out an update in progress so we never read half a rollover.
    let mut spins = 0;
    while cmos(0x0A) & 0x80 != 0 && spins < 100_000 {
        spins += 1;
    }
    [cmos(0x00), cmos(0x02), cmos(0x04), cmos(0x07), cmos(0x08), cmos(0x09)]
}

fn bcd(v: u8) -> u32 {
    ((v >> 4) * 10 + (v & 0x0F)) as u32
}

/// Read the current date and time from the hardware clock.
pub fn read() -> DateTime {
    // Read until two passes agree, which rules out a rollover mid-read.
    let mut a = raw();
    for _ in 0..4 {
        let b = raw();
        if a == b {
            break;
        }
        a = b;
    }

    let status_b = cmos(0x0B);
    let binary = status_b & 0x04 != 0;
    let is_24h = status_b & 0x02 != 0;
    let conv = |v: u8| if binary { v as u32 } else { bcd(v) };

    let pm = a[2] & 0x80 != 0;
    let mut hour = conv(a[2] & 0x7F);
    if !is_24h {
        hour %= 12;
        if pm {
            hour += 12;
        }
    }

    DateTime {
        year: 2000 + conv(a[5]) as i32,
        month: conv(a[4]).clamp(1, 12),
        day: conv(a[3]).clamp(1, 31),
        hour: hour.min(23),
        minute: conv(a[1]).min(59),
        second: conv(a[0]).min(59),
    }
}
