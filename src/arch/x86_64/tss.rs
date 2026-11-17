//! Task State Segment. In long mode its job is to hold the IST stacks,
//! which the CPU switches to for selected exceptions regardless of the
//! state of the current stack.

use core::mem::size_of;
use core::ptr::addr_of_mut;

/// IDT entries select an IST slot with a 1-based index.
pub const IST_DOUBLE_FAULT: u8 = 1;

const STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct Stack([u8; STACK_SIZE]);

static mut DF_STACK: Stack = Stack([0; STACK_SIZE]);

#[repr(C, packed)]
pub struct Tss {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

static mut TSS: Tss = Tss {
    reserved0: 0,
    rsp: [0; 3],
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: size_of::<Tss>() as u16,
};

pub fn init() {
    unsafe {
        let stack_base = addr_of_mut!(DF_STACK) as u64;
        let top = stack_base + STACK_SIZE as u64;
        let tss = addr_of_mut!(TSS);
        // ist is not 8-byte aligned inside the packed struct, so write unaligned.
        addr_of_mut!((*tss).ist[(IST_DOUBLE_FAULT - 1) as usize]).write_unaligned(top);
    }
}

/// Stack the CPU switches to when an interrupt or exception arrives while
/// a program runs in ring 3.
pub fn set_rsp0(top: u64) {
    unsafe {
        let tss = addr_of_mut!(TSS);
        addr_of_mut!((*tss).rsp[0]).write_unaligned(top);
    }
}

pub fn address() -> u64 {
    addr_of_mut!(TSS) as u64
}

pub fn size() -> u32 {
    size_of::<Tss>() as u32
}
