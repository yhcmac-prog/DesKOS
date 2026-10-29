//! PS/2 keyboard: IRQ1, scancode set 1, US layout, key event queue.

use crate::console::{inb, SpinLock};

const DATA: u16 = 0x60;

#[derive(Clone, Copy, PartialEq)]
pub struct KeyEvent {
    pub code: u8,      // ascii or key code
    pub pressed: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

pub const KEY_UP: u8 = 0x80;
pub const KEY_DOWN: u8 = 0x81;
pub const KEY_LEFT: u8 = 0x82;
pub const KEY_RIGHT: u8 = 0x83;
pub const KEY_F1: u8 = 0x90;
pub const KEY_F2: u8 = 0x91;

const QUEUE_SIZE: usize = 64;

pub struct KeyQueue {
    buf: [KeyEvent; QUEUE_SIZE],
    head: usize,
    tail: usize,
}

impl KeyQueue {
    const fn new() -> Self {
        KeyQueue {
            buf: [KeyEvent { code: 0, pressed: false, alt: false, ctrl: false, shift: false }; QUEUE_SIZE],
            head: 0,
            tail: 0,
        }
    }
    fn push(&mut self, ev: KeyEvent) {
        let next = (self.tail + 1) % QUEUE_SIZE;
        if next != self.head {
            self.buf[self.tail] = ev;
            self.tail = next;
        }
    }
    pub fn pop(&mut self) -> Option<KeyEvent> {
        if self.head == self.tail {
            None
        } else {
            let ev = self.buf[self.head];
            self.head = (self.head + 1) % QUEUE_SIZE;
            Some(ev)
        }
    }
}

pub static KEYS: SpinLock<KeyQueue> = SpinLock::new(KeyQueue::new());

static mut SHIFT: bool = false;
static mut CTRL: bool = false;
static mut ALT: bool = false;
static mut EXTENDED: bool = false;

const SCANCODE_ASCII: [u8; 58] = [
    0, 27, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b'-', b'=', 8, b'\t',
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0,
    b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'', b'`', 0, b'\\',
    b'z', b'x', b'c', b'v', b'b', b'n', b'm', b',', b'.', b'/', 0, b'*', 0, b' ',
];

const SCANCODE_ASCII_SHIFT: [u8; 58] = [
    0, 27, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')', b'_', b'+', 8, b'\t',
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0,
    b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', b'"', b'~', 0, b'|',
    b'Z', b'X', b'C', b'V', b'B', b'N', b'M', b'<', b'>', b'?', 0, b'*', 0, b' ',
];

pub fn init() {
    crate::pic::unmask(1);
}

pub fn irq_handler() {
    let sc = unsafe { inb(DATA) };
    handle_scancode(sc);
}

pub fn handle_scancode(sc: u8) {
    unsafe {
        if sc == 0xE0 {
            EXTENDED = true;
            return;
        }
        let pressed = sc & 0x80 == 0;
        let code = sc & 0x7F;
        let ext = EXTENDED;
        EXTENDED = false;

        match (code, ext) {
            (0x2A, false) | (0x36, false) => {
                SHIFT = pressed;
                return;
            }
            (0x1D, _) => {
                CTRL = pressed;
                return;
            }
            (0x38, _) => {
                ALT = pressed;
                return;
            }
            _ => {}
        }

        if !pressed {
            return; // key release: only modifiers tracked
        }

        let mut out: u8 = 0;
        if ext {
            out = match code {
                0x48 => KEY_UP,
                0x50 => KEY_DOWN,
                0x4B => KEY_LEFT,
                0x4D => KEY_RIGHT,
                _ => 0,
            };
        } else {
            match code {
                0x3B => out = KEY_F1,
                0x3C => out = KEY_F2,
                _ => {
                    if (code as usize) < 58 {
                        out = if SHIFT { SCANCODE_ASCII_SHIFT[code as usize] } else { SCANCODE_ASCII[code as usize] };
                    }
                }
            }
        }

        if out != 0 {
            let ev = KeyEvent { code: out, pressed: true, alt: ALT, ctrl: CTRL, shift: SHIFT };
            let (flags, q) = KEYS.lock_irq();
            q.push(ev);
            unsafe { KEYS.unlock_irq(flags) };
        }
    }
}

/// Pop a key event, None if queue empty.
pub fn poll() -> Option<KeyEvent> {
    let (flags, q) = KEYS.lock_irq();
    let ev = q.pop();
    unsafe { KEYS.unlock_irq(flags) };
    ev
}

/// Blocking read of one key.
pub fn getkey() -> KeyEvent {
    loop {
        if let Some(ev) = poll() {
            return ev;
        }
        unsafe { core::arch::asm!("sti; hlt") };
    }
}
