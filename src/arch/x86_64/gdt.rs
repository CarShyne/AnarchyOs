//! Global Descriptor Table. In 64-bit mode the segments are mostly
//! flat, but the CPU still needs valid code and data selectors.

use core::arch::asm;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;

/// Layout the CPU expects for lgdt and lidt.
#[repr(C, packed)]
pub struct DescriptorTablePointer {
    pub limit: u16,
    pub base: u64,
}

static GDT: [u64; 3] = [
    0,                     // null descriptor, required
    0x00AF_9B00_0000_FFFF, // kernel code: present, ring 0, executable, long mode
    0x00CF_9300_0000_FFFF, // kernel data: present, ring 0, writable
];

pub fn init() {
    let ptr = DescriptorTablePointer {
        limit: (core::mem::size_of_val(&GDT) - 1) as u16,
        base: GDT.as_ptr() as u64,
    };

    unsafe {
        asm!(
            "lgdt [{ptr}]",
            // Far return to reload CS with the kernel code selector.
            "push {code}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            // Reload the data segment registers.
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov ss, {data:x}",
            ptr = in(reg) &ptr as *const DescriptorTablePointer,
            code = in(reg) KERNEL_CODE as u64,
            data = in(reg) KERNEL_DATA as u64,
            tmp = out(reg) _,
        );
    }
}
