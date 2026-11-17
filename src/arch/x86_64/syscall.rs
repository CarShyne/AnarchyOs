//! The bridge between ring 3 and the kernel.
//!
//! Three paths live here:
//! - `enter_user` saves the kernel's registers, then drops to ring 3 with iretq.
//! - `syscall_entry` is what the `syscall` instruction jumps to. It switches
//!   to a kernel stack, calls the dispatcher, and returns with sysret.
//! - `user_return` is how a program leaves. It puts back the kernel stack saved
//!   by `enter_user`, so the call to `enter_user` returns with the exit code.
//!
//! Syscall numbers and argument registers follow Linux x86_64, so static
//! Linux binaries can use this interface later.

use crate::arch::x86_64::gdt::KERNEL_CODE;
use crate::arch::x86_64::tss;
use core::arch::{asm, global_asm};
use core::ptr::addr_of_mut;

const MSR_FS_BASE: u32 = 0xC000_0100;
const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_SFMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1 << 0;
const RFLAGS_IF: u64 = 1 << 9;

const KSTACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct KStack([u8; KSTACK_SIZE]);

static mut KSTACK: KStack = KStack([0; KSTACK_SIZE]);

// Scratch words shared with the assembly below. One CPU and one running
// program at a time, so a single slot of each is enough.
#[no_mangle]
static mut USER_RSP_SAVE: u64 = 0;
#[no_mangle]
static mut KSTACK_TOP: u64 = 0;
#[no_mangle]
static mut KERNEL_RSP_SAVE: u64 = 0;

global_asm!(
    ".globl syscall_entry",
    "syscall_entry:",
    "    mov [rip + USER_RSP_SAVE], rsp",
    "    mov rsp, [rip + KSTACK_TOP]",
    "    push qword ptr [rip + USER_RSP_SAVE]",
    "    push rcx",           // user rip, needed by sysret
    "    push r11",           // user rflags, needed by sysret
    // Build a struct on the stack: { nr, a0..a5 }, lowest address first.
    // Linux passes nr in rax and args in rdi, rsi, rdx, r10, r8, r9.
    "    push r9",            // a5
    "    push r8",            // a4
    "    push r10",           // a3
    "    push rdx",           // a2
    "    push rsi",           // a1
    "    push rdi",           // a0
    "    push rax",           // nr
    "    mov rdi, rsp",       // pointer to the struct, first C argument
    "    call syscall_dispatch",
    "    add rsp, 56",        // drop the seven argument words
    "    pop r11",
    "    pop rcx",
    "    pop rsp",            // back to the user stack
    "    sysretq",
);

global_asm!(
    ".globl enter_user",
    "enter_user:",
    // rdi = entry point, rsi = user stack pointer.
    "    push rbp",
    "    push rbx",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    pushfq",
    "    mov [rip + KERNEL_RSP_SAVE], rsp",
    // The frame iretq pops: ss, rsp, rflags, cs, rip.
    "    push 0x1B",          // user data selector, RPL 3
    "    push rsi",
    "    push 0x202",         // IF set, bit 1 always set
    "    push 0x23",          // user code selector, RPL 3
    "    push rdi",
    "    xor eax, eax",
    "    xor ebx, ebx",
    "    xor ecx, ecx",
    "    xor edx, edx",
    "    xor esi, esi",
    "    xor edi, edi",
    "    xor ebp, ebp",
    "    xor r8d, r8d",
    "    xor r9d, r9d",
    "    xor r10d, r10d",
    "    xor r11d, r11d",
    "    xor r12d, r12d",
    "    xor r13d, r13d",
    "    xor r14d, r14d",
    "    xor r15d, r15d",
    "    iretq",
);

global_asm!(
    ".globl user_return",
    "user_return:",
    // rdi = exit code. Restore the registers that enter_user saved and return
    // the code to whoever called it.
    "    mov rax, rdi",
    "    mov rsp, [rip + KERNEL_RSP_SAVE]",
    "    popfq",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbx",
    "    pop rbp",
    "    ret",
);

extern "C" {
    fn enter_user(entry: u64, user_sp: u64) -> u64;
    fn user_return(code: u64) -> !;
    fn syscall_entry();
}

fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
             options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

fn wrmsr(msr: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
             options(nomem, nostack, preserves_flags));
    }
}

/// Enable `syscall`, point it at our entry stub, and give ring 0 its stack.
pub fn init() {
    unsafe {
        let top = addr_of_mut!(KSTACK) as u64 + KSTACK_SIZE as u64;
        KSTACK_TOP = top;
        tss::set_rsp0(top);
    }
    wrmsr(MSR_EFER, rdmsr(MSR_EFER) | EFER_SCE);
    // syscall loads CS from bits 47:32 and SS from +8. sysret loads CS from
    // bits 63:48 + 16 and SS from bits 63:48 + 8, each with RPL 3.
    let star = ((0x10u64) << 48) | ((KERNEL_CODE as u64) << 32);
    wrmsr(MSR_STAR, star);
    wrmsr(MSR_LSTAR, syscall_entry as *const () as u64);
    // Clear IF on entry, so syscalls run with interrupts off.
    wrmsr(MSR_SFMASK, RFLAGS_IF);
}

/// Run a program at `entry` on the user stack `user_sp`. Returns the exit
/// code when the program leaves through `exit` or a fault. Its address space
/// must already be active.
pub fn run_user(entry: u64, user_sp: u64) -> u64 {
    unsafe { enter_user(entry, user_sp) }
}

/// Leave the program and return from `run_user` with `code`.
pub fn leave(code: u64) -> ! {
    unsafe { user_return(code) }
}

/// The registers of a syscall, in the order the entry stub pushes them.
#[repr(C)]
pub struct Args {
    pub nr: u64,
    pub a: [u64; 6],
}

/// Dispatch a syscall from ring 3 to the Linux layer.
#[no_mangle]
extern "C" fn syscall_dispatch(args: *const Args) -> u64 {
    let args = unsafe { &*args };
    crate::linux::dispatch(args.nr, args.a)
}

/// Set the FS base, which musl uses for thread-local storage.
pub fn set_fs_base(base: u64) {
    wrmsr(MSR_FS_BASE, base);
}
