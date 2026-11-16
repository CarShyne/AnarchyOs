//! Interrupt Descriptor Table. Maps CPU exceptions (and later hardware
//! interrupts) to handler functions.

use crate::arch::x86_64::gdt::{DescriptorTablePointer, KERNEL_CODE};
use crate::arch::x86_64::tss;
use core::arch::asm;
use core::ptr::addr_of_mut;

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl Entry {
    const MISSING: Entry = Entry {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    /// `ist` is 0 for the normal stack, or 1-7 to switch to a TSS IST stack.
    fn set(&mut self, handler: u64, ist: u8) {
        self.offset_low = handler as u16;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.selector = KERNEL_CODE;
        self.ist = ist;
        self.type_attr = 0x8E; // present, ring 0, 64-bit interrupt gate
        self.reserved = 0;
    }
}

/// The stack frame the CPU pushes before calling a handler.
#[repr(C)]
pub struct InterruptStackFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

static mut IDT: [Entry; 256] = [Entry::MISSING; 256];

fn dump(frame: &InterruptStackFrame) {
    kprintln!("  rip    = {:#018x}", frame.rip);
    kprintln!("  cs     = {:#x}", frame.cs);
    kprintln!("  rflags = {:#018x}", frame.rflags);
    kprintln!("  rsp    = {:#018x}", frame.rsp);
    kprintln!("  ss     = {:#x}", frame.ss);
}

fn halt() -> ! {
    loop {
        unsafe { asm!("hlt", options(nomem, nostack)) }
    }
}

// ---- Exception handlers -----------------------------------------------

extern "x86-interrupt" fn divide_error(frame: InterruptStackFrame) {
    kprintln!("EXCEPTION: divide error (#DE)");
    dump(&frame);
    halt();
}

extern "x86-interrupt" fn breakpoint(frame: InterruptStackFrame) {
    // Traps return normally, so this is a useful test of the IDT.
    kprintln!("EXCEPTION: breakpoint (#BP), continuing");
    dump(&frame);
}

extern "x86-interrupt" fn invalid_opcode(frame: InterruptStackFrame) {
    kprintln!("EXCEPTION: invalid opcode (#UD)");
    dump(&frame);
    halt();
}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, error_code: u64) -> ! {
    // Runs on the IST stack, so it works even when the main stack is broken.
    kprintln!("EXCEPTION: double fault (#DF), error code {:#x}", error_code);
    dump(&frame);
    halt();
}

extern "x86-interrupt" fn general_protection(frame: InterruptStackFrame, error_code: u64) {
    kprintln!("EXCEPTION: general protection fault (#GP), error code {:#x}", error_code);
    dump(&frame);
    halt();
}

extern "x86-interrupt" fn page_fault(frame: InterruptStackFrame, error_code: u64) {
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
    kprintln!("EXCEPTION: page fault (#PF)");
    kprintln!("  faulting address = {:#018x}", cr2);
    kprintln!("  error code       = {:#x}", error_code);
    dump(&frame);
    halt();
}

// ---- Setup -------------------------------------------------------------

pub fn init() {
    unsafe {
        let idt = &mut *addr_of_mut!(IDT);

        idt[0].set(divide_error as *const () as u64, 0);
        idt[3].set(breakpoint as *const () as u64, 0);
        idt[6].set(invalid_opcode as *const () as u64, 0);
        idt[8].set(double_fault as *const () as u64, tss::IST_DOUBLE_FAULT);
        idt[13].set(general_protection as *const () as u64, 0);
        idt[14].set(page_fault as *const () as u64, 0);

        let ptr = DescriptorTablePointer {
            limit: (idt.len() * core::mem::size_of::<Entry>() - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &ptr as *const DescriptorTablePointer,
             options(readonly, nostack, preserves_flags));
    }
}
