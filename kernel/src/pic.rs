//! 8259 PIC: remap to 0x20/0x28 and mask management.

use crate::console::{inb, outb};

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

pub fn init() {
    unsafe {
        let m1 = inb(PIC1_DATA);
        let m2 = inb(PIC2_DATA);
        outb(PIC1_CMD, 0x11);
        outb(PIC2_CMD, 0x11);
        outb(PIC1_DATA, 0x20); // master offset 32
        outb(PIC2_DATA, 0x28); // slave offset 40
        outb(PIC1_DATA, 4);
        outb(PIC2_DATA, 2);
        outb(PIC1_DATA, 0x01);
        outb(PIC2_DATA, 0x01);
        outb(PIC1_DATA, m1);
        outb(PIC2_DATA, m2);
        // mask all except cascade; drivers unmask their IRQ
        set_mask(0xFFFF & !(1 << 2));
    }
}

pub fn set_mask(mask: u16) {
    unsafe {
        outb(PIC1_DATA, (mask & 0xFF) as u8);
        outb(PIC2_DATA, (mask >> 8) as u8);
    }
}

pub fn get_mask() -> u16 {
    unsafe { inb(PIC1_DATA) as u16 | ((inb(PIC2_DATA) as u16) << 8) }
}

pub fn unmask(irq: u8) {
    set_mask(get_mask() & !(1 << irq));
}

pub fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_CMD, 0x20);
        }
        outb(PIC1_CMD, 0x20);
    }
}
