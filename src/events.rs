use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicUsize, Ordering};

const CAP: usize = 64;

#[derive(Clone, Copy)]
pub enum Event {
    Empty,
    Key(u8),
    Mouse { dx: i32, dy: i32, buttons: u8 },
    Tick(u64),
}

static mut QUEUE: [Event; CAP] = [Event::Empty; CAP];
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicUsize = AtomicUsize::new(0);

pub fn push(ev: Event) {
    let head = HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % CAP;
    if next == TAIL.load(Ordering::Acquire) {
        let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n % 100 == 0 {
            kprintln!("events: queue full, dropped {} so far", n);
        }
        return;
    }
    unsafe { (*addr_of_mut!(QUEUE))[head] = ev };
    HEAD.store(next, Ordering::Release);
}

pub fn pop() -> Option<Event> {
    let tail = TAIL.load(Ordering::Relaxed);
    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }
    let ev = unsafe { (*addr_of!(QUEUE))[tail] };
    TAIL.store((tail + 1) % CAP, Ordering::Release);
    Some(ev)
}
