//! Paging. We build our own top-level table instead of living on the one
//! Limine left us. The higher half is copied across, so the HHDM and the
//! kernel stay mapped through the switch. Then the kernel's own pages get
//! proper permissions: code read-only and executable, data non-executable.

use crate::pmm;
use core::arch::asm;
use core::ptr::addr_of;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const HUGE: u64 = 1 << 7;
pub const NO_EXECUTE: u64 = 1 << 63;

const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const ENTRIES: usize = 512;
const EFER: u32 = 0xC000_0080;
const EFER_NXE: u64 = 1 << 11;

/// A scratch page in the higher half, used by the self-test.
const TEST_VA: u64 = 0xffff_9000_0000_0000;

type Table = [u64; ENTRIES];

extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __bss_end: u8;
}

/// Access a page table by its physical address, through the HHDM.
fn table(phys: u64) -> &'static mut Table {
    // SAFETY: page tables come from the frame allocator, and the HHDM
    // maps all physical RAM.
    unsafe { &mut *(pmm::phys_to_virt(phys) as *mut Table) }
}

fn new_table() -> Option<u64> {
    let phys = pmm::alloc_frame()?;
    table(phys).fill(0);
    Some(phys)
}

/// Index into the table at `level` (4 = PML4, 1 = PT).
const fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1ff) as usize
}

fn flush(virt: u64) {
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) }
}

fn read_cr3() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value & ADDR_MASK
}

fn write_cr3(phys: u64) {
    unsafe { asm!("mov cr3, {}", in(reg) phys, options(nostack, preserves_flags)) };
}

/// Turn one 2 MiB page into 512 4 KiB pages with the same permissions.
fn split_huge(pd_phys: u64, idx: usize) -> Option<u64> {
    let pd = table(pd_phys);
    let huge = pd[idx];
    let base = huge & ADDR_MASK & !0x1f_ffff;
    let flags = huge & !(ADDR_MASK | HUGE);

    let pt_phys = new_table()?;
    let pt = table(pt_phys);
    for (i, entry) in pt.iter_mut().enumerate() {
        *entry = (base + (i as u64) * 4096) | flags;
    }
    pd[idx] = pt_phys | PRESENT | WRITABLE | (huge & USER);
    Some(pt_phys)
}

/// Walk down to the page table that maps `virt`. With `create` set,
/// missing tables are allocated. Huge pages are split as we pass them.
fn walk_to_pt(root: u64, virt: u64, create: bool) -> Option<u64> {
    let mut phys = root;
    for level in [4u32, 3, 2] {
        let idx = index(virt, level);
        let entry = table(phys)[idx];
        if entry & PRESENT == 0 {
            if !create {
                return None;
            }
            let next = new_table()?;
            table(phys)[idx] = next | PRESENT | WRITABLE;
            phys = next;
        } else if entry & HUGE != 0 {
            if level != 2 {
                return None; // 1 GiB pages are not supported
            }
            phys = split_huge(phys, idx)?;
        } else {
            phys = entry & ADDR_MASK;
        }
    }
    Some(phys)
}

/// Map one 4 KiB page `virt` -> `phys` with the given flags.
pub fn map_4k(root: u64, virt: u64, phys: u64, flags: u64) -> Option<()> {
    let pt = walk_to_pt(root, virt, true)?;
    table(pt)[index(virt, 1)] = (phys & ADDR_MASK) | flags | PRESENT;
    flush(virt);
    Some(())
}

/// Change the permissions of a page that is already mapped.
fn set_flags(root: u64, virt: u64, flags: u64) -> Option<()> {
    let pt = walk_to_pt(root, virt, false)?;
    let entry = &mut table(pt)[index(virt, 1)];
    if *entry & PRESENT == 0 {
        return None;
    }
    *entry = (*entry & ADDR_MASK) | flags | PRESENT;
    flush(virt);
    Some(())
}

/// Physical address behind `virt`, or None if unmapped.
pub fn translate(root: u64, virt: u64) -> Option<u64> {
    let pt = walk_to_pt(root, virt, false)?;
    let entry = table(pt)[index(virt, 1)];
    if entry & PRESENT == 0 {
        return None;
    }
    Some((entry & ADDR_MASK) | (virt & 0xfff))
}

fn protect(root: u64, start: u64, end: u64, flags: u64) {
    let mut va = start;
    while va < end {
        set_flags(root, va, flags).expect("kernel page missing from mapping");
        va += 4096;
    }
}

fn enable_nx() {
    unsafe {
        let lo: u32;
        let hi: u32;
        asm!("rdmsr", in("ecx") EFER, out("eax") lo, out("edx") hi,
             options(nomem, nostack, preserves_flags));
        let value = (((hi as u64) << 32) | lo as u64) | EFER_NXE;
        asm!("wrmsr", in("ecx") EFER, in("eax") value as u32,
             in("edx") (value >> 32) as u32,
             options(nomem, nostack, preserves_flags));
    }
}

/// Build our own page tables, protect the kernel sections, and switch to them.
/// Returns the physical address of the new PML4.
pub fn init() -> u64 {
    enable_nx();

    let old_root = read_cr3();
    let new_root = new_table().expect("no frame for PML4");

    // Share Limine's higher-half tables: HHDM and kernel stay reachable.
    for i in 256..ENTRIES {
        table(new_root)[i] = table(old_root)[i];
    }

    unsafe {
        protect(new_root,
                addr_of!(__text_start) as u64, addr_of!(__text_end) as u64, 0);
        protect(new_root,
                addr_of!(__rodata_start) as u64, addr_of!(__rodata_end) as u64,
                NO_EXECUTE);
        protect(new_root,
                addr_of!(__data_start) as u64, addr_of!(__bss_end) as u64,
                WRITABLE | NO_EXECUTE);
    }

    write_cr3(new_root);
    kprintln!("paging: switched to own PML4 at {:#x}, kernel sections protected", new_root);
    new_root
}

/// Map a scratch page, write through it, and check the mapping both ways.
pub fn self_test(root: u64) {
    let frame = pmm::alloc_frame().expect("out of frames");
    map_4k(root, TEST_VA, frame, WRITABLE | NO_EXECUTE).expect("map failed");

    let translated_ok = translate(root, TEST_VA) == Some(frame);

    let ptr = TEST_VA as *mut u64;
    let pattern: u64 = 0xA11C_E000_0000_0001;
    unsafe { ptr.write_volatile(pattern) };
    let readback = unsafe { ptr.read_volatile() };

    kprintln!(
        "paging: {:#x} -> {:#x} ({}), write/read {}",
        TEST_VA,
        frame,
        if translated_ok { "translate ok" } else { "MISMATCH" },
        if readback == pattern { "ok" } else { "MISMATCH" }
    );
}

/// The PML4 that is active right now.
pub fn current_root() -> u64 {
    read_cr3()
}

/// Make `root` the active address space.
pub fn activate(root: u64) {
    write_cr3(root);
}

/// A new, empty lower half for a user program. The higher half is shared
/// with the kernel, so the kernel stays reachable when the program is running.
pub fn new_address_space(kernel_root: u64) -> Option<u64> {
    let root = new_table()?;
    for i in 256..ENTRIES {
        table(root)[i] = table(kernel_root)[i];
    }
    Some(root)
}

/// Map a user page. Unlike `map_4k`, every table on the way down gets the
/// USER bit, because the CPU checks the bit at each level.
pub fn map_user_4k(root: u64, virt: u64, phys: u64, flags: u64) -> Option<()> {
    let mut current = root;
    for level in [4u32, 3, 2] {
        let idx = index(virt, level);
        let entry = table(current)[idx];
        current = if entry & PRESENT == 0 {
            let next = new_table()?;
            table(current)[idx] = next | PRESENT | WRITABLE | USER;
            next
        } else if entry & HUGE != 0 {
            return None; // user mappings never sit under a huge page
        } else {
            table(current)[idx] = entry | USER;
            entry & ADDR_MASK
        };
    }
    table(current)[index(virt, 1)] = (phys & ADDR_MASK) | flags | PRESENT | USER;
    flush(virt);
    Some(())
}

/// Free every frame in the lower half of `root`: data pages, the tables that
/// map them, and the PML4 itself. The higher half is shared, so it is left alone.
pub fn free_user_space(root: u64) {
    for i in 0..256 {
        let entry = table(root)[i];
        if entry & PRESENT != 0 {
            free_level(entry & ADDR_MASK, 3);
        }
    }
    pmm::free_frame(root);
}

fn free_level(phys: u64, level: u32) {
    if level > 1 {
        for entry in table(phys).iter() {
            if entry & PRESENT != 0 && entry & HUGE == 0 {
                free_level(entry & ADDR_MASK, level - 1);
            }
        }
    } else {
        for entry in table(phys).iter() {
            if entry & PRESENT != 0 {
                pmm::free_frame(entry & ADDR_MASK);
            }
        }
    }
    pmm::free_frame(phys);
}

/// Decode the page-fault error code into plain words.
pub fn describe_page_fault(error_code: u64) {
    let cause = if error_code & 1 != 0 { "protection violation" } else { "page not present" };
    let access = if error_code & 2 != 0 { "write" } else { "read" };
    let mode = if error_code & 4 != 0 { "user" } else { "kernel" };
    let kind = if error_code & 16 != 0 { "instruction fetch" } else { "data access" };
    kprintln!("  cause    = {}", cause);
    kprintln!("  access   = {} ({})", access, kind);
    kprintln!("  mode     = {}", mode);
    if error_code & 8 != 0 {
        kprintln!("  reserved = bit set in a paging entry");
    }
}
