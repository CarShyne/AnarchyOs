use alloc::boxed::Box;
use core::arch::global_asm;
use core::ptr::addr_of_mut;

const MAX_TASKS: usize = 8;
const STACK_SIZE: usize = 32 * 1024;

global_asm!(
    ".global task_switch",
    "task_switch:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
    ".global task_start",
    "task_start:",
    "and rsp, -16",
    "sti",
    "call rbx",
    "2:",
    "hlt",
    "jmp 2b",
);

extern "C" {
    fn task_switch(save: *mut u64, next: u64);
    fn task_start();
}

#[derive(Clone, Copy)]
struct Task {
    rsp: u64,
}

struct Sched {
    tasks: [Task; MAX_TASKS],
    count: usize,
    current: usize,
}

static mut SCHED: Sched = Sched {
    tasks: [Task { rsp: 0 }; MAX_TASKS],
    count: 0,
    current: 0,
};

fn sched() -> &'static mut Sched {
    unsafe { &mut *addr_of_mut!(SCHED) }
}

pub fn init() {
    let s = sched();
    s.count = 1;
    s.current = 0;
}

pub fn spawn(entry: extern "C" fn() -> !, name: &'static str) {
    let s = sched();
    if s.count == MAX_TASKS {
        kprintln!("sched: task table full, cannot spawn {}", name);
        return;
    }

    let stack: &'static mut [u8] =
        Box::leak(alloc::vec![0u8; STACK_SIZE].into_boxed_slice());
    let top = (stack.as_mut_ptr() as u64 + STACK_SIZE as u64) & !0xf;
    let ret_slot = top - 16;
    let sp = ret_slot - 48;

    unsafe {
        let p = sp as *mut u64;
        p.add(0).write(0);
        p.add(1).write(0);
        p.add(2).write(0);
        p.add(3).write(0);
        p.add(4).write(entry as *const () as u64);
        p.add(5).write(0);
        (ret_slot as *mut u64).write(task_start as *const () as u64);
    }

    s.tasks[s.count] = Task { rsp: sp };
    s.count += 1;
    kprintln!("sched: spawned {} as task {}", name, s.count - 1);
}

pub fn preempt() {
    let s = sched();
    if s.count < 2 {
        return;
    }
    let old = s.current;
    let next = (old + 1) % s.count;
    s.current = next;
    let save = addr_of_mut!(s.tasks[old].rsp);
    let target = s.tasks[next].rsp;
    unsafe { task_switch(save, target) };
}
