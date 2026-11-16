use crate::paging;
use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

const VIRTIO_VENDOR: u16 = 0x1AF4;
const VIRTIO_BLK_LEGACY: u16 = 0x1001;

const REG_DEV_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0C;
const REG_QUEUE_SEL: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
const REG_CAPACITY: u16 = 0x14;

const ST_ACK: u8 = 1;
const ST_DRIVER: u8 = 2;
const ST_DRIVER_OK: u8 = 4;

const F_NEXT: u16 = 1;
const F_WRITE: u16 = 2;
const T_IN: u32 = 0;
const T_OUT: u32 = 1;

const MAX_Q: usize = 256;
const VQ_BYTES: usize = 12288;
const SPIN_LIMIT: u32 = 50_000_000;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(4096))]
struct Queue([u8; VQ_BYTES]);

#[repr(C, align(4096))]
struct Io {
    kind: u32,
    reserved: u32,
    sector: u64,
    data: [u8; 512],
    status: u8,
}

static mut QUEUE: Queue = Queue([0; VQ_BYTES]);
static mut IO: Io = Io { kind: 0, reserved: 0, sector: 0, data: [0; 512], status: 0 };

struct Driver {
    base: u16,
    qsize: usize,
    last_used: u16,
    io_phys: u64,
    sectors: u64,
    ready: bool,
}

static mut DRV: Driver = Driver {
    base: 0,
    qsize: 0,
    last_used: 0,
    io_phys: 0,
    sectors: 0,
    ready: false,
};

fn drv() -> &'static mut Driver {
    unsafe { &mut *addr_of_mut!(DRV) }
}

unsafe fn outb(p: u16, v: u8) {
    asm!("out dx, al", in("dx") p, in("al") v, options(nomem, nostack, preserves_flags));
}
unsafe fn outw(p: u16, v: u16) {
    asm!("out dx, ax", in("dx") p, in("ax") v, options(nomem, nostack, preserves_flags));
}
unsafe fn outl(p: u16, v: u32) {
    asm!("out dx, eax", in("dx") p, in("eax") v, options(nomem, nostack, preserves_flags));
}
unsafe fn inb(p: u16) -> u8 {
    let v: u8;
    asm!("in al, dx", out("al") v, in("dx") p, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn inw(p: u16) -> u16 {
    let v: u16;
    asm!("in ax, dx", out("ax") v, in("dx") p, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn inl(p: u16) -> u32 {
    let v: u32;
    asm!("in eax, dx", out("eax") v, in("dx") p, options(nomem, nostack, preserves_flags));
    v
}

fn pci_read(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    let addr = 0x8000_0000u32
        | (bus as u32) << 16
        | (dev as u32) << 11
        | (func as u32) << 8
        | (off as u32 & 0xFC);
    unsafe {
        outl(0xCF8, addr);
        inl(0xCFC)
    }
}

fn pci_write(bus: u8, dev: u8, func: u8, off: u8, val: u32) {
    let addr = 0x8000_0000u32
        | (bus as u32) << 16
        | (dev as u32) << 11
        | (func as u32) << 8
        | (off as u32 & 0xFC);
    unsafe {
        outl(0xCF8, addr);
        outl(0xCFC, val);
    }
}

fn find_device() -> Option<u16> {
    for dev in 0..32u8 {
        let id = pci_read(0, dev, 0, 0);
        if (id & 0xFFFF) as u16 != VIRTIO_VENDOR || (id >> 16) as u16 != VIRTIO_BLK_LEGACY {
            continue;
        }
        let bar = pci_read(0, dev, 0, 0x10);
        if bar & 1 == 0 {
            return None;
        }
        let cmd = pci_read(0, dev, 0, 0x04);
        pci_write(0, dev, 0, 0x04, (cmd & 0xFFFF_0000) | ((cmd & 0xFFFF) | 0x5));
        return Some((bar & 0xFFFC) as u16);
    }
    None
}

pub fn init(root: u64) -> bool {
    let base = match find_device() {
        Some(b) => b,
        None => return false,
    };
    let d = drv();
    d.base = base;

    unsafe {
        outb(base + REG_STATUS, 0);
        outb(base + REG_STATUS, ST_ACK);
        outb(base + REG_STATUS, ST_ACK | ST_DRIVER);
        let _features = inl(base + REG_DEV_FEATURES);
        outl(base + REG_GUEST_FEATURES, 0);

        outw(base + REG_QUEUE_SEL, 0);
        let n = inw(base + REG_QUEUE_SIZE) as usize;
        if n == 0 || n > MAX_Q {
            kprintln!("virtio-blk: unusable queue size {}", n);
            return false;
        }

        let q = addr_of_mut!(QUEUE) as *mut u8;
        core::ptr::write_bytes(q, 0, VQ_BYTES);
        let qphys = match paging::translate(root, q as u64) {
            Some(p) => p,
            None => return false,
        };
        let iphys = match paging::translate(root, addr_of!(IO) as u64) {
            Some(p) => p,
            None => return false,
        };

        d.qsize = n;
        d.io_phys = iphys;
        outl(base + REG_QUEUE_PFN, (qphys >> 12) as u32);
        outb(base + REG_STATUS, ST_ACK | ST_DRIVER | ST_DRIVER_OK);

        let lo = inl(base + REG_CAPACITY) as u64;
        let hi = inl(base + REG_CAPACITY + 4) as u64;
        d.sectors = (hi << 32) | lo;
    }

    d.ready = true;
    kprintln!("virtio-blk: {} sectors ({} KiB)", d.sectors, d.sectors / 2);
    true
}

fn request(kind: u32, lba: u64, buf: &mut [u8; 512]) -> bool {
    let d = drv();
    if !d.ready || lba >= d.sectors {
        return false;
    }
    let io = addr_of_mut!(IO);
    let q = addr_of_mut!(QUEUE) as *mut u8;
    let n = d.qsize;
    let io_p = d.io_phys;

    unsafe {
        (*io).kind = kind;
        (*io).reserved = 0;
        (*io).sector = lba;
        (*io).status = 0xFF;
        if kind == T_OUT {
            (*io).data.copy_from_slice(&buf[..]);
        }

        let desc = q as *mut Desc;
        desc.add(0).write(Desc { addr: io_p, len: 16, flags: F_NEXT, next: 1 });
        let data_flags = if kind == T_IN { F_NEXT | F_WRITE } else { F_NEXT };
        desc.add(1).write(Desc { addr: io_p + 16, len: 512, flags: data_flags, next: 2 });
        desc.add(2).write(Desc { addr: io_p + 528, len: 1, flags: F_WRITE, next: 0 });

        let avail = q.add(n * 16) as *mut u16;
        let idx = read_volatile(avail.add(1));
        write_volatile(avail.add(2 + (idx as usize % n)), 0);
        fence(Ordering::Release);
        write_volatile(avail.add(1), idx.wrapping_add(1));
        fence(Ordering::Release);

        outw(d.base + REG_QUEUE_NOTIFY, 0);

        let used_off = (n * 16 + 4 + 2 * n + 4095) & !4095;
        let used = q.add(used_off) as *mut u16;
        let target = d.last_used;
        let mut spins: u32 = 0;
        while read_volatile(used.add(1)) == target {
            spins += 1;
            if spins > SPIN_LIMIT {
                return false;
            }
        }
        fence(Ordering::Acquire);
        d.last_used = target.wrapping_add(1);

        let ok = read_volatile(addr_of!((*io).status)) == 0;
        if kind == T_IN && ok {
            core::ptr::copy_nonoverlapping(addr_of!((*io).data) as *const u8, buf.as_mut_ptr(), 512);
        }
        ok
    }
}

pub fn read(lba: u64, buf: &mut [u8; 512]) -> bool {
    request(T_IN, lba, buf)
}

pub fn write(lba: u64, buf: &[u8; 512]) -> bool {
    let mut tmp = *buf;
    request(T_OUT, lba, &mut tmp)
}

pub fn sectors() -> u64 {
    drv().sectors
}

pub fn ready() -> bool {
    drv().ready
}
