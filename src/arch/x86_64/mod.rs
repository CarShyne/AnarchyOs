pub mod gdt;
pub mod idt;
pub mod syscall;
pub mod tss;

/// Bring up the CPU tables. Order matters: the TSS must be filled in
/// before the GDT references it, and the GDT must be loaded before the
/// IDT, because IDT entries refer to the kernel code selector. The syscall
/// MSRs come last because they name the GDT selectors.
pub fn init() {
    tss::init();
    gdt::init();
    idt::init();
    syscall::init();
}
