//! Physical memory manager. Hands out 4 KiB frames from the usable
//! regions reported by Limine. Freed frames go on a singly linked list
//! whose links are stored inside the free frames themselves, reached
//! through the higher-half direct map (HHDM).

use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicU64, Ordering};
use limine::memory_map::{Entry, EntryType};

pub const PAGE_SIZE: u64 = 4096;

/// Skip the first 1 MiB. It holds legacy BIOS and real-mode data we
/// have no business handing out.
const LOW_LIMIT: u64 = 0x10_0000;
const MAX_REGIONS: usize = 64;

#[derive(Clone, Copy)]
struct Region {
    next: u64,
    end: u64,
}

static mut REGIONS: [Region; MAX_REGIONS] = [Region { next: 0, end: 0 }; MAX_REGIONS];
static mut REGION_COUNT: usize = 0;

/// Head of the free list. Physical address 0 is never handed out, so it
/// doubles as the end-of-list marker.
static FREE_LIST: AtomicU64 = AtomicU64::new(0);
static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES: AtomicU64 = AtomicU64::new(0);

const fn align_up(x: u64, a: u64) -> u64 {
    (x + a - 1) & !(a - 1)
}

const fn align_down(x: u64, a: u64) -> u64 {
    x & !(a - 1)
}

/// Translate a physical address to a virtual one we can dereference.
pub fn phys_to_virt(phys: u64) -> u64 {
    phys + HHDM_OFFSET.load(Ordering::Relaxed)
}

pub fn init(entries: &[&Entry], hhdm_offset: u64) {
    HHDM_OFFSET.store(hhdm_offset, Ordering::Relaxed);

    let regions = unsafe { &mut *addr_of_mut!(REGIONS) };
    let mut count = 0;
    let mut total: u64 = 0;

    for entry in entries {
        if entry.entry_type != EntryType::USABLE {
            continue;
        }
        if count == MAX_REGIONS {
            break;
        }
        let start = align_up(entry.base.max(LOW_LIMIT), PAGE_SIZE);
        let end = align_down(entry.base + entry.length, PAGE_SIZE);
        if end <= start {
            continue;
        }
        regions[count] = Region { next: start, end };
        count += 1;
        total += end - start;
    }

    unsafe { *addr_of_mut!(REGION_COUNT) = count };
    TOTAL_BYTES.store(total, Ordering::Relaxed);
    kprintln!(
        "pmm: {} usable regions, {} MiB free",
        count,
        total / (1024 * 1024)
    );
}

/// Usable RAM found at boot, in bytes.
pub fn total_bytes() -> u64 {
    TOTAL_BYTES.load(Ordering::Relaxed)
}

/// Allocate one 4 KiB physical frame. Returns its physical address.
pub fn alloc_frame() -> Option<u64> {
    // Reuse freed frames first.
    let head = FREE_LIST.load(Ordering::Relaxed);
    if head != 0 {
        let next = unsafe { *(phys_to_virt(head) as *const u64) };
        FREE_LIST.store(next, Ordering::Relaxed);
        return Some(head);
    }

    // Otherwise carve a fresh frame from the bump cursor of a region.
    let regions = unsafe { &mut *addr_of_mut!(REGIONS) };
    let count = unsafe { *addr_of!(REGION_COUNT) };
    for region in regions[..count].iter_mut() {
        if region.next < region.end {
            let frame = region.next;
            region.next += PAGE_SIZE;
            return Some(frame);
        }
    }
    None
}

/// Return a frame to the allocator. The caller must not use it afterwards.
pub fn free_frame(phys: u64) {
    let old_head = FREE_LIST.load(Ordering::Relaxed);
    unsafe { *(phys_to_virt(phys) as *mut u64) = old_head };
    FREE_LIST.store(phys, Ordering::Relaxed);
}
