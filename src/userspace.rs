//! Programs that run in ring 3. Each one gets its own address space, a
//! static ELF64 executable is loaded into it, and it runs until it calls
//! exit or faults. Then its memory is freed.

use crate::arch::x86_64::idt::InterruptStackFrame;
use crate::arch::x86_64::syscall;
use crate::linux;
use crate::paging::{self, NO_EXECUTE, WRITABLE};
use crate::pmm;
use crate::terminal;
use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

const PAGE: u64 = 4096;
/// Everything below this address belongs to user programs.
const USER_LIMIT: u64 = 0x0000_8000_0000_0000;
const STACK_TOP: u64 = 0x0000_7FFF_FFF0_0000;
const STACK_PAGES: u64 = 16;
/// Exit code for a program that crashed, as a shell reports it (128 + SIGSEGV).
const FAULT_EXIT: u64 = 139;

const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

static RUNNING: AtomicBool = AtomicBool::new(false);
static mut CURRENT_ROOT: u64 = 0;

/// True while a program is on the CPU. The timer uses this to leave the
/// scheduler alone, because the program's kernel stack is not a task.
pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// Can the running program read `len` bytes at `addr`? Used to check
/// pointers handed to syscalls before the kernel touches them.
pub fn readable(addr: u64, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    let end = match addr.checked_add(len) {
        Some(end) if end <= USER_LIMIT => end,
        _ => return false,
    };
    let root = unsafe { CURRENT_ROOT };
    let mut page = addr & !(PAGE - 1);
    while page < end {
        if paging::translate(root, page).is_none() {
            return false;
        }
        page += PAGE;
    }
    true
}

/// Run an ELF64 image. Returns the program's exit code.
pub fn run(image: &[u8]) -> Result<u64, &'static str> {
    let kernel = paging::current_root();
    let root = paging::new_address_space(kernel).ok_or("out of memory")?;
    let outcome = load_and_run(root, image);
    paging::activate(kernel);
    RUNNING.store(false, Ordering::Relaxed);
    paging::free_user_space(root);
    outcome
}

fn load_and_run(root: u64, image: &[u8]) -> Result<u64, &'static str> {
    let (entry, brk_base) = load(root, image)?;
    linux::reset(brk_base);
    let sp = map_stack(root)?;
    unsafe { CURRENT_ROOT = root };
    paging::activate(root);
    RUNNING.store(true, Ordering::Relaxed);
    Ok(syscall::run_user(entry, sp))
}

/// Stop the running program because it faulted. Never returns to the program.
pub fn fault(what: &str, error_code: u64, frame: &InterruptStackFrame) -> ! {
    kprintln!("program fault: {} at {:#x}, error code {:#x}", what, frame.rip, error_code);
    terminal::print(&format!("program fault: {} at {:#x}", what, frame.rip));
    syscall::leave(FAULT_EXIT)
}

/// Map anonymous zeroed memory at `va`, skipping pages that are already mapped.
pub fn map_anon(va: u64, len: u64, writable: bool, exec: bool) -> Result<(), &'static str> {
    let root = unsafe { CURRENT_ROOT };
    let mut flags = 0;
    if writable {
        flags |= WRITABLE;
    }
    if !exec {
        flags |= NO_EXECUTE;
    }
    let mut page = va;
    while page < va + len {
        if paging::translate(root, page).is_none() {
            map_new_page(root, page, flags)?;
        }
        page += PAGE;
    }
    Ok(())
}

// ---- ELF loader ---------------------------------------------------------

fn le16(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    let mut a = [0u8; 4];
    a.copy_from_slice(b.get(at..at + 4)?);
    Some(u32::from_le_bytes(a))
}

fn le64(b: &[u8], at: usize) -> Option<u64> {
    let mut a = [0u8; 8];
    a.copy_from_slice(b.get(at..at + 8)?);
    Some(u64::from_le_bytes(a))
}

/// Map a fresh zeroed page at `va` and return its physical frame.
fn map_new_page(root: u64, va: u64, flags: u64) -> Result<u64, &'static str> {
    let frame = pmm::alloc_frame().ok_or("out of memory")?;
    unsafe { core::ptr::write_bytes(pmm::phys_to_virt(frame) as *mut u8, 0, PAGE as usize) };
    paging::map_user_4k(root, va, frame, flags).ok_or("could not map a page")?;
    Ok(frame)
}

/// Load the PT_LOAD segments of a static ELF64 executable. Returns the entry
/// point and the first page after the image, where the program break starts.
fn load(root: u64, image: &[u8]) -> Result<(u64, u64), &'static str> {
    if image.len() < 64 || &image[..4] != b"\x7fELF" {
        return Err("not an ELF file");
    }
    if image[4] != 2 || image[5] != 1 {
        return Err("only 64-bit little-endian ELF files run");
    }
    if le16(image, 16) != Some(ET_EXEC) {
        return Err("only static executables run");
    }
    if le16(image, 18) != Some(EM_X86_64) {
        return Err("the program is not x86_64");
    }
    let truncated = "truncated ELF file";
    let entry = le64(image, 24).ok_or(truncated)?;
    let phoff = le64(image, 32).ok_or(truncated)? as usize;
    let phentsize = le16(image, 54).ok_or(truncated)? as usize;
    let phnum = le16(image, 56).ok_or(truncated)? as usize;
    if phentsize < 56 {
        return Err("bad program header size");
    }

    let mut image_end = 0u64;
    for i in 0..phnum {
        let at = phoff + i * phentsize;
        let p_type = le32(image, at).ok_or(truncated)?;
        if p_type != PT_LOAD {
            continue;
        }
        let flags = le32(image, at + 4).ok_or(truncated)?;
        let offset = le64(image, at + 8).ok_or(truncated)?;
        let vaddr = le64(image, at + 16).ok_or(truncated)?;
        let filesz = le64(image, at + 32).ok_or(truncated)?;
        let memsz = le64(image, at + 40).ok_or(truncated)?;

        if memsz < filesz {
            return Err("bad segment sizes");
        }
        if vaddr < PAGE || vaddr.checked_add(memsz).map_or(true, |e| e > USER_LIMIT) {
            return Err("segment outside user space");
        }
        if offset.checked_add(filesz).map_or(true, |e| e > image.len() as u64) {
            return Err("segment runs past the end of the file");
        }

        load_segment(root, image, offset, vaddr, filesz, memsz, flags)?;
        image_end = image_end.max(vaddr + memsz);
    }
    Ok((entry, (image_end + PAGE - 1) & !(PAGE - 1)))
}

fn load_segment(
    root: u64,
    image: &[u8],
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    flags: u32,
) -> Result<(), &'static str> {
    let mut page_flags = 0;
    if flags & PF_W != 0 {
        page_flags |= WRITABLE;
    }
    if flags & PF_X == 0 {
        page_flags |= NO_EXECUTE;
    }

    let seg_end = vaddr + memsz;
    let file_end = vaddr + filesz;
    let mut page = vaddr & !(PAGE - 1);
    while page < seg_end {
        // Two segments can share a page. Reuse the frame the first one mapped.
        let frame = match paging::translate(root, page) {
            Some(existing) => existing & !(PAGE - 1),
            None => map_new_page(root, page, page_flags)?,
        };

        // Copy the part of the file that falls inside this page.
        let lo = page.max(vaddr);
        let hi = (page + PAGE).min(file_end);
        if lo < hi {
            let src = (offset + (lo - vaddr)) as usize;
            let dst = pmm::phys_to_virt(frame) + (lo - page);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    image.as_ptr().add(src),
                    dst as *mut u8,
                    (hi - lo) as usize,
                );
            }
        }
        page += PAGE;
    }
    Ok(())
}

/// Give the program a stack below STACK_TOP and return the initial stack pointer.
fn map_stack(root: u64) -> Result<u64, &'static str> {
    for i in 1..=STACK_PAGES {
        map_new_page(root, STACK_TOP - i * PAGE, WRITABLE | NO_EXECUTE)?;
    }
    Ok(STACK_TOP - 16)
}

// ---- Built-in test program ----------------------------------------------

/// A static program built in memory. It writes one line through write(2)
/// and exits with code 0 through exit(2). It exists so the ring 3 path can
/// be tested before a disk-based toolchain is available.
pub fn hello_image() -> Vec<u8> {
    const BASE: u64 = 0x40_0000;
    const HEADERS: u64 = 64 + 56;
    const CODE_LEN: u64 = 31;

    let msg = b"Hello from a ring 3 program.\n";
    let msg_addr = BASE + HEADERS + CODE_LEN;
    let total = HEADERS + CODE_LEN + msg.len() as u64;

    let mut out = Vec::new();

    // ELF header.
    out.extend_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0]);
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&2u16.to_le_bytes()); // e_type: executable
    out.extend_from_slice(&62u16.to_le_bytes()); // e_machine: x86_64
    out.extend_from_slice(&1u32.to_le_bytes()); // e_version
    out.extend_from_slice(&(BASE + HEADERS).to_le_bytes()); // e_entry
    out.extend_from_slice(&64u64.to_le_bytes()); // e_phoff
    out.extend_from_slice(&0u64.to_le_bytes()); // e_shoff
    out.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    out.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    out.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    out.extend_from_slice(&1u16.to_le_bytes()); // e_phnum
    out.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // One PT_LOAD segment for the whole file, readable and executable.
    out.extend_from_slice(&1u32.to_le_bytes()); // p_type
    out.extend_from_slice(&5u32.to_le_bytes()); // p_flags: R+X
    out.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    out.extend_from_slice(&BASE.to_le_bytes()); // p_vaddr
    out.extend_from_slice(&BASE.to_le_bytes()); // p_paddr
    out.extend_from_slice(&total.to_le_bytes()); // p_filesz
    out.extend_from_slice(&total.to_le_bytes()); // p_memsz
    out.extend_from_slice(&0x1000u64.to_le_bytes()); // p_align

    // Code.
    out.push(0xB8); // mov eax, 1 (write)
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(0xBF); // mov edi, 1 (stdout)
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(0xBE); // mov esi, msg
    out.extend_from_slice(&(msg_addr as u32).to_le_bytes());
    out.push(0xBA); // mov edx, len
    out.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0x0F, 0x05]); // syscall
    out.push(0xB8); // mov eax, 60 (exit)
    out.extend_from_slice(&60u32.to_le_bytes());
    out.extend_from_slice(&[0x31, 0xFF]); // xor edi, edi (code 0)
    out.extend_from_slice(&[0x0F, 0x05]); // syscall

    out.extend_from_slice(msg);
    out
}
