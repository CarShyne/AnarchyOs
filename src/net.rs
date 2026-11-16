use crate::paging;
use crate::pit;
use crate::terminal;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

const VENDOR: u16 = 0x1AF4;
const DEVICE: u16 = 0x1000;

const REG_DEV_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0C;
const REG_QUEUE_SEL: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
const REG_MAC: u16 = 0x14;

const ST_ACK: u8 = 1;
const ST_DRIVER: u8 = 2;
const ST_DRIVER_OK: u8 = 4;

const F_MAC: u32 = 1 << 5;
const DESC_WRITE: u16 = 2;

const QBYTES: usize = 12288;
const QMAX: usize = 256;
const RX_COUNT: usize = 64;
const BUF: usize = 2048;
const HDR: usize = 10;
const SPIN: u32 = 50_000_000;
const PING_TIMEOUT: u64 = 300;
const MY_IP: [u8; 4] = [10, 0, 2, 15];
const GATEWAY: [u8; 4] = [10, 0, 2, 2];
const BCAST: [u8; 6] = [0xff; 6];
const ICMP_ID: u16 = 0x4141;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(4096))]
struct Ring([u8; QBYTES]);

#[repr(C, align(2048))]
struct Buf([u8; BUF]);

static mut RX_Q: Ring = Ring([0; QBYTES]);
static mut TX_Q: Ring = Ring([0; QBYTES]);
static mut RX_MEM: [Buf; RX_COUNT] = [const { Buf([0; BUF]) }; RX_COUNT];
static mut TX_MEM: Buf = Buf([0; BUF]);

#[derive(Clone, Copy)]
struct Ping {
    target: [u8; 4],
    seq: u16,
    sent: u64,
    arp_wait: bool,
}

struct Nic {
    up: bool,
    base: u16,
    mac: [u8; 6],
    rx_size: usize,
    tx_size: usize,
    rx_avail: u16,
    rx_used: u16,
    tx_used: u16,
    tx_phys: u64,
    rx_phys: [u64; RX_COUNT],
    arp: [Option<([u8; 4], [u8; 6])>; 8],
    ping: Option<Ping>,
    seq: u16,
}

static mut NIC: Nic = Nic {
    up: false,
    base: 0,
    mac: [0; 6],
    rx_size: 0,
    tx_size: 0,
    rx_avail: 0,
    rx_used: 0,
    tx_used: 0,
    tx_phys: 0,
    rx_phys: [0; RX_COUNT],
    arp: [None; 8],
    ping: None,
    seq: 0,
};

fn nic() -> &'static mut Nic {
    unsafe { &mut *addr_of_mut!(NIC) }
}

const fn used_off(n: usize) -> usize {
    (16 * n + 4 + 2 * n + 4095) & !4095
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

fn find_nic() -> Option<u16> {
    for dev in 0..32u8 {
        let id = pci_read(0, dev, 0, 0);
        if (id & 0xFFFF) as u16 != VENDOR || (id >> 16) as u16 != DEVICE {
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

fn setup_queue(base: u16, sel: u16, q: *mut u8, root: u64) -> Option<usize> {
    unsafe {
        outw(base + REG_QUEUE_SEL, sel);
        let size = inw(base + REG_QUEUE_SIZE) as usize;
        if size == 0 || size > QMAX {
            return None;
        }
        core::ptr::write_bytes(q, 0, QBYTES);
        let phys = paging::translate(root, q as u64)?;
        outl(base + REG_QUEUE_PFN, (phys >> 12) as u32);
        Some(size)
    }
}

pub fn init(root: u64) -> bool {
    let base = match find_nic() {
        Some(b) => b,
        None => {
            kprintln!("net: no virtio-net device attached");
            return false;
        }
    };
    let rx_q = addr_of_mut!(RX_Q) as *mut u8;
    let tx_q = addr_of_mut!(TX_Q) as *mut u8;
    let mut mac = [0u8; 6];
    unsafe {
        outb(base + REG_STATUS, 0);
        outb(base + REG_STATUS, ST_ACK);
        outb(base + REG_STATUS, ST_ACK | ST_DRIVER);
        let feats = inl(base + REG_DEV_FEATURES);
        outl(base + REG_GUEST_FEATURES, feats & F_MAC);
        for (i, b) in mac.iter_mut().enumerate() {
            *b = inb(base + REG_MAC + i as u16);
        }
    }
    let rx_size = match setup_queue(base, 0, rx_q, root) {
        Some(s) if s >= RX_COUNT => s,
        _ => {
            kprintln!("net: rx queue unusable");
            return false;
        }
    };
    let tx_size = match setup_queue(base, 1, tx_q, root) {
        Some(s) => s,
        None => {
            kprintln!("net: tx queue unusable");
            return false;
        }
    };

    let n = nic();
    n.base = base;
    n.mac = mac;
    n.rx_size = rx_size;
    n.tx_size = tx_size;
    unsafe {
        let mem = addr_of_mut!(RX_MEM) as *mut Buf;
        for i in 0..RX_COUNT {
            let phys = match paging::translate(root, mem.add(i) as u64) {
                Some(p) => p,
                None => return false,
            };
            n.rx_phys[i] = phys;
            (rx_q as *mut Desc).add(i).write(Desc {
                addr: phys,
                len: BUF as u32,
                flags: DESC_WRITE,
                next: 0,
            });
        }
        let avail = rx_q.add(16 * rx_size) as *mut u16;
        for i in 0..RX_COUNT {
            write_volatile(avail.add(2 + i), i as u16);
        }
        fence(Ordering::Release);
        write_volatile(avail.add(1), RX_COUNT as u16);
        n.rx_avail = RX_COUNT as u16;
        n.tx_phys = match paging::translate(root, addr_of_mut!(TX_MEM) as u64) {
            Some(p) => p,
            None => return false,
        };
        outb(base + REG_STATUS, ST_ACK | ST_DRIVER | ST_DRIVER_OK);
        outw(base + REG_QUEUE_NOTIFY, 0);
    }
    n.up = true;
    kprintln!("net: mac {}, ip {}, gateway {}", fmt_mac(mac), fmt_ip(MY_IP), fmt_ip(GATEWAY));
    send_arp(1, BCAST, [0; 6], GATEWAY);
    true
}

fn send(frame: &[u8]) -> bool {
    let n = nic();
    if !n.up || frame.len() + HDR > BUF {
        return false;
    }
    let q = addr_of_mut!(TX_Q) as *mut u8;
    let size = n.tx_size;
    let base = n.base;
    let tx_phys = n.tx_phys;
    let target = n.tx_used;
    unsafe {
        let buf = addr_of_mut!(TX_MEM) as *mut u8;
        core::ptr::write_bytes(buf, 0, HDR);
        core::ptr::copy_nonoverlapping(frame.as_ptr(), buf.add(HDR), frame.len());
        (q as *mut Desc).write(Desc {
            addr: tx_phys,
            len: (HDR + frame.len()) as u32,
            flags: 0,
            next: 0,
        });
        let avail = q.add(16 * size) as *mut u16;
        let idx = read_volatile(avail.add(1));
        write_volatile(avail.add(2 + idx as usize % size), 0);
        fence(Ordering::Release);
        write_volatile(avail.add(1), idx.wrapping_add(1));
        fence(Ordering::Release);
        outw(base + REG_QUEUE_NOTIFY, 1);
        let used = q.add(used_off(size)) as *const u16;
        let mut spins = 0u32;
        while read_volatile(used.add(1)) == target {
            spins += 1;
            if spins > SPIN {
                return false;
            }
        }
        fence(Ordering::Acquire);
    }
    nic().tx_used = target.wrapping_add(1);
    true
}

fn repost(id: usize) {
    let n = nic();
    let size = n.rx_size;
    let q = addr_of_mut!(RX_Q) as *mut u8;
    unsafe {
        let avail = q.add(16 * size) as *mut u16;
        write_volatile(avail.add(2 + n.rx_avail as usize % size), id as u16);
        n.rx_avail = n.rx_avail.wrapping_add(1);
        fence(Ordering::Release);
        write_volatile(avail.add(1), n.rx_avail);
        fence(Ordering::Release);
        outw(n.base + REG_QUEUE_NOTIFY, 0);
    }
}

unsafe fn rx_slice(id: usize, len: usize) -> &'static [u8] {
    let base = addr_of!(RX_MEM) as *const Buf;
    let buf = &(*base.add(id)).0;
    &buf[HDR..len.min(BUF)]
}

pub fn poll() {
    if !nic().up {
        return;
    }
    let size = nic().rx_size;
    let q = addr_of_mut!(RX_Q) as *mut u8;
    let used = unsafe { q.add(used_off(size)) };
    loop {
        let idx = unsafe { read_volatile(used.add(2) as *const u16) };
        fence(Ordering::Acquire);
        if idx == nic().rx_used {
            break;
        }
        let slot = nic().rx_used as usize % size;
        let elem = unsafe { used.add(4 + 8 * slot) } as *const u32;
        let (id, len) = unsafe {
            (read_volatile(elem) as usize, read_volatile(elem.add(1)) as usize)
        };
        nic().rx_used = nic().rx_used.wrapping_add(1);
        if id < RX_COUNT && len > HDR {
            let frame = unsafe { rx_slice(id, len) };
            handle_frame(frame);
        }
        repost(id);
    }

    let now = pit::ticks();
    if let Some(p) = nic().ping {
        if now.wrapping_sub(p.sent) > PING_TIMEOUT {
            terminal::print(&format!("ping {}: no reply", fmt_ip(p.target)));
            nic().ping = None;
        }
    }
}

fn handle_frame(f: &[u8]) {
    if f.len() < 14 {
        return;
    }
    let src = [f[6], f[7], f[8], f[9], f[10], f[11]];
    match u16::from_be_bytes([f[12], f[13]]) {
        0x0806 => arp_rx(&f[14..]),
        0x0800 => ipv4_rx(src, &f[14..]),
        _ => {}
    }
}

fn eth(dst: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(60.max(14 + payload.len()));
    f.extend_from_slice(&dst);
    f.extend_from_slice(&nic().mac);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    while f.len() < 60 {
        f.push(0);
    }
    f
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn ipv4(proto: u8, dst: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let total = (20 + payload.len()) as u16;
    let mut h = [0u8; 20];
    h[0] = 0x45;
    h[2..4].copy_from_slice(&total.to_be_bytes());
    h[8] = 64;
    h[9] = proto;
    h[12..16].copy_from_slice(&MY_IP);
    h[16..20].copy_from_slice(&dst);
    let c = checksum(&h);
    h[10..12].copy_from_slice(&c.to_be_bytes());
    let mut out = Vec::with_capacity(20 + payload.len());
    out.extend_from_slice(&h);
    out.extend_from_slice(payload);
    out
}

fn send_arp(op: u16, dst: [u8; 6], tha: [u8; 6], tpa: [u8; 4]) {
    let n = nic();
    let mut p = [0u8; 28];
    p[0..2].copy_from_slice(&1u16.to_be_bytes());
    p[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    p[4] = 6;
    p[5] = 4;
    p[6..8].copy_from_slice(&op.to_be_bytes());
    p[8..14].copy_from_slice(&n.mac);
    p[14..18].copy_from_slice(&MY_IP);
    p[18..24].copy_from_slice(&tha);
    p[24..28].copy_from_slice(&tpa);
    send(&eth(dst, 0x0806, &p));
}

fn lookup(ip: [u8; 4]) -> Option<[u8; 6]> {
    nic().arp.iter().flatten().find(|e| e.0 == ip).map(|e| e.1)
}

fn learn(ip: [u8; 4], mac: [u8; 6]) {
    let n = nic();
    for slot in n.arp.iter_mut() {
        if let Some(e) = slot {
            if e.0 == ip {
                e.1 = mac;
                return;
            }
        }
    }
    for slot in n.arp.iter_mut() {
        if slot.is_none() {
            *slot = Some((ip, mac));
            return;
        }
    }
    n.arp[0] = Some((ip, mac));
}

fn next_hop(ip: [u8; 4]) -> [u8; 4] {
    if ip[0] == 10 && ip[1] == 0 && ip[2] == 2 {
        ip
    } else {
        GATEWAY
    }
}

fn arp_rx(p: &[u8]) {
    if p.len() < 28 {
        return;
    }
    let op = u16::from_be_bytes([p[6], p[7]]);
    let mut sha = [0u8; 6];
    sha.copy_from_slice(&p[8..14]);
    let mut spa = [0u8; 4];
    spa.copy_from_slice(&p[14..18]);
    learn(spa, sha);

    if op == 1 && &p[24..28] == &MY_IP[..] {
        send_arp(2, sha, sha, spa);
    }
    if op == 2 {
        let n = nic();
        if let Some(pg) = n.ping {
            if pg.arp_wait && next_hop(pg.target) == spa {
                n.ping = Some(Ping { arp_wait: false, ..pg });
                send_echo(pg.target, pg.seq);
            }
        }
    }
}

fn ipv4_rx(src_mac: [u8; 6], p: &[u8]) {
    if p.len() < 20 {
        return;
    }
    let ihl = ((p[0] & 0x0F) as usize) * 4;
    if ihl < 20 || p.len() < ihl {
        return;
    }
    if &p[16..20] != &MY_IP[..] {
        return;
    }
    let total = (u16::from_be_bytes([p[2], p[3]]) as usize).min(p.len());
    if total < ihl + 8 || p[9] != 1 {
        return;
    }
    let src = [p[12], p[13], p[14], p[15]];
    learn(src, src_mac);
    let icmp = &p[ihl..total];
    match icmp[0] {
        8 => {
            let mut r = icmp.to_vec();
            r[0] = 0;
            r[2] = 0;
            r[3] = 0;
            let c = checksum(&r);
            r[2..4].copy_from_slice(&c.to_be_bytes());
            let ip = ipv4(1, src, &r);
            send(&eth(src_mac, 0x0800, &ip));
        }
        0 => echo_reply(src, icmp),
        _ => {}
    }
}

fn echo_reply(src: [u8; 4], icmp: &[u8]) {
    let seq = u16::from_be_bytes([icmp[6], icmp[7]]);
    let n = nic();
    let pg = match n.ping {
        Some(p) => p,
        None => return,
    };
    if pg.arp_wait || pg.target != src || pg.seq != seq {
        return;
    }
    let ms = pit::ticks().wrapping_sub(pg.sent) * 1000 / pit::HZ;
    n.ping = None;
    terminal::print(&format!("reply from {}: seq={} time={} ms", fmt_ip(src), seq, ms));
}

fn send_echo(target: [u8; 4], seq: u16) {
    let mac = match lookup(next_hop(target)) {
        Some(m) => m,
        None => return,
    };
    let mut m = Vec::with_capacity(22);
    m.push(8);
    m.push(0);
    m.extend_from_slice(&[0, 0]);
    m.extend_from_slice(&ICMP_ID.to_be_bytes());
    m.extend_from_slice(&seq.to_be_bytes());
    m.extend_from_slice(b"anarchyos");
    let c = checksum(&m);
    m[2..4].copy_from_slice(&c.to_be_bytes());
    send(&eth(mac, 0x0800, &ipv4(1, target, &m)));
}

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut it = s.split('.');
    for slot in out.iter_mut() {
        *slot = it.next()?.parse().ok()?;
    }
    if it.next().is_some() {
        return None;
    }
    Some(out)
}

fn fmt_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

fn fmt_mac(m: [u8; 6]) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}

fn ping(arg: &str) {
    let target = match parse_ip(arg) {
        Some(t) => t,
        None => {
            terminal::print("usage: ping a.b.c.d");
            return;
        }
    };
    if !nic().up {
        terminal::print("ping: no network device");
        return;
    }
    let hop = next_hop(target);
    let wait = lookup(hop).is_none();
    let n = nic();
    n.seq = n.seq.wrapping_add(1);
    let seq = n.seq;
    n.ping = Some(Ping { target, seq, sent: pit::ticks(), arp_wait: wait });
    if wait {
        send_arp(1, BCAST, [0; 6], hop);
        terminal::print(&format!("ping {}: resolving {}", fmt_ip(target), fmt_ip(hop)));
    } else {
        terminal::print(&format!("ping {} seq={}", fmt_ip(target), seq));
        send_echo(target, seq);
    }
}

fn status() {
    let n = nic();
    terminal::print(&format!("link:    {}", if n.up { "up" } else { "down" }));
    if !n.up {
        return;
    }
    terminal::print(&format!("mac:     {}", fmt_mac(n.mac)));
    terminal::print(&format!("ip:      {}", fmt_ip(MY_IP)));
    terminal::print(&format!("gateway: {}", fmt_ip(GATEWAY)));
    let cached = n.arp.iter().flatten().count();
    terminal::print(&format!("arp:     {} entries", cached));
}

pub fn command(name: &str, rest: &str) -> bool {
    match name {
        "net" => {
            status();
            true
        }
        "arp" => {
            send_arp(1, BCAST, [0; 6], GATEWAY);
            terminal::print("arp: request sent for gateway");
            true
        }
        "ping" => {
            ping(rest);
            true
        }
        _ => false,
    }
}
