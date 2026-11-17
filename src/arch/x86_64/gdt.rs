//! Global Descriptor Table. In 64-bit mode the segments are mostly
//! flat, but the CPU still needs valid code, data, and TSS descriptors.

use crate::arch::x86_64::tss;
use core::arch::asm;
use core::ptr::addr_of_mut;

// Layout chosen so that `sysret` lands on the user selectors: with STAR
// holding 0x10, sysret loads CS = 0x10 + 16 and SS = 0x10 + 8, both with RPL 3.
pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_DATA: u16 = 0x1B; // index 3, RPL 3
pub const USER_CODE: u16 = 0x23; // index 4, RPL 3
pub const TSS_SELECTOR: u16 = 0x28;

/// Layout the CPU expects for lgdt and lidt.
#[repr(C, packed)]
pub struct DescriptorTablePointer {
    pub limit: u16,
    pub base: u64,
}

// This must be writable. The CPU sets Accessed bits when segments load,
// and the Busy bit when ltr runs. Both are writes into the table.
static mut GDT: [u64; 7] = [
    0,                     // null descriptor, required
    0x00AF_9B00_0000_FFFF, // kernel code: present, ring 0, executable, long mode, accessed
    0x00CF_9300_0000_FFFF, // kernel data: present, ring 0, writable, accessed
    0x00CF_F300_0000_FFFF, // user data: present, ring 3, writable
    0x00AF_FB00_0000_FFFF, // user code: present, ring 3, executable, long mode
    0,                     // TSS low half, filled in at init
    0,                     // TSS high half, filled in at init
];

/// Build the two 8-byte words of a 64-bit available TSS descriptor.
fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let limit = limit as u64;
    let low = (limit & 0xFFFF)
        | ((base & 0xFFFF) << 16)
        | (((base >> 16) & 0xFF) << 32)
        | (0x89u64 << 40) // present, type 0x9: available 64-bit TSS
        | (((limit >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = base >> 32;
    (low, high)
}

pub fn init() {
    unsafe {
        let gdt = &mut *addr_of_mut!(GDT);

        let (low, high) = tss_descriptor(tss::address(), tss::size() - 1);
        gdt[5] = low;
        gdt[6] = high;

        let ptr = DescriptorTablePointer {
            limit: (gdt.len() * core::mem::size_of::<u64>() - 1) as u16,
            base: gdt.as_ptr() as u64,
        };

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

        // Load the task register so the CPU can find the IST stacks.
        asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }
}
