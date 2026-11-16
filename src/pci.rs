use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut};

#[derive(Clone, Copy)]
struct Dev {
    vendor: u16,
    device: u16,
    base: u8,
    sub: u8,
}

static mut DEVS: Vec<Dev> = Vec::new();

fn read(slot: u8, off: u8) -> u32 {
    let addr = 0x8000_0000u32 | (slot as u32) << 11 | (off as u32 & 0xFC);
    unsafe {
        asm!("out dx, eax", in("dx") 0xCF8u16, in("eax") addr,
             options(nomem, nostack, preserves_flags));
        let v: u32;
        asm!("in eax, dx", out("eax") v, in("dx") 0xCFCu16,
             options(nomem, nostack, preserves_flags));
        v
    }
}

pub fn init() {
    let devs = unsafe { &mut *addr_of_mut!(DEVS) };
    devs.clear();
    for slot in 0..32u8 {
        let id = read(slot, 0x00);
        let vendor = (id & 0xFFFF) as u16;
        if vendor == 0xFFFF {
            continue;
        }
        let class = read(slot, 0x08);
        devs.push(Dev {
            vendor,
            device: (id >> 16) as u16,
            base: (class >> 24) as u8,
            sub: (class >> 16) as u8,
        });
    }
    kprintln!("pci: {} devices on bus 0", devs.len());
}

fn is_wireless(d: &Dev) -> bool {
    d.base == 0x0D || (d.base == 0x02 && d.sub == 0x80)
}

/// Wireless controllers found on the bus, as "vendor:device" ids.
pub fn wireless_found() -> Vec<String> {
    let devs = unsafe { &*addr_of!(DEVS) };
    devs.iter()
        .filter(|d| is_wireless(d))
        .map(|d| format!("{:04x}:{:04x}", d.vendor, d.device))
        .collect()
}
