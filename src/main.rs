#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

#[macro_use]
mod serial;
mod arch;

use core::arch::asm;
use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::request::{RequestsEndMarker, RequestsStartMarker};

#[used]
#[link_section = ".requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

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
    kprintln!("gdt and idt loaded");

    // Test: a breakpoint should print a message and return here.
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
