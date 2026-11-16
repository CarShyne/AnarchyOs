use crate::arch::x86_64::idt::{self, InterruptStackFrame};
use crate::pic;
use crate::wm;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

pub const HZ: u64 = 100;
const PIT_FREQ: u32 = 1_193_182;
const PIT_VECTOR: u8 = 32;

static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

extern "x86-interrupt" fn timer_irq(_frame: InterruptStackFrame) {
    let t = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    pic::eoi(0);
    wm::tick(t);
}

pub fn init() {
    let divisor = (PIT_FREQ / HZ as u32) as u16;
    unsafe {
        asm!("out dx, al", in("dx") 0x43u16, in("al") 0x36u8,
             options(nomem, nostack, preserves_flags));
        asm!("out dx, al", in("dx") 0x40u16, in("al") divisor as u8,
             options(nomem, nostack, preserves_flags));
        asm!("out dx, al", in("dx") 0x40u16, in("al") (divisor >> 8) as u8,
             options(nomem, nostack, preserves_flags));
    }
    idt::set_irq(PIT_VECTOR, timer_irq as *const () as u64);
    crate::kprintln!("pit: timer at {} Hz", HZ);
}
