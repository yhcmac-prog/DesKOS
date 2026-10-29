//! PIT 8253/8254: 100Hz preemptive tick on IRQ0.

use crate::console::outb;
use crate::idt::IsrFrame;
use core::sync::atomic::{AtomicU64, Ordering};

const CHANNEL0: u16 = 0x40;
const CMD: u16 = 0x43;
const HZ: u32 = 100;

pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let div = 1193182 / HZ;
    unsafe {
        outb(CMD, 0x36);
        outb(CHANNEL0, (div & 0xFF) as u8);
        outb(CHANNEL0, (div >> 8) as u8);
    }
    crate::pic::unmask(0);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_ms() -> u64 {
    ticks() * (1000 / HZ as u64)
}

pub fn irq_handler(frame: &mut IsrFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    #[cfg(has_sched)]
    crate::sched::tick(frame);
    #[cfg(not(has_sched))]
    let _ = frame;
}
