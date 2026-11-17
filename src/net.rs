//! Network driver and stack. The virtio-net driver moves frames; the rest of
//! this file is the stack built on them: ARP, IPv4, ICMP ping, UDP, DHCP,
//! DNS, a single TCP connection, and a small HTTP/1.0 client. Protocol
//! encoding lives in netproto.rs, where the tests run on the build machine.
//!
//! Everything is polled, not interrupt driven. Blocking calls (DHCP, DNS,
//! fetch) spin on `wait_until`, which keeps reading the receive ring while
//! they wait. They run from the event loop, never from an interrupt.

use crate::fs;
use crate::netproto as np;
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
const ICMP_ID: u16 = 0x4141;
const BCAST: [u8; 6] = [0xff; 6];
const BCAST_IP: [u8; 4] = [255; 4];
const ZERO_IP: [u8; 4] = [0; 4];

/// Our UDP source port for DNS queries.
const DNS_SPORT: u16 = 40053;
const TCP_WINDOW: u16 = 16384;
/// Largest piece of data sent in one TCP segment.
const TCP_CHUNK: usize = 1200;

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

/// Addresses in use. Starts with the QEMU user-network defaults, so the
/// stack works before DHCP answers, and DHCP replaces them.
#[derive(Clone, Copy)]
struct Config {
    ip: [u8; 4],
    mask: [u8; 4],
    gateway: [u8; 4],
    dns: [u8; 4],
    from_dhcp: bool,
}

static mut CFG: Config = Config {
    ip: [10, 0, 2, 15],
    mask: [255, 255, 255, 0],
    gateway: [10, 0, 2, 2],
    dns: [10, 0, 2, 3],
    from_dhcp: false,
};

fn cfg() -> &'static mut Config {
    unsafe { &mut *addr_of_mut!(CFG) }
}

/// Set by init. The first poll after boot runs DHCP, because interrupts are
/// still off during init and the wait would never end.
static mut DHCP_PENDING: bool = false;
static mut DHCP_XID: u32 = 0;
static mut DHCP_INBOX: Option<np::DhcpReply> = None;

static mut DNS_WANT: Option<u16> = None;
static mut DNS_ANSWER: Option<[u8; 4]> = None;

#[derive(Clone, Copy, PartialEq)]
enum TcpState {
    SynSent,
    Established,
    Reset,
}

struct Conn {
    st: TcpState,
    lport: u16,
    rip: [u8; 4],
    rport: u16,
    snd_una: u32,
    snd_nxt: u32,
    rcv_nxt: u32,
    rx: Vec<u8>,
    fin: bool,
}

static mut CONN: Option<Conn> = None;
static mut SEED: u64 = 0;

fn conn() -> Option<&'static mut Conn> {
    unsafe { (*addr_of_mut!(CONN)).as_mut() }
}

fn conn_state() -> TcpState {
    conn().map_or(TcpState::Reset, |c| c.st)
}

fn random() -> u32 {
    unsafe {
        SEED ^= pit::ticks().wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        SEED ^= SEED << 13;
        SEED ^= SEED >> 7;
        SEED ^= SEED << 17;
        (SEED >> 16) as u32
    }
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
    kprintln!("net: mac {}, address via DHCP after boot", fmt_mac(mac));
    unsafe { DHCP_PENDING = true };
    true
}

// ---- Frames --------------------------------------------------------------

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

const fn used_off(n: usize) -> usize {
    (16 * n + 4 + 2 * n + 4095) & !4095
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

/// Read every frame that has arrived and handle it.
fn poll_rx() {
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
}

/// Called from the event loop on every timer tick.
pub fn poll() {
    if !nic().up {
        return;
    }
    if unsafe { DHCP_PENDING } {
        unsafe { DHCP_PENDING = false };
        startup_dhcp();
    }
    poll_rx();

    let now = pit::ticks();
    if let Some(p) = nic().ping {
        if now.wrapping_sub(p.sent) > PING_TIMEOUT {
            terminal::print(&format!("ping {}: no reply", fmt_ip(p.target)));
            nic().ping = None;
        }
    }
}

/// Spin, reading frames, until `done` is true or `ms` milliseconds pass.
fn wait_until(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = pit::ticks() + ms * pit::HZ / 1000;
    loop {
        poll_rx();
        if done() {
            return true;
        }
        if pit::ticks() >= deadline {
            return false;
        }
        core::hint::spin_loop();
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

// ---- ARP -----------------------------------------------------------------

fn send_arp(op: u16, dst: [u8; 6], tha: [u8; 6], tpa: [u8; 4]) {
    let n = nic();
    let mut p = [0u8; 28];
    p[0..2].copy_from_slice(&1u16.to_be_bytes());
    p[2..4].copy_from_slice(&0x0800u16.to_be_bytes());
    p[4] = 6;
    p[5] = 4;
    p[6..8].copy_from_slice(&op.to_be_bytes());
    p[8..14].copy_from_slice(&n.mac);
    p[14..18].copy_from_slice(&cfg().ip);
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

/// The machine a packet for `ip` goes to: the address itself on our subnet,
/// otherwise the gateway.
fn next_hop(ip: [u8; 4]) -> [u8; 4] {
    let c = cfg();
    let same_subnet = (0..4).all(|i| ip[i] & c.mask[i] == c.ip[i] & c.mask[i]);
    if same_subnet {
        ip
    } else {
        c.gateway
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

    if op == 1 && p[24..28] == cfg().ip {
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

/// Ask for `hop`'s address and wait for the reply. Only for command
/// context, never from frame handling.
fn resolve(hop: [u8; 4]) -> Option<[u8; 6]> {
    if let Some(mac) = lookup(hop) {
        return Some(mac);
    }
    for _ in 0..3 {
        send_arp(1, BCAST, [0; 6], hop);
        if wait_until(500, || lookup(hop).is_some()) {
            return lookup(hop);
        }
    }
    None
}

// ---- IPv4 ----------------------------------------------------------------

/// Send an IP packet if the next hop's address is already known. Used from
/// frame handling, which must not wait. Asks for the address if it is not known.
fn ip_send_now(dst: [u8; 4], proto: u8, payload: &[u8]) -> bool {
    let hop = next_hop(dst);
    let mac = match lookup(hop) {
        Some(m) => m,
        None => {
            send_arp(1, BCAST, [0; 6], hop);
            return false;
        }
    };
    let packet = np::build_ipv4(cfg().ip, dst, proto, payload);
    send(&eth(mac, 0x0800, &packet))
}

/// Send an IP packet, waiting for the next hop's address if needed.
fn ip_send(dst: [u8; 4], proto: u8, payload: &[u8]) -> Result<(), &'static str> {
    let mac = resolve(next_hop(dst)).ok_or("no answer from the gateway")?;
    let packet = np::build_ipv4(cfg().ip, dst, proto, payload);
    if send(&eth(mac, 0x0800, &packet)) {
        Ok(())
    } else {
        Err("the network card did not take the packet")
    }
}

/// Broadcast a UDP datagram from 0.0.0.0, which DHCP needs before it has an address.
fn broadcast_udp(sport: u16, dport: u16, data: &[u8]) -> bool {
    let seg = np::build_udp(ZERO_IP, BCAST_IP, sport, dport, data);
    let packet = np::build_ipv4(ZERO_IP, BCAST_IP, np::PROTO_UDP, &seg);
    send(&eth(BCAST, 0x0800, &packet))
}

fn ipv4_rx(src_mac: [u8; 6], p: &[u8]) {
    let ip = match np::parse_ipv4(p) {
        Some(ip) => ip,
        None => return,
    };
    if ip.src != ZERO_IP {
        learn(ip.src, src_mac);
    }
    let me = cfg().ip;
    match ip.proto {
        1 if ip.dst == me => icmp_rx(ip.src, src_mac, ip.payload),
        np::PROTO_UDP => {
            if let Some(u) = np::parse_udp(ip.src, ip.dst, ip.payload) {
                match u.dport {
                    68 => dhcp_rx(u.data),
                    DNS_SPORT => dns_rx(u.data),
                    _ => {}
                }
            }
        }
        np::PROTO_TCP if ip.dst == me => tcp_rx(ip.src, ip.payload),
        _ => {}
    }
}

fn icmp_rx(src: [u8; 4], src_mac: [u8; 6], icmp: &[u8]) {
    if icmp.len() < 8 {
        return;
    }
    match icmp[0] {
        8 => {
            let mut r = icmp.to_vec();
            r[0] = 0;
            r[2] = 0;
            r[3] = 0;
            let c = np::checksum(&r);
            r[2..4].copy_from_slice(&c.to_be_bytes());
            let packet = np::build_ipv4(cfg().ip, src, 1, &r);
            send(&eth(src_mac, 0x0800, &packet));
        }
        0 => echo_reply(src, icmp),
        _ => {}
    }
}

// ---- Ping ----------------------------------------------------------------

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
    let c = np::checksum(&m);
    m[2..4].copy_from_slice(&c.to_be_bytes());
    send(&eth(mac, 0x0800, &np::build_ipv4(cfg().ip, target, 1, &m)));
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

// ---- UDP and DHCP --------------------------------------------------------

fn dhcp_rx(data: &[u8]) {
    let r = np::parse_dhcp(data, unsafe { DHCP_XID }, nic().mac);
    if let Some(r) = r {
        unsafe { DHCP_INBOX = Some(r) };
    }
}

fn inbox_has(kind: u8) -> bool {
    unsafe { (*addr_of!(DHCP_INBOX)).as_ref().map_or(false, |r| r.msg == kind) }
}

fn take_inbox() -> Option<np::DhcpReply> {
    unsafe { (*addr_of_mut!(DHCP_INBOX)).take() }
}

/// Run the DHCP exchange and adopt the address it gives us.
fn dhcp() -> Result<(), &'static str> {
    let mac = nic().mac;
    let xid = random();
    unsafe {
        DHCP_XID = xid;
        DHCP_INBOX = None;
    }

    let mut offer = None;
    for _ in 0..3 {
        broadcast_udp(68, 67, &np::build_dhcp(mac, xid, np::DHCP_DISCOVER, None, None));
        if wait_until(1000, || inbox_has(np::DHCP_OFFER)) {
            offer = take_inbox();
            break;
        }
    }
    let offer = offer.ok_or("no DHCP offer arrived")?;

    let mut ack = None;
    for _ in 0..3 {
        let request = np::build_dhcp(mac, xid, np::DHCP_REQUEST, Some(offer.yiaddr), Some(offer.server));
        broadcast_udp(68, 67, &request);
        if wait_until(1000, || inbox_has(np::DHCP_ACK)) {
            ack = take_inbox();
            break;
        }
    }
    let ack = ack.ok_or("the DHCP server did not confirm the address")?;

    let c = cfg();
    c.ip = ack.yiaddr;
    c.mask = ack.mask;
    c.gateway = if ack.router != ZERO_IP { ack.router } else { ack.server };
    c.dns = if ack.dns != ZERO_IP { ack.dns } else { c.gateway };
    c.from_dhcp = true;
    send_arp(1, BCAST, [0; 6], c.gateway);
    Ok(())
}

fn startup_dhcp() {
    match dhcp() {
        Ok(()) => {
            let c = *cfg();
            terminal::print(&format!("network: address {} via DHCP", fmt_ip(c.ip)));
        }
        Err(e) => {
            let c = *cfg();
            terminal::print(&format!("network: {} (using {})", e, fmt_ip(c.ip)));
        }
    }
}

// ---- DNS -----------------------------------------------------------------

fn dns_rx(data: &[u8]) {
    let want = unsafe { DNS_WANT };
    if let Some(id) = want {
        if let Some(ip) = np::parse_dns_a(data, id) {
            unsafe {
                DNS_ANSWER = Some(ip);
                DNS_WANT = None;
            }
        }
    }
}

/// Look up an IPv4 address for `name`. A literal address is returned as is.
fn resolve_name(name: &str) -> Result<[u8; 4], &'static str> {
    if let Some(ip) = parse_ip(name) {
        return Ok(ip);
    }
    let dns = cfg().dns;
    for _ in 0..2 {
        let id = (random() & 0xFFFF) as u16;
        unsafe {
            DNS_WANT = Some(id);
            DNS_ANSWER = None;
        }
        let query = np::build_dns_query(id, name);
        let seg = np::build_udp(cfg().ip, dns, DNS_SPORT, 53, &query);
        ip_send(dns, np::PROTO_UDP, &seg)?;
        if wait_until(2000, || unsafe { (*addr_of!(DNS_ANSWER)).is_some() }) {
            let answer = unsafe { (*addr_of_mut!(DNS_ANSWER)).take() };
            unsafe { DNS_WANT = None };
            return answer.ok_or("no address in the answer");
        }
    }
    unsafe { DNS_WANT = None };
    Err("the name did not resolve")
}

// ---- TCP -----------------------------------------------------------------

fn seq_lt(a: u32, b: u32) -> bool {
    (b.wrapping_sub(a) as i32) > 0
}

fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

fn tcp_rx(src: [u8; 4], seg: &[u8]) {
    let t = match np::parse_tcp(src, cfg().ip, seg) {
        Some(t) => t,
        None => return,
    };
    let c = match conn() {
        Some(c) => c,
        None => return,
    };
    if t.dport != c.lport || t.sport != c.rport || src != c.rip {
        return;
    }
    if t.flags & np::TCP_RST != 0 {
        c.st = TcpState::Reset;
        return;
    }
    match c.st {
        TcpState::SynSent => {
            let syn_ack = np::TCP_SYN | np::TCP_ACK;
            if t.flags & syn_ack == syn_ack && t.ack == c.snd_nxt {
                c.rcv_nxt = t.seq.wrapping_add(1);
                c.snd_una = t.ack;
                c.st = TcpState::Established;
                send_ack(c);
            }
        }
        TcpState::Established => {
            if t.flags & np::TCP_ACK != 0 && seq_lt(c.snd_una, t.ack) && seq_le(t.ack, c.snd_nxt) {
                c.snd_una = t.ack;
            }
            let mut ack_needed = false;
            if !t.data.is_empty() {
                ack_needed = true;
                if t.seq == c.rcv_nxt {
                    c.rx.extend_from_slice(t.data);
                    c.rcv_nxt = c.rcv_nxt.wrapping_add(t.data.len() as u32);
                }
            }
            if t.flags & np::TCP_FIN != 0 {
                ack_needed = true;
                if t.seq.wrapping_add(t.data.len() as u32) == c.rcv_nxt {
                    c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
                    c.fin = true;
                }
            }
            if ack_needed {
                send_ack(c);
            }
        }
        TcpState::Reset => {}
    }
}

fn send_ack(c: &Conn) {
    let seg = np::build_tcp(
        cfg().ip,
        c.rip,
        c.lport,
        c.rport,
        c.snd_nxt,
        c.rcv_nxt,
        np::TCP_ACK,
        None,
        TCP_WINDOW,
        &[],
    );
    ip_send_now(c.rip, np::PROTO_TCP, &seg);
}

fn tcp_connect(ip: [u8; 4], port: u16) -> Result<(), &'static str> {
    let isn = random();
    let lport = 49152 + (random() % 16000) as u16;
    unsafe {
        CONN = Some(Conn {
            st: TcpState::SynSent,
            lport,
            rip: ip,
            rport: port,
            snd_una: isn,
            snd_nxt: isn.wrapping_add(1),
            rcv_nxt: 0,
            rx: Vec::new(),
            fin: false,
        });
    }
    let result: Result<(), &'static str> = (|| {
        for _ in 0..3 {
            let syn = np::build_tcp(cfg().ip, ip, lport, port, isn, 0, np::TCP_SYN, Some(1460), TCP_WINDOW, &[]);
            ip_send(ip, np::PROTO_TCP, &syn)?;
            if wait_until(1000, || conn_state() != TcpState::SynSent) {
                break;
            }
        }
        match conn_state() {
            TcpState::Established => Ok(()),
            TcpState::Reset => Err("the connection was refused"),
            TcpState::SynSent => Err("the connection timed out"),
        }
    })();
    if result.is_err() {
        unsafe { CONN = None };
    }
    result
}

/// Send data reliably, one chunk at a time, resending until it is acknowledged.
fn tcp_send(data: &[u8]) -> Result<(), &'static str> {
    for chunk in data.chunks(TCP_CHUNK) {
        let seq = conn().ok_or("no connection")?.snd_nxt;
        let end = seq.wrapping_add(chunk.len() as u32);
        conn().ok_or("no connection")?.snd_nxt = end;
        let mut acked = false;
        for _ in 0..5 {
            let c = conn().ok_or("no connection")?;
            let seg = np::build_tcp(
                cfg().ip,
                c.rip,
                c.lport,
                c.rport,
                seq,
                c.rcv_nxt,
                np::TCP_ACK | np::TCP_PSH,
                None,
                TCP_WINDOW,
                chunk,
            );
            let rip = c.rip;
            ip_send(rip, np::PROTO_TCP, &seg)?;
            if wait_until(1000, || conn().map_or(true, |c| seq_le(end, c.snd_una) || c.st == TcpState::Reset)) {
                acked = true;
                break;
            }
        }
        if !acked || conn_state() == TcpState::Reset {
            return Err("the server stopped answering");
        }
    }
    Ok(())
}

/// Wait for the server to close its side, then return everything it sent.
fn tcp_wait_close(timeout_ms: u64) -> Result<Vec<u8>, &'static str> {
    let ok = wait_until(timeout_ms, || conn().map_or(true, |c| c.fin || c.st == TcpState::Reset));
    let c = conn().ok_or("no connection")?;
    if c.fin {
        Ok(core::mem::take(&mut c.rx))
    } else if ok {
        Err("the connection was reset")
    } else {
        Err("timed out waiting for the reply")
    }
}

fn tcp_close() {
    if let Some(c) = conn() {
        let seg = np::build_tcp(
            cfg().ip,
            c.rip,
            c.lport,
            c.rport,
            c.snd_nxt,
            c.rcv_nxt,
            np::TCP_FIN | np::TCP_ACK,
            None,
            TCP_WINDOW,
            &[],
        );
        let rip = c.rip;
        ip_send_now(rip, np::PROTO_TCP, &seg);
    }
    unsafe { CONN = None };
}

// ---- Commands ------------------------------------------------------------

/// Download a plain HTTP URL into a file on the drive.
fn fetch_to(url: &str, file: &str) -> Result<String, &'static str> {
    if !file.starts_with('/') {
        return Err("give the drive path in full, like /Documents/file.txt");
    }
    if !nic().up {
        return Err("no network device");
    }
    if !fs::mounted() {
        return Err("unlock the drive first");
    }
    let (host, port, path) = np::parse_url(url)?;
    let ip = resolve_name(&host)?;
    tcp_connect(ip, port)?;
    let raw = tcp_send(&np::build_http_get(&host, &path))
        .and_then(|()| tcp_wait_close(30_000));
    tcp_close();
    let raw = raw?;
    let response = np::parse_response(&raw).ok_or("the reply was not a complete HTTP response")?;
    if response.status != 200 {
        return Ok(format!("the server answered HTTP {}, nothing saved", response.status));
    }
    fs::write(file, &response.body).map_err(|_| "could not write the file to the drive")?;
    Ok(format!("saved {} bytes to {}", response.body.len(), file))
}

fn fetch(rest: &str) {
    let mut parts = rest.split_whitespace();
    let (url, file) = match (parts.next(), parts.next()) {
        (Some(url), Some(file)) => (url, file),
        _ => {
            terminal::print("usage: fetch http://host/path /Documents/file");
            return;
        }
    };
    terminal::print(&format!("fetching {} ...", url));
    match fetch_to(url, file) {
        Ok(msg) => terminal::print(&msg),
        Err(e) => terminal::print(&format!("fetch failed: {}", e)),
    }
}

fn status() {
    let n = nic();
    terminal::print(&format!("link:    {}", if n.up { "up" } else { "down" }));
    if !n.up {
        return;
    }
    let c = *cfg();
    terminal::print(&format!("mac:     {}", fmt_mac(n.mac)));
    terminal::print(&format!("ip:      {} ({})", fmt_ip(c.ip), if c.from_dhcp { "DHCP" } else { "default" }));
    terminal::print(&format!("mask:    {}", fmt_ip(c.mask)));
    terminal::print(&format!("gateway: {}", fmt_ip(c.gateway)));
    terminal::print(&format!("dns:     {}", fmt_ip(c.dns)));
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
            send_arp(1, BCAST, [0; 6], cfg().gateway);
            terminal::print("arp: request sent for gateway");
            true
        }
        "ping" => {
            ping(rest);
            true
        }
        "dhcp" => {
            if !nic().up {
                terminal::print("dhcp: no network device");
            } else {
                startup_dhcp();
            }
            true
        }
        "dns" => {
            let name = rest.trim();
            if name.is_empty() {
                terminal::print("usage: dns <name>");
            } else {
                match resolve_name(name) {
                    Ok(ip) => terminal::print(&format!("{} = {}", name, fmt_ip(ip))),
                    Err(e) => terminal::print(&format!("dns: {}", e)),
                }
            }
            true
        }
        "fetch" => {
            fetch(rest);
            true
        }
        _ => false,
    }
}

pub fn renew() {
    if nic().up {
        startup_dhcp();
    }
}

/// What the Settings app shows on its Network page.
pub struct Info {
    pub up: bool,
    pub mac: String,
    pub ip: String,
    pub gateway: String,
    pub arp_entries: usize,
}

pub fn info() -> Info {
    let n = nic();
    let c = *cfg();
    Info {
        up: n.up,
        mac: fmt_mac(n.mac),
        ip: fmt_ip(c.ip),
        gateway: fmt_ip(c.gateway),
        arp_entries: n.arp.iter().flatten().count(),
    }
}
