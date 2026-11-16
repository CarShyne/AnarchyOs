pub mod gdt;
pub mod idt;

/// Bring up the CPU tables. The GDT must come first, because
/// IDT entries refer to the kernel code segment selector.
pub fn init() {
    gdt::init();
    idt::init();
}
