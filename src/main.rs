#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
// Some drivers keep functions that nothing calls yet (the scheduler, frame
// freeing), and build.rs emits icons that are not all used.
#![allow(dead_code, unused_unsafe)]

extern crate alloc;

#[macro_use]
mod serial;
mod anarchyai;
mod arch;
mod console;
mod events;
mod framebuffer;
mod fs;
mod heap;
mod paging;
mod pic;
mod pit;
mod pmm;
mod clock;
mod cpu;
mod crypto;
mod files_app;
mod gfx;
mod image;
mod linux;
mod lock;
mod net;
mod netproto;
mod pci;
mod ps2;
mod rtc;
mod sched;
mod scene;
mod settings;
mod settings_app;
mod terminal;
mod theme;
mod ui;
mod userspace;
mod virtio_blk;
mod volume;
mod wm;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::arch::asm;
use core::panic::PanicInfo;
use events::Event;
use limine::BaseRevision;
use limine::request::{
    FramebufferRequest, HhdmRequest, MemoryMapRequest, RequestsEndMarker, RequestsStartMarker,
    StackSizeRequest,
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
#[link_section = ".requests"]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

/// The desktop draws through several layers of calls, so ask the
/// bootloader for more than its default 64 KiB of stack.
#[used]
#[link_section = ".requests"]
static STACK_SIZE_REQUEST: StackSizeRequest = StackSizeRequest::new().with_size(512 * 1024);

#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

fn main_loop() -> ! {
    loop {
        // Mouse movement that piled up while the last frame was drawing is
        // added together and handled as one move, so a slow frame makes
        // the pointer jump ahead instead of falling behind.
        let mut motion: Option<(i32, i32, u8)> = None;
        while let Some(ev) = events::pop() {
            match ev {
                Event::Mouse { dx, dy, buttons } => {
                    motion = match motion {
                        Some((px, py, held)) if held == buttons => Some((px + dx, py + dy, buttons)),
                        Some((px, py, held)) => {
                            // The buttons changed, so the earlier movement
                            // has to land first to keep clicks in place.
                            wm::mouse(px, py, held);
                            Some((dx, dy, buttons))
                        }
                        None => Some((dx, dy, buttons)),
                    };
                }
                Event::Key(c) => {
                    if let Some((px, py, held)) = motion.take() {
                        wm::mouse(px, py, held);
                    }
                    wm::key(c);
                }
                Event::Tick(t) => {
                    wm::tick(t);
                    net::poll();
                }
                Event::Empty => {}
            }
        }
        if let Some((px, py, held)) = motion {
            wm::mouse(px, py, held);
        }
        unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
    }
}

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

    match FRAMEBUFFER_REQUEST.get_response().and_then(|r| r.framebuffers().next()) {
        Some(fb) => {
            framebuffer::init(
                fb.addr() as u64,
                fb.width() as u64,
                fb.height() as u64,
                fb.pitch() as u64,
            );
            console::start();
            kprintln!(
                "framebuffer: {}x{} at {:#x}",
                framebuffer::width(),
                framebuffer::height(),
                fb.addr() as u64
            );
        }
        None => kprintln!("framebuffer: none reported, continuing on serial only"),
    }

    let memmap = MEMMAP_REQUEST.get_response().expect("no memory map from Limine");
    let hhdm = HHDM_REQUEST.get_response().expect("no HHDM from Limine");
    pmm::init(memmap.entries(), hhdm.offset());

    let root = paging::init();
    paging::self_test(root);

    heap::init(root);
    framebuffer::alloc_backbuffer();

    let before = heap::free_bytes();
    {
        let boxed = Box::new(0x5A17_u64);
        kprintln!("heap: Box holds {:#x}", *boxed);

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
    }
    let after = heap::free_bytes();
    kprintln!(
        "heap: free {} KiB before, {} KiB after ({})",
        before / 1024,
        after / 1024,
        if before == after { "merged, no leak" } else { "LEAK" }
    );

    asm!("int3");
    kprintln!("returned from breakpoint, exceptions work");

    ps2::init();
    if virtio_blk::init(root) {
        volume::probe();
        if volume::is_open() {
            kprintln!("storage: drive open");
        } else {
            kprintln!("storage: drive waiting for a passphrase");
        }
    } else {
        kprintln!("storage: no virtio disk attached");
    }

    net::init(root);
    console::disable();
    wm::init();
    pit::init();
    kprintln!("events, storage and desktop ready");

    asm!("sti", options(nomem, nostack));
    main_loop()
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
