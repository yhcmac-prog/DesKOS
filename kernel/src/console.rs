//! Framebuffer glyph console + VGA text (0xB8000) + UART 16550 (COM1) triple output.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::font::FONT8X8;

const VGA_BUF: usize = 0xB8000;
const VGA_W: usize = 80;
const VGA_H: usize = 25;

const COM1: u16 = 0x3F8;

// Character cell size on the framebuffer.  Each 8×8 font glyph is drawn
// as a 2×2 pixel block, so an 8×8 glyph occupies a 16×16 cell — readable
// in a VBox window even when the host display scales the VM view.
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;
const SCALE: usize = 2;
const CELL_W: usize = GLYPH_W * SCALE;   // 16
const CELL_H: usize = GLYPH_H * SCALE;   // 16

// ---------------------------------------------------------------------------
// Minimal spin lock
// ---------------------------------------------------------------------------
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: core::cell::UnsafeCell<T>,
}
unsafe impl<T> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(v: T) -> Self {
        SpinLock { locked: AtomicBool::new(false), data: core::cell::UnsafeCell::new(v) }
    }
    pub fn lock(&self) -> &mut T {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        unsafe { &mut *self.data.get() }
    }
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
    /// Lock with interrupts disabled. Returns the saved RFLAGS; pass it to
    /// `unlock_irq`. Required when the same lock is taken from an IRQ handler.
    pub fn lock_irq(&self) -> (u64, &mut T) {
        let mut flags: u64;
        unsafe {
            core::arch::asm!(
                "pushfq",
                "pop {0}",
                "cli",
                out(reg) flags, options(nomem)
            );
        }
        let d = self.lock();
        (flags, d)
    }
    /// # Safety
    /// `flags` must come from the matching `lock_irq` on the same lock.
    pub unsafe fn unlock_irq(&self, flags: u64) {
        self.unlock();
        core::arch::asm!("push {0}", "popfq", in(reg) flags, options(nomem));
    }
}

// ---------------------------------------------------------------------------
// Port I/O
// ---------------------------------------------------------------------------
pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}
pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack));
    v
}

pub unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

// ---------------------------------------------------------------------------
// UART byte I/O (COM1)
// ---------------------------------------------------------------------------
pub fn uart_putc(c: u8) {
    unsafe {
        while inb(COM1 + 5) & 0x20 == 0 {}
        outb(COM1, c);
    }
}

pub fn uart_getc() -> Option<u8> {
    unsafe {
        if inb(COM1 + 5) & 1 != 0 {
            Some(inb(COM1))
        } else {
            None
        }
    }
}

pub fn uart_getc_blocking() -> u8 {
    let mut flags: u64;
    unsafe { core::arch::asm!("pushfq", "pop {0}", out(reg) flags, options(nomem)) };
    let c = loop {
        if let Some(c) = uart_getc() {
            break c;
        }
        unsafe { core::arch::asm!("sti", "hlt", options(nomem, nostack)) };
    };
    unsafe { core::arch::asm!("push {0}", "popfq", in(reg) flags, options(nomem)) };
    c
}

// ---------------------------------------------------------------------------
// Console state
// ---------------------------------------------------------------------------
pub struct Console {
    color: u8,

    // framebuffer glyph console state (SCALE×GLYPH cell size)
    fb_col: usize,
    fb_row: usize,
    fb_ptr: *mut u8,
    fb_w: usize,
    fb_h: usize,
    fb_pitch: usize,    // bytes per scanline
    fb_bpp: usize,
    fb_valid: bool,
    fb_cols: usize,     // char cols = fb_w / CELL_W
    fb_rows: usize,     // char rows = fb_h / CELL_H

    // VGA text console state (80×25 text mode)
    vga_col: usize,
    vga_row: usize,
}

// zero-nullptr is fine — fb_valid gates all FB access
unsafe impl Sync for Console {}
unsafe impl Send for Console {}

pub static CONSOLE: SpinLock<Console> = SpinLock::new(Console {
    color: 0x0F,
    fb_col: 0, fb_row: 0,
    fb_ptr: core::ptr::null_mut(),
    fb_w: 0, fb_h: 0, fb_pitch: 0, fb_bpp: 0,
    fb_valid: false, fb_cols: 0, fb_rows: 0,
    vga_col: 0, vga_row: 0,
});

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1 + 0, 0x01);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }

    let bi = crate::bootinfo::get();
    let fb_bytes = (bi.fb_width as usize) * (bi.fb_height as usize) * (bi.fb_bpp as usize / 8);
    let fb_valid = fb_bytes > 0 && bi.fb_phys != 0;
    let fb_cols = (bi.fb_width as usize) / CELL_W;
    let fb_rows = (bi.fb_height as usize) / CELL_H;

    let c = CONSOLE.lock();
    *c = Console {
        color: 0x0F,
        fb_col: 0, fb_row: 0,
        fb_ptr: bi.fb_phys as *mut u8,
        fb_w: bi.fb_width as usize,
        fb_h: bi.fb_height as usize,
        fb_pitch: bi.fb_pitch as usize,
        fb_bpp: bi.fb_bpp as usize,
        fb_valid,
        fb_cols,
        fb_rows,
        vga_col: 0, vga_row: 0,
    };
    c.clear();
    CONSOLE.unlock();
}

// ---------------------------------------------------------------------------
// FB glyph drawing helpers
// ---------------------------------------------------------------------------
impl Console {
    fn fb_write_pixel(&self, x: usize, y: usize, rgb: (u8, u8, u8)) {
        if !self.fb_valid { return; }
        if x >= self.fb_w || y >= self.fb_h { return; }
        let offset = y * self.fb_pitch + x * (self.fb_bpp / 8);
        unsafe {
            // Switch to kernel page table for framebuffer writes.
            // Framebuffer physical memory is fully identity-mapped only in
            // KERNEL_PML4; user pml4s only map the low 4GB which may have
            // different attributes (missing PCD for MMIO regions, etc.).
            // Safe because CONSOLE.lock_irq() held IRQs off on entry.
            let cur_cr3 = crate::vmm::current_cr3();
            let need_switch = cur_cr3 != crate::vmm::KERNEL_PML4;
            if need_switch {
                crate::vmm::load_cr3(crate::vmm::KERNEL_PML4);
            }
            let p = self.fb_ptr.add(offset);
            match self.fb_bpp {
                32 => {
                    p.write_volatile(rgb.2);
                    p.add(1).write_volatile(rgb.1);
                    p.add(2).write_volatile(rgb.0);
                    p.add(3).write_volatile(0xFF);
                }
                24 => {
                    p.write_volatile(rgb.2);
                    p.add(1).write_volatile(rgb.1);
                    p.add(2).write_volatile(rgb.0);
                }
                16 => {
                    let v: u16 = ((rgb.0 as u16 & 0xF8) << 8)
                               | ((rgb.1 as u16 & 0xFC) << 3)
                               | ((rgb.2 as u16 & 0xF8) >> 3);
                    p.cast::<u16>().write_volatile(v.to_le());
                }
                _ => {}
            }
            if need_switch {
                crate::vmm::load_cr3(cur_cr3);
            }
        }
    }

    fn fb_clear_cell(&self, col: usize, row: usize) {
        let x0 = col * CELL_W;
        let y0 = row * CELL_H;
        for y in 0..CELL_H {
            for x in 0..CELL_W {
                self.fb_write_pixel(x0 + x, y0 + y, (0, 0, 0));
            }
        }
    }

    fn fb_scroll(&self) {
        if !self.fb_valid { return; }
        let bytes_per_row = CELL_H * self.fb_pitch;
        let cur_cr3 = crate::vmm::current_cr3();
        let need_switch = cur_cr3 != crate::vmm::KERNEL_PML4;
        if need_switch {
            crate::vmm::load_cr3(crate::vmm::KERNEL_PML4);
        }
        unsafe {
            let dst = self.fb_ptr;
            let src = self.fb_ptr.add(bytes_per_row);
            let total = (self.fb_rows - 1) * bytes_per_row;
            core::ptr::copy_nonoverlapping(src, dst, total);
            let bot = self.fb_ptr.add((self.fb_rows - 1) * bytes_per_row);
            core::ptr::write_bytes(bot, 0, bytes_per_row);
        }
        if need_switch {
            crate::vmm::load_cr3(cur_cr3);
        }
    }

    fn fb_put_glyph(&self, col: usize, row: usize, ch: u8) {
        if !self.fb_valid { return; }
        let glyph_idx = ch as usize;
        let glyph = if glyph_idx < FONT8X8.len() {
            &FONT8X8[glyph_idx]
        } else {
            &FONT8X8[0]
        };
        let px = col * CELL_W;
        let py = row * CELL_H;
        for gy in 0..GLYPH_H {
            let row_byte = glyph[gy];
            for gx in 0..GLYPH_W {
                let bit = (row_byte >> (7 - gx)) & 1;
                let color = if bit != 0 { (0xFF, 0xFF, 0xFF) } else { (0, 0, 0) };
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        self.fb_write_pixel(px + gx * SCALE + dx,
                                            py + gy * SCALE + dy,
                                            color);
                    }
                }
            }
        }
    }

    fn put_fb(&mut self, ch: u8) {
        if !self.fb_valid { return; }
        match ch {
            b'\n' => {
                self.fb_col = 0;
                self.fb_row += 1;
            }
            b'\r' => self.fb_col = 0,
            8 => {
                if self.fb_col > 0 {
                    self.fb_col -= 1;
                    self.fb_clear_cell(self.fb_col, self.fb_row);
                }
            }
            _ => {
                self.fb_put_glyph(self.fb_col, self.fb_row, ch);
                self.fb_col += 1;
                if self.fb_col >= self.fb_cols {
                    self.fb_col = 0;
                    self.fb_row += 1;
                }
            }
        }
        if self.fb_row >= self.fb_rows {
            self.fb_scroll();
            self.fb_row = self.fb_rows - 1;
        }
    }

    fn put_vga(&mut self, ch: u8) {
        match ch {
            b'\n' => { self.vga_col = 0; self.vga_row += 1; }
            b'\r' => self.vga_col = 0,
            8 => {
                if self.vga_col > 0 {
                    self.vga_col -= 1;
                    let vga = VGA_BUF as *mut u16;
                    unsafe { vga.add(self.vga_row * VGA_W + self.vga_col).write_volatile((0x0F00u16) | b' ' as u16) };
                }
            }
            _ => {
                let vga = VGA_BUF as *mut u16;
                let cell = ((self.color as u16) << 8) | ch as u16;
                unsafe { vga.add(self.vga_row * VGA_W + self.vga_col).write_volatile(cell) };
                self.vga_col += 1;
                if self.vga_col >= VGA_W { self.vga_col = 0; self.vga_row += 1; }
            }
        }
        if self.vga_row >= VGA_H { self.scroll(); }
    }

    fn scroll(&mut self) {
        let vga = VGA_BUF as *mut u16;
        unsafe {
            core::ptr::copy(vga.add(VGA_W), vga, VGA_W * (VGA_H - 1));
            let blank = (0x0F00u16) | b' ' as u16;
            for i in 0..VGA_W {
                vga.add(VGA_W * (VGA_H - 1) + i).write_volatile(blank);
            }
        }
        self.vga_row = VGA_H - 1;
    }

    fn put_uart(&self, ch: u8) {
        unsafe {
            while inb(COM1 + 5) & 0x20 == 0 {}
            outb(COM1, ch);
            if ch == b'\n' {
                while inb(COM1 + 5) & 0x20 == 0 {}
                outb(COM1, b'\r');
            }
        }
    }

    pub fn put_byte(&mut self, ch: u8) {
        self.put_fb(ch);
        self.put_vga(ch);
        self.put_uart(ch);
    }

    pub fn clear(&mut self) {
        // VGA clear
        let vga = VGA_BUF as *mut u16;
        let blank = (0x0F00u16) | b' ' as u16;
        for i in 0..(VGA_W * VGA_H) {
            unsafe { vga.add(i).write_volatile(blank) };
        }
        // FB clear to black
        if self.fb_valid {
            unsafe { core::ptr::write_bytes(self.fb_ptr, 0, self.fb_pitch * self.fb_h); }
        }
        self.fb_col = 0;
        self.fb_row = 0;
        self.vga_col = 0;
        self.vga_row = 0;
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            self.put_byte(b);
        }
        Ok(())
    }
}

pub struct Printer;
impl Write for Printer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let (flags, c) = CONSOLE.lock_irq();
        let r = c.write_str(s);
        unsafe { CONSOLE.unlock_irq(flags) };
        r
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let mut p = $crate::console::Printer;
        let _ = write!(p, $($arg)*);
    });
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ({
        $crate::print!("{}\n", format_args!($($arg)*));
    });
}
