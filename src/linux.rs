//! Linux x86_64 system calls. Numbers, argument order and return values
//! follow Linux, so static musl binaries can call straight into them. Errors
//! come back as negative errno values in rax. Calls we do not implement
//! return -ENOSYS, which musl handles gracefully.

use crate::arch::x86_64::syscall;
use crate::fs;
use crate::pit;
use crate::terminal;
use crate::userspace;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::addr_of_mut;

const ENOENT: i64 = -2;
const EBADF: i64 = -9;
const ENOMEM: i64 = -12;
const EFAULT: i64 = -14;
const ENODEV: i64 = -19;
const EINVAL: i64 = -22;
const ENOTTY: i64 = -25;
const EROFS: i64 = -30;
const ENOSYS: i64 = -38;

const PAGE: u64 = 4096;
const AT_FDCWD: u64 = (-100i64) as u64;
const MAP_FIXED: u64 = 0x10;
const MAP_ANONYMOUS: u64 = 0x20;
const PROT_WRITE: u64 = 0x2;
const PROT_EXEC: u64 = 0x4;
const O_ACCMODE: u64 = 3;
const SEEK_SET: u64 = 0;
const SEEK_CUR: u64 = 1;
const SEEK_END: u64 = 2;
const ARCH_SET_FS: u64 = 0x1002;

/// Where anonymous mmap regions start. They grow upward from here.
const MMAP_BASE: u64 = 0x0000_1000_0000_0000;
const MMAP_LIMIT: u64 = 0x0000_7000_0000_0000;
/// The program break may grow this far past the end of the image.
const HEAP_LIMIT: u64 = 256 << 20;

struct OpenFile {
    data: Vec<u8>,
    pos: usize,
}

// One program runs at a time, so this state belongs to the current program.
// Slots 0, 1 and 2 are stdin, stdout and stderr, and stay empty here.
static mut FILES: Vec<Option<OpenFile>> = Vec::new();
static mut BRK_BASE: u64 = 0;
static mut BRK_CUR: u64 = 0;
static mut MMAP_NEXT: u64 = MMAP_BASE;
static mut RNG: u64 = 0x9E37_79B9_7F4A_7C15;

/// Reset the per-program state before a new program starts.
pub fn reset(brk_base: u64) {
    unsafe {
        let files = &mut *addr_of_mut!(FILES);
        files.clear();
        files.extend((0..3).map(|_| None));
        BRK_BASE = brk_base;
        BRK_CUR = brk_base;
        MMAP_NEXT = MMAP_BASE;
    }
}

/// Run a syscall. `a` holds the six Linux argument registers.
pub fn dispatch(nr: u64, a: [u64; 6]) -> u64 {
    let result: i64 = match nr {
        0 => read(a[0], a[1], a[2]),
        1 => write(a[0], a[1], a[2]),
        2 => open(a[0], a[1]),
        3 => close(a[0]),
        5 => fstat(a[0], a[1]),
        8 => lseek(a[0], a[1] as i64, a[2]),
        9 => mmap(a[0], a[1], a[2], a[3]),
        // Accepted but not enforced yet: memory protection, unmapping, signals.
        10 | 11 | 13 | 14 => 0,
        12 => brk(a[0]),
        16 => ENOTTY,
        20 => writev(a[0], a[1], a[2]),
        39 | 186 | 218 => 1, // getpid, gettid, set_tid_address: one process, pid 1
        60 | 231 => syscall::leave(a[0]),
        63 => uname(a[0]),
        158 => arch_prctl(a[0], a[1]),
        257 => {
            if a[0] == AT_FDCWD {
                open(a[1], a[2])
            } else {
                ENOENT
            }
        }
        267 => ENOENT, // readlinkat: there are no symbolic links yet
        318 => getrandom(a[0], a[1]),
        _ => ENOSYS,
    };
    result as u64
}

// ---- Helpers for moving data across the user boundary -------------------

/// Read a NUL-terminated string from user memory.
fn user_str(addr: u64) -> Option<String> {
    let mut out = Vec::new();
    let mut p = addr;
    loop {
        if !userspace::readable(p, 1) {
            return None;
        }
        let b = unsafe { *(p as *const u8) };
        if b == 0 {
            break;
        }
        out.push(b);
        if out.len() > 255 {
            return None;
        }
        p += 1;
    }
    String::from_utf8(out).ok()
}

/// Copy bytes into user memory. Returns false if the range is not mapped.
fn copy_to_user(dst: u64, src: &[u8]) -> bool {
    if !userspace::readable(dst, src.len() as u64) {
        return false;
    }
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len()) };
    true
}

fn page_up(x: u64) -> u64 {
    (x + PAGE - 1) & !(PAGE - 1)
}

fn file_mut(fd: u64) -> Option<&'static mut OpenFile> {
    let files = unsafe { &mut *addr_of_mut!(FILES) };
    files.get_mut(fd as usize)?.as_mut()
}

fn next_random() -> u64 {
    unsafe {
        let mut x = RNG ^ core::arch::x86_64::_rdtsc() ^ pit::ticks();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        RNG = x;
        x
    }
}

// ---- File descriptors ----------------------------------------------------

fn read(fd: u64, buf: u64, len: u64) -> i64 {
    if fd == 0 {
        return 0; // stdin has nothing to give yet
    }
    let file = match file_mut(fd) {
        Some(file) => file,
        None => return EBADF,
    };
    let n = (len as usize).min(file.data.len() - file.pos);
    let chunk = &file.data[file.pos..file.pos + n];
    if !copy_to_user(buf, chunk) {
        return EFAULT;
    }
    file.pos += n;
    n as i64
}

fn write(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != 1 && fd != 2 {
        return EBADF;
    }
    let len = len.min(4096);
    if !userspace::readable(buf, len) {
        return EFAULT;
    }
    let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) };
    terminal::print(&String::from_utf8_lossy(bytes));
    len as i64
}

fn writev(fd: u64, iov: u64, count: u64) -> i64 {
    let mut total = 0i64;
    for i in 0..count.min(16) {
        let at = iov + i * 16;
        if !userspace::readable(at, 16) {
            return EFAULT;
        }
        let base = unsafe { *(at as *const u64) };
        let len = unsafe { *((at + 8) as *const u64) };
        let n = write(fd, base, len);
        if n < 0 {
            return n;
        }
        total += n;
    }
    total
}

fn open(path: u64, flags: u64) -> i64 {
    let path = match user_str(path) {
        Some(p) => p,
        None => return EFAULT,
    };
    if flags & O_ACCMODE != 0 {
        return EROFS; // the filesystem is read-only from programs for now
    }
    let full = if path.starts_with('/') { path } else { format!("/{}", path) };
    let data = match fs::read(&full) {
        Some(data) => data,
        None => return ENOENT,
    };
    let files = unsafe { &mut *addr_of_mut!(FILES) };
    let file = Some(OpenFile { data, pos: 0 });
    match files.iter().skip(3).position(|slot| slot.is_none()) {
        Some(i) => {
            files[i + 3] = file;
            (i + 3) as i64
        }
        None => {
            files.push(file);
            (files.len() - 1) as i64
        }
    }
}

fn close(fd: u64) -> i64 {
    if fd < 3 {
        return 0;
    }
    let files = unsafe { &mut *addr_of_mut!(FILES) };
    match files.get_mut(fd as usize) {
        Some(slot) if slot.is_some() => {
            *slot = None;
            0
        }
        _ => EBADF,
    }
}

fn lseek(fd: u64, offset: i64, whence: u64) -> i64 {
    let file = match file_mut(fd) {
        Some(file) => file,
        None => return EBADF,
    };
    let base = match whence {
        SEEK_SET => 0,
        SEEK_CUR => file.pos as i64,
        SEEK_END => file.data.len() as i64,
        _ => return EINVAL,
    };
    let target = base + offset;
    if target < 0 {
        return EINVAL;
    }
    file.pos = target as usize;
    target
}

/// fstat. Fills in the fields musl reads: mode, size and block size.
fn fstat(fd: u64, buf: u64) -> i64 {
    let mut st = [0u8; 144];
    let (mode, size): (u32, i64) = if fd < 3 {
        (0o020620, 0) // character device, like a terminal
    } else {
        match file_mut(fd) {
            Some(file) => (0o100444, file.data.len() as i64), // regular, read-only
            None => return EBADF,
        }
    };
    st[24..28].copy_from_slice(&mode.to_le_bytes());
    st[48..56].copy_from_slice(&size.to_le_bytes());
    st[56..64].copy_from_slice(&(PAGE as i64).to_le_bytes());
    if copy_to_user(buf, &st) { 0 } else { EFAULT }
}

// ---- Memory --------------------------------------------------------------

fn mmap(addr: u64, len: u64, prot: u64, flags: u64) -> i64 {
    if flags & MAP_ANONYMOUS == 0 {
        return ENODEV; // file-backed mappings come later
    }
    if len == 0 {
        return EINVAL;
    }
    let len = page_up(len);
    let va = if flags & MAP_FIXED != 0 {
        if addr & (PAGE - 1) != 0 {
            return EINVAL;
        }
        addr
    } else {
        unsafe {
            let va = MMAP_NEXT;
            MMAP_NEXT += len;
            va
        }
    };
    if va.checked_add(len).map_or(true, |end| end > MMAP_LIMIT) {
        return ENOMEM;
    }
    match userspace::map_anon(va, len, prot & PROT_WRITE != 0, prot & PROT_EXEC != 0) {
        Ok(()) => va as i64,
        Err(_) => ENOMEM,
    }
}

/// brk: move the program break. Musl uses it for its heap.
fn brk(addr: u64) -> i64 {
    let (base, cur) = unsafe { (BRK_BASE, BRK_CUR) };
    if addr < base || addr > base + HEAP_LIMIT {
        return cur as i64; // report the current break and change nothing
    }
    let old_top = page_up(cur);
    let new_top = page_up(addr);
    if new_top > old_top && userspace::map_anon(old_top, new_top - old_top, true, false).is_err() {
        return cur as i64;
    }
    unsafe { BRK_CUR = addr };
    addr as i64
}

// ---- Process and system information -------------------------------------

fn arch_prctl(code: u64, addr: u64) -> i64 {
    match code {
        ARCH_SET_FS => {
            syscall::set_fs_base(addr);
            0
        }
        _ => EINVAL,
    }
}

/// uname: six fields of 65 bytes each, NUL-padded.
fn uname(buf: u64) -> i64 {
    let fields = ["Linux", "anarchy", "6.1.0-anarchy", "AnarchyOS", "x86_64", "(none)"];
    let mut out = [0u8; 65 * 6];
    for (i, field) in fields.iter().enumerate() {
        out[i * 65..i * 65 + field.len()].copy_from_slice(field.as_bytes());
    }
    if copy_to_user(buf, &out) { 0 } else { EFAULT }
}

fn getrandom(buf: u64, len: u64) -> i64 {
    let len = len.min(4096) as usize;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        out.extend_from_slice(&next_random().to_le_bytes());
    }
    out.truncate(len);
    if copy_to_user(buf, &out) { len as i64 } else { EFAULT }
}
