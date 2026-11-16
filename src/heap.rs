//! Kernel heap: a fixed virtual region backed by PMM frames, managed by a
//! first-fit free list kept sorted by address. Freed blocks merge with their
//! neighbours, so the heap does not fragment into unusable slivers.

use crate::paging;
use crate::pmm;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const HEAP_START: u64 = 0xffff_a000_0000_0000;
pub const HEAP_SIZE: usize = 8 * 1024 * 1024;

/// Every block starts and ends on an 8-byte boundary.
const fn round8(n: usize) -> usize {
    (n + 7) & !7
}

const MIN_BLOCK: usize = core::mem::size_of::<FreeBlock>();

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

struct Heap {
    head: AtomicUsize, // address of the first FreeBlock, 0 when empty
}

#[global_allocator]
static HEAP: Heap = Heap { head: AtomicUsize::new(0) };

/// Single core and no allocation from interrupt handlers, so disabling
/// interrupts around list edits is enough for now.
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
        let size = round8(layout.size().max(MIN_BLOCK));
        let align = layout.align().max(core::mem::align_of::<FreeBlock>());
        with_irqs_off(|| {
            let mut prev: *mut FreeBlock = ptr::null_mut();
            let mut cur = self.head.load(Ordering::Relaxed) as *mut FreeBlock;
            while !cur.is_null() {
                let block_start = cur as usize;
                let block_end = block_start + (*cur).size;
                let aligned = (block_start + align - 1) & !(align - 1);
                if aligned + size <= block_end {
                    let alloc_end = aligned + size;
                    let next = (*cur).next;
                    let mut replacement = next;

                    // Keep the tail as a free block if it is big enough.
                    let back = block_end - alloc_end;
                    if back >= MIN_BLOCK {
                        let tail = alloc_end as *mut FreeBlock;
                        (*tail).size = back;
                        (*tail).next = next;
                        replacement = tail;
                    }
                    // Keep the head as a free block if it is big enough.
                    let front = aligned - block_start;
                    if front >= MIN_BLOCK {
                        (*cur).size = front;
                        (*cur).next = replacement;
                        replacement = cur;
                    }

                    if prev.is_null() {
                        self.head.store(replacement as usize, Ordering::Relaxed);
                    } else {
                        (*prev).next = replacement;
                    }
                    return aligned as *mut u8;
                }
                prev = cur;
                cur = (*cur).next;
            }
            ptr::null_mut()
        })
    }

    unsafe fn dealloc(&self, ptr_: *mut u8, layout: Layout) {
        let size = round8(layout.size().max(MIN_BLOCK));
        with_irqs_off(|| {
            let block = ptr_ as *mut FreeBlock;
            let block_addr = block as usize;

            // Find the insertion point that keeps the list sorted by address.
            let mut prev: *mut FreeBlock = ptr::null_mut();
            let mut cur = self.head.load(Ordering::Relaxed) as *mut FreeBlock;
            while !cur.is_null() && (cur as usize) < block_addr {
                prev = cur;
                cur = (*cur).next;
            }

            (*block).size = size;
            (*block).next = cur;

            // Merge with the following block if it touches us.
            if !cur.is_null() && block_addr + size == cur as usize {
                (*block).size += (*cur).size;
                (*block).next = (*cur).next;
            }

            // Merge into the preceding block if we touch it, else link in.
            if !prev.is_null() && prev as usize + (*prev).size == block_addr {
                (*prev).size += (*block).size;
                (*prev).next = (*block).next;
            } else if prev.is_null() {
                self.head.store(block as usize, Ordering::Relaxed);
            } else {
                (*prev).next = block;
            }
        })
    }
}

/// Total bytes currently on the free list. Used to check for leaks.
pub fn free_bytes() -> usize {
    with_irqs_off(|| unsafe {
        let mut total = 0;
        let mut cur = HEAP.head.load(Ordering::Relaxed) as *mut FreeBlock;
        while !cur.is_null() {
            total += (*cur).size;
            cur = (*cur).next;
        }
        total
    })
}

/// Map the heap region page by page, then hand the whole thing to the allocator.
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
