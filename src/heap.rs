//! Kernel heap. A fixed virtual region, backed by frames from the PMM,
//! managed by a first-fit free-list allocator. It is what `alloc` uses
//! for Box, Vec and String.

use crate::paging;
use crate::pmm;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

pub const HEAP_START: u64 = 0xffff_a000_0000_0000;
pub const HEAP_SIZE: usize = 1024 * 1024; // 1 MiB

const MIN_BLOCK: usize = core::mem::size_of::<FreeBlock>();

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

struct Heap {
    head: AtomicUsize, // pointer to first FreeBlock, 0 when empty
}

#[global_allocator]
static HEAP: Heap = Heap { head: AtomicUsize::new(0) };

/// Single-core kernel for now, so an interrupt-free critical section is
/// enough. This needs a real lock once we have SMP or interrupts that allocate.
fn with_irqs_off<T>(f: impl FnOnce() -> T) -> T {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}; cli", out(reg) flags, options(nomem, preserves_flags));
    }
    let result = f();
    if flags & (1 << 9) != 0 {
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }
    result
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(MIN_BLOCK);
        let align = layout.align().max(core::mem::align_of::<FreeBlock>());
        with_irqs_off(|| {
            let mut prev: *mut FreeBlock = ptr::null_mut();
            let mut cur = self.head.load(Ordering::Relaxed) as *mut FreeBlock;
            while !cur.is_null() {
                let block_start = cur as usize;
                let block_end = block_start + (*cur).size;
                let aligned = (block_start + align - 1) & !(align - 1);
                if aligned + size <= block_end {
                    // Split the block: keep any space before and after the allocation.
                    let next = (*cur).next;
                    let mut replacement = next;
                    let tail = aligned + size;
                    if block_end - tail >= MIN_BLOCK {
                        let tail_block = tail as *mut FreeBlock;
                        (*tail_block).size = block_end - tail;
                        (*tail_block).next = next;
                        replacement = tail_block;
                    }
                    if aligned - block_start >= MIN_BLOCK {
                        (*cur).size = aligned - block_start;
                        (*cur).next = replacement;
                    } else if prev.is_null() {
                        self.head.store(replacement as usize, Ordering::Relaxed);
                    } else {
                        (*prev).next = replacement;
                    }
                    if aligned - block_start < MIN_BLOCK && prev.is_null() {
                        // head already updated above
                    } else if aligned - block_start < MIN_BLOCK {
                        // prev already updated above
                    }
                    return aligned as *mut u8;
                }
                prev = cur;
                cur = (*cur).next;
            }
            ptr::null_mut()
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size().max(MIN_BLOCK);
        with_irqs_off(|| {
            let block = ptr as *mut FreeBlock;
            (*block).size = size;
            // Insert at the head. Coalescing is skipped for this first version.
            (*block).next = self.head.load(Ordering::Relaxed) as *mut FreeBlock;
            self.head.store(block as usize, Ordering::Relaxed);
        })
    }
}

/// Map the heap region page by page, then give the whole thing to the allocator.
pub fn init(root: u64) {
    let pages = HEAP_SIZE / pmm::PAGE_SIZE as usize;
    for i in 0..pages {
        let frame = pmm::alloc_frame().expect("out of frames for heap");
        let virt = HEAP_START + (i as u64) * pmm::PAGE_SIZE;
        paging::map_4k(root, virt, frame, paging::WRITABLE | paging::NO_EXECUTE)
            .expect("heap map failed");
    }

    unsafe {
        let block = HEAP_START as *mut FreeBlock;
        (*block).size = HEAP_SIZE;
        (*block).next = ptr::null_mut();
        HEAP.head.store(block as usize, Ordering::Relaxed);
    }
    crate::kprintln!("heap: {} KiB at {:#x}", HEAP_SIZE / 1024, HEAP_START);
}

/// Allocation failure is fatal in the kernel.
#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    panic!("kernel out of heap memory: {:?}", layout);
}

#[allow(dead_code)]
fn _unused(_: NonNull<u8>) {}
