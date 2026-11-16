#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

#[macro_use]
mod serial;
mod arch;
mod heap;
mod paging;
mod pmm;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::arch::asm;
use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::request::{
    HhdmRequest, MemoryMapRequest, RequestsEndMarker, RequestsStartMarker,
};

#[used]
#[link_section = ".requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[link_section = ".requests"]
static MEMMAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[link_section = ".requests"]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    serial::init();

    kprintln!("AnarchyOS kernel v0.1.0");
    if !BASE_REVISION.is_supported() {
        kprintln!("warning: bootloader does not support our Limine revision");
    }
    kprintln!("booted via Limine, serial online");

    arch::x86_64::init();
    kprintln!("gdt, tss and idt loaded");

    let memmap = MEMMAP_REQUEST.get_response().expect("no memory map from Limine");
    let hhdm = HHDM_REQUEST.get_response().expect("no HHDM from Limine");
    pmm::init(memmap.entries(), hhdm.offset());

    let root = paging::init();
    paging::self_test(root);

    heap::init(root);

    // Test 1: a Box on the heap.
    let boxed = Box::new(0x5A17_u64);
    kprintln!("heap: Box holds {:#x}", *boxed);
    drop(boxed);

    // Test 2: a Vec that grows past one 4 KiB page.
    let mut v: Vec<u64> = Vec::new();
    for i in 0..2048u64 {
        v.push(i * 3);
    }
    let sum: u64 = v.iter().sum();
    let expected: u64 = (0..2048u64).map(|i| i * 3).sum();
    kprintln!(
        "heap: Vec grew to {} entries ({} KiB), sum {}",
        v.len(),
        v.len() * 8 / 1024,
        if sum == expected { "ok" } else { "MISMATCH" }
    );
    drop(v);

    kprintln!("heap tests done");

    // The breakpoint test returns, so run it last before halting.
    asm!("int3");
    kprintln!("returned from breakpoint, exceptions work");

    halt()
}

fn halt() -> ! {
    loop {
        unsafe { asm!("hlt", options(nomem, nostack)) }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kprintln!("KERNEL PANIC: {}", info);
    halt()
}
