//! Legacy 8259 interrupt controllers. We remap them above the CPU exception
//! range (IRQ 0-15 become vectors 32-47), then unmask only the lines we use.

use core::arch::asm;

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const EOI: u8 = 0x20;

#[inline]
unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value,
         options(nomem, nostack, preserves_flags));
}

pub fn remap() {
    unsafe {
        // Start initialization, cascade mode.
        outb(PIC1_CMD, 0x11);
        outb(PIC2_CMD, 0x11);
        // Vector offsets: master at 32, slave at 40.
        outb(PIC1_DATA, 32);
        outb(PIC2_DATA, 40);
        // Wiring: slave on IRQ2 of master, then cascade identity.
        outb(PIC1_DATA, 4);
        outb(PIC2_DATA, 2);
        // 8086 mode.
        outb(PIC1_DATA, 0x01);
        outb(PIC2_DATA, 0x01);
        // Masks: a 0 bit means enabled.
        // Master: keyboard (IRQ1) and cascade (IRQ2) enabled, timer masked.
        outb(PIC1_DATA, 0b1111_1001);
        // Slave: mouse (IRQ12 = slave bit 4) enabled, everything else masked.
        outb(PIC2_DATA, 0b1110_1111);
    }
}

/// Acknowledge an IRQ so the controller will deliver the next one.
pub fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_CMD, EOI);
        }
        outb(PIC1_CMD, EOI);
    }
}
