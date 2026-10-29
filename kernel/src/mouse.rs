//! PS/2 mouse: IRQ12, 3-byte packet decoding, relative motion + buttons.

use crate::console::{inb, outb, SpinLock};

const CMD: u16 = 0x64;
const DATA: u16 = 0x60;

#[derive(Clone, Copy)]
pub struct MouseEvent {
    pub dx: i16,
    pub dy: i16,
    pub buttons: u8, // bit0 left, bit1 right, bit2 middle
}

pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
    pkt: [u8; 3],
    idx: usize,
    max_x: i32,
    max_y: i32,
    pub moved: bool,
    pub clicked: bool,   // left pressed edge
    pub rclicked: bool,  // right pressed edge
}

impl MouseState {
    const fn new() -> Self {
        MouseState { x: 0, y: 0, buttons: 0, pkt: [0; 3], idx: 0, max_x: 0, max_y: 0, moved: false, clicked: false, rclicked: false }
    }
}

pub static MOUSE: SpinLock<MouseState> = SpinLock::new(MouseState::new());

fn wait_write() {
    for _ in 0..100000 {
        if unsafe { inb(CMD) } & 2 == 0 {
            return;
        }
    }
}

fn wait_read() {
    for _ in 0..100000 {
        if unsafe { inb(CMD) } & 1 != 0 {
            return;
        }
    }
}

fn mouse_cmd(c: u8) {
    wait_write();
    unsafe { outb(CMD, 0xD4) };
    wait_write();
    unsafe { outb(DATA, c) };
    wait_read();
    unsafe { inb(DATA) }; // ACK
}

pub fn init(width: i32, height: i32) {
    unsafe {
        // enable aux device
        wait_write();
        outb(CMD, 0xA8);
        // enable IRQ12 in controller config
        wait_write();
        outb(CMD, 0x20);
        wait_read();
        let mut cfg = inb(DATA);
        cfg |= 2;
        cfg &= !0x20;
        wait_write();
        outb(CMD, 0x60);
        wait_write();
        outb(DATA, cfg);
    }
    mouse_cmd(0xF6); // defaults
    mouse_cmd(0xF4); // enable streaming

    let (flags, m) = MOUSE.lock_irq();
    m.x = width / 2;
    m.y = height / 2;
    m.max_x = width - 1;
    m.max_y = height - 1;
    unsafe { MOUSE.unlock_irq(flags) };
    crate::pic::unmask(12);
}

pub fn irq_handler() {
    let b = unsafe { inb(DATA) };
    let (flags, m) = MOUSE.lock_irq();
    m.pkt[m.idx] = b;
    m.idx += 1;
    if m.idx == 1 && b & 0x08 == 0 {
        m.idx = 0; // resync: bit3 of byte0 always 1
        unsafe { MOUSE.unlock_irq(flags) };
        return;
    }
    if m.idx < 3 {
        unsafe { MOUSE.unlock_irq(flags) };
        return;
    }
    m.idx = 0;
    let dx = m.pkt[1] as i8 as i16;
    let dy = m.pkt[2] as i8 as i16;
    let old_buttons = m.buttons;
    m.buttons = m.pkt[0] & 7;
    // pointer acceleration: 1x for small deltas (precise adjustments land
    // exactly), 2x for large ones (fast slides stay fast). A flat 2x made
    // every small correction overshoot by an extra block.
    let ax = if m.pkt[0] & 0x10 == 0 {
        dx as i32
    } else {
        -((256 - m.pkt[1] as i32).min(255))
    };
    let ay = if m.pkt[0] & 0x20 == 0 {
        -(dy as i32)
    } else {
        (256 - m.pkt[2] as i32).min(255)
    };
    m.x += if ax.abs() >= 4 { ax * 2 } else { ax };
    m.y += if ay.abs() >= 4 { ay * 2 } else { ay };
    // clamp to the configured screen
    if m.x < 0 { m.x = 0; }
    if m.y < 0 { m.y = 0; }
    if m.x > m.max_x { m.x = m.max_x; }
    if m.y > m.max_y { m.y = m.max_y; }
    if m.buttons & 1 != 0 && old_buttons & 1 == 0 {
        m.clicked = true;
    }
    if m.buttons & 2 != 0 && old_buttons & 2 == 0 {
        m.rclicked = true;
    }
    m.moved = true;
    unsafe { MOUSE.unlock_irq(flags) };
}

/// Poll mouse state from a normal (interruptible) context.
/// Returns (x, y, buttons, click: 0=none 1=left 2=right).
pub fn poll() -> (i32, i32, u8, u8) {
    let (flags, m) = MOUSE.lock_irq();
    let clicked = m.clicked;
    let rclicked = m.rclicked;
    m.clicked = false;
    m.rclicked = false;
    let out = (m.x, m.y, m.buttons, if clicked { 1 } else if rclicked { 2 } else { 0 });
    unsafe { MOUSE.unlock_irq(flags) };
    out
}
