#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

#[macro_use]
mod serial;
mod arch;
mod paging;
mod pmm;

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

    // Test 1: a breakpoint should print a message and return here.
    asm!("int3");
    kprintln!("returned from breakpoint, exceptions work");

    // Test 2: read an unmapped address. Expect a decoded page-fault report.
    // The handler halts, so this is the last test.
    kprintln!("testing page fault on unmapped address...");
    let _ = core::ptr::read_volatile(0xffff_9100_0000_0000u64 as *const u64);

    halt()
}

/// Re-enable this to test the double-fault stack. It halts, so call it
/// in place of the page-fault test.
#[allow(dead_code)]
fn double_fault_test() -> ! {
    kprintln!("testing double fault on IST stack...");
    unsafe {
        asm!(
            "mov rsp, {bad}",
            "int3",
            bad = in(reg) 0xdead_beef_0000_0000u64,
            options(noreturn),
        );
    }
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
