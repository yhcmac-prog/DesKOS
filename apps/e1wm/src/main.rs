//! e1wm: tAbLeOS window manager (Rust port).
//!
//! Differences from the C version (e1LibreOS/src/fbui/e1wm.c):
//! - runs in ring3 via syscalls, not kernel mode
//! - removes VT switching, pinyin input, evdev, procfs
//! - keeps: windowing, traffic-light buttons, animations, bilingual CN/EN,
//!   PS/2 mouse cursor, 8x8/16px CJK font rendering

#![no_std]
#![no_main]

use libe1::*;
use core::panic::PanicInfo;

// generated ASCII font data (public domain, tools/mkfont_rs.py)
include!("font8x8.rs");

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    libe1::exit(1)
}

const MAX_WINS: usize = 16;
const ANIM_FRAMES: usize = 10;
const ANIM_MS: usize = 28;

// Window states
const WIN_OPEN: u8 = 1;
const WIN_CLOSING: u8 = 2;

struct Window {
    id: usize,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    title: [u8; 64],
    content: [u8; 2048],
    state: u8,
    anim_start: usize,
}

struct E1wm {
    fb: *mut u32,     // real framebuffer (uncacheable MMIO — blit target only)
    buf: *mut u32,    // back buffer in cached RAM — all drawing happens here
    fb_w: u32,
    fb_h: u32,
    fb_pitch: u32,
    wins: [Option<Window>; MAX_WINS],
    mouse_x: i32,
    mouse_y: i32,
    mouse_buttons: u8,
    last_clicked: usize,
    cur_patch: [u32; 121], // fb pixels under the cursor (11x11), for erase
    lang: u8, // 0=CN, 1=EN
    tick: usize,
}

static mut WM: Option<E1wm> = None;

impl E1wm {
    fn new() -> Self {
        let mut fbinfo = [0u32; 5];
        fb_info(&mut fbinfo);
        let (fb_phys, w, h, pitch, bpp) = (fbinfo[0], fbinfo[1], fbinfo[2], fbinfo[3], fbinfo[4]);
        // identity-map the fb into this process (RW+USER) before touching it
        if fb_map() != 0 {
            exit(1);
        }
        // back buffer in cached RAM: drawing straight into the uncacheable fb
        // makes every full-screen repaint take ~150ms and the display shows
        // torn half-drawn frames nearly all the time
        let bb = fb_backbuf();
        if bb < 0 {
            exit(1);
        }
        let fb = fb_phys as *mut u32;
        E1wm {
            fb,
            buf: bb as *mut u32,
            fb_w: w,
            fb_h: h,
            fb_pitch: pitch,
            wins: [const { None }; MAX_WINS],
            mouse_x: 0,
            mouse_y: 0,
            mouse_buttons: 0,
            last_clicked: 0,
            cur_patch: [0; 121],
            lang: 0,
            tick: 0,
        }
    }

    fn fb_write(&self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 || x >= self.fb_w as i32 || y >= self.fb_h as i32 {
            return;
        }
        unsafe {
            let off = (y as u32 * (self.fb_pitch / 4) + x as u32) as isize;
            *self.buf.offset(off) = color;
        }
    }

    /// Copy the finished back buffer to the framebuffer in one pass.
    /// The fb stride equals pitch/4 and the back buffer uses the same layout,
    /// so a flat u32 copy is enough.
    fn blit(&self) {
        let n = (self.fb_pitch / 4) * self.fb_h;
        unsafe {
            for i in 0..n as isize {
                self.fb.offset(i).write_volatile(self.buf.offset(i).read());
            }
        }
    }

    fn fill_rect(&self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                self.fb_write(x + dx, y + dy, color);
            }
        }
    }

    fn draw_rect(&self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h as i32 - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w as i32 - 1, y, 1, h, color);
    }

    fn draw_char_8x8(&self, x: i32, y: i32, c: char, color: u32) {
        let glyph = font8x8(c);
        for row in 0..8i32 {
            let bits = glyph[row as usize];
            for col in 0..8i32 {
                if bits & (1 << col) != 0 {
                    self.fb_write(x + col, y + row, color);
                }
            }
        }
    }

    fn draw_string(&self, x: i32, y: i32, s: &str, color: u32) {
        let mut cx = x;
        for c in s.chars() {
            self.draw_char_8x8(cx, y, c, color);
            cx += 8;
        }
    }

    fn draw_string_cjk(&self, x: i32, y: i32, s: &str, color: u32) {
        let mut cx = x;
        for c in s.chars() {
            if c.is_ascii() {
                self.draw_char_8x8(cx, y, c, color);
                cx += 8;
            } else {
                // 16x16 CJK
                let glyph = font16_cjk(c);
                for row in 0..16i32 {
                    let bits = glyph[row as usize];
                    for col in 0..16i32 {
                        if bits & (1 << (15 - col)) != 0 {
                            self.fb_write(cx + col, y + row, color);
                        }
                    }
                }
                cx += 16;
            }
        }
    }

    fn draw_desktop(&self) {
        self.fill_rect(0, 0, self.fb_w, self.fb_h, 0x2C001E); // dark purple
        // title bar
        self.fill_rect(0, 0, self.fb_w, 24, 0x5E2750);
        self.draw_string_cjk(8, 4, self.title(), 0xFFFFFF);
        // taskbar
        self.fill_rect(0, self.fb_h as i32 - 24, self.fb_w, 24, 0x3E3E3E);
    }

    fn title(&self) -> &'static str {
        // NOTE: brand must come from this crate's cfg, not libe1::BRAND_NAME —
        // `cargo rustc --` only passes --cfg to the final crate, so deps are
        // always compiled without the brand cfg
        #[cfg(brand = "desk")]
        { if self.lang == 0 { "DesKOS 桌面" } else { "DesKOS Desktop" } }
        #[cfg(not(brand = "desk"))]
        { if self.lang == 0 { "tAbLeOS 桌面" } else { "tAbLeOS Desktop" } }
    }

    fn draw_window(&self, idx: usize) {
        if let Some(ref w) = self.wins[idx] {
            if w.state == 0 {
                return;
            }
            // shadow
            self.fill_rect(w.x + 4, w.y + 4, w.w, w.h, 0x000000);
            // border
            self.draw_rect(w.x, w.y, w.w, w.h, 0x888888);
            // title bar
            self.fill_rect(w.x, w.y, w.w, 20, 0x5E2750);
            // traffic lights
            self.fill_rect(w.x + 4, w.y + 4, 8, 8, 0xFF5F57); // close
            self.fill_rect(w.x + 16, w.y + 4, 8, 8, 0xFEBC2E); // minimize
            self.fill_rect(w.x + 28, w.y + 4, 8, 8, 0x28C840); // maximize
            // title text
            let title = trim_nul(&w.title);
            self.draw_string(w.x + 40, w.y + 4, title, 0xFFFFFF);
            // content
            let content = trim_nul(&w.content);
            let mut cy = w.y + 24;
            for line in content.lines() {
                if cy + 8 > w.y + w.h as i32 - 8 {
                    break;
                }
                self.draw_string(w.x + 4, cy, line, 0xCCCCCC);
                cy += 10;
            }
        }
    }

    /// Copy the 11x11 cursor region from the back buffer to the fb.
    fn patch_blit(&self) {
        let stride = (self.fb_pitch / 4) as i32;
        for dy in -4..=4 {
            for dx in -4..=4 {
                let x = self.mouse_x + dx;
                let y = self.mouse_y + dy;
                if x >= 0 && y >= 0 && x < self.fb_w as i32 && y < self.fb_h as i32 {
                    let off = (y * stride + x) as isize;
                    unsafe { self.fb.offset(off).write_volatile(self.buf.offset(off).read()) };
                }
            }
        }
    }

    /// Save the clean scene pixels under the cursor from the back buffer.
    fn cursor_save(&mut self) {
        let stride = (self.fb_pitch / 4) as i32;
        let mut k = 0usize;
        for dy in -4..=4 {
            for dx in -4..=4 {
                let x = self.mouse_x + dx;
                let y = self.mouse_y + dy;
                self.cur_patch[k] = if x >= 0 && y >= 0 && x < self.fb_w as i32 && y < self.fb_h as i32 {
                    unsafe { self.buf.offset((y * stride + x) as isize).read() }
                } else {
                    0
                };
                k += 1;
            }
        }
    }

    /// Erase the cursor from the back buffer and push the restored scene
    /// region to the fb.
    fn cursor_restore(&self) {
        let stride = (self.fb_pitch / 4) as i32;
        let mut k = 0usize;
        for dy in -4..=4 {
            for dx in -4..=4 {
                let x = self.mouse_x + dx;
                let y = self.mouse_y + dy;
                if x >= 0 && y >= 0 && x < self.fb_w as i32 && y < self.fb_h as i32 {
                    unsafe { self.buf.offset((y * stride + x) as isize).write(self.cur_patch[k]) };
                }
                k += 1;
            }
        }
        self.patch_blit();
    }

    /// Draw the cursor into the back buffer and push the region to the fb.
    fn cursor_draw(&self) {
        // simple crosshair cursor
        for i in -4..5 {
            self.bb_put(self.mouse_x + i, self.mouse_y, 0xFFFFFF);
            self.bb_put(self.mouse_x, self.mouse_y + i, 0xFFFFFF);
        }
        self.patch_blit();
    }

    fn bb_put(&self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 || x >= self.fb_w as i32 || y >= self.fb_h as i32 {
            return;
        }
        let stride = (self.fb_pitch / 4) as i32;
        unsafe { self.buf.offset((y * stride + x) as isize).write(color) };
    }

    fn open_window(&mut self, title: &str, content: &str) -> usize {
        for i in 0..MAX_WINS {
            if self.wins[i].is_none() {
                let id = i;
                let mut t = [0u8; 64];
                let tlen = title.len().min(63);
                t[..tlen].copy_from_slice(&title.as_bytes()[..tlen]);
                let mut c = [0u8; 2048];
                let clen = content.len().min(2047);
                c[..clen].copy_from_slice(&content.as_bytes()[..clen]);
                let w = Window {
                    id,
                    x: 40 + (i as i32 % 4) * 30,
                    y: 40 + (i as i32 % 4) * 30,
                    w: 400,
                    h: 240,
                    title: t,
                    content: c,
                    state: WIN_OPEN,
                    anim_start: self.tick,
                };
                self.wins[i] = Some(w);
                return i;
            }
        }
        0
    }

    fn close_window(&mut self, idx: usize) {
        if let Some(ref mut w) = self.wins[idx] {
            w.state = WIN_CLOSING;
            w.anim_start = self.tick;
        }
    }

    fn animate(&mut self) -> bool {
        self.tick += 1;
        let mut removed = false;
        for i in 0..MAX_WINS {
            if let Some(ref mut w) = self.wins[i] {
                if w.state == WIN_CLOSING {
                    let elapsed = self.tick - w.anim_start;
                    if elapsed >= ANIM_FRAMES * ANIM_MS / 10 {
                        self.wins[i] = None;
                        removed = true;
                        continue;
                    }
                    let shrink = elapsed * w.h as usize / (ANIM_FRAMES * ANIM_MS / 10);
                    let _ = shrink; // animation visual effect not fully implemented
                }
            }
        }
        removed
    }

    /// Poll mouse state. Moves the cursor incrementally (erase old patch,
    /// draw new); returns true if the scene changed (click closed a window)
    /// and a full redraw is needed.
    fn poll_mouse(&mut self) -> bool {
        let mut buf = [0i32; 3];
        let ev = mouse_poll(&mut buf);
        let (nx, ny) = (buf[0], buf[1]);
        if nx != self.mouse_x || ny != self.mouse_y {
            self.cursor_restore();
            self.mouse_x = nx;
            self.mouse_y = ny;
            self.cursor_save();
            self.cursor_draw();
        }
        self.mouse_buttons = buf[2] as u8;

        let mut changed = false;
        if ev == 1 {
            // left click
            for i in 0..MAX_WINS {
                if let Some(ref w) = self.wins[i] {
                    if self.mouse_x >= w.x && self.mouse_x < w.x + w.w as i32
                        && self.mouse_y >= w.y && self.mouse_y < w.y + w.h as i32
                    {
                        // check traffic light
                        if self.mouse_y < w.y + 16 {
                            let lx = self.mouse_x - w.x;
                            if lx >= 4 && lx < 12 {
                                self.close_window(i);
                                changed = true;
                            } else if lx >= 16 && lx < 24 {
                                // minimize (v1: no-op)
                            } else if lx >= 28 && lx < 36 {
                                // maximize (v1: no-op)
                            }
                        }
                        self.last_clicked = i;
                        break;
                    }
                }
            }
        } else if ev == 2 {
            // right click
            for i in 0..MAX_WINS {
                if let Some(ref w) = self.wins[i] {
                    if self.mouse_x >= w.x && self.mouse_x < w.x + w.w as i32
                        && self.mouse_y >= w.y && self.mouse_y < w.y + w.h as i32
                    {
                        self.close_window(i);
                        changed = true;
                        break;
                    }
                }
            }
        }
        changed
    }

    /// Poll one keyboard event; returns true if the scene changed.
    fn poll_key(&mut self) -> bool {
        let mut buf = [0u8; 5];
        if getkey(&mut buf) > 0 {
            let key = buf[0];
            let alt = buf[2];
            if alt == 1 && key == b'l' {
                self.lang = 1 - self.lang;
                return true;
            } else if alt == 1 && key == b'q' {
                // quit: close all windows
                for i in 0..MAX_WINS {
                    if self.wins[i].is_some() {
                        self.close_window(i);
                    }
                }
                return true;
            } else if alt == 1 && key == b'i' {
                self.open_window("Install", "rit install hello.rit");
                return true;
            } else if alt == 1 && key == b'1' {
                self.open_window("Terminal", "ush v0.1\nType 'exit' to quit");
                return true;
            }
        }
        false
    }

    /// One full frame: redraw the scene into the back buffer (with the
    /// cursor) and blit. Expensive (~150ms UC fb writes) — call only when
    /// the scene actually changed.
    fn full_frame(&mut self) {
        self.draw_desktop();
        for i in 0..MAX_WINS {
            self.draw_window(i);
        }
        self.cursor_save();
        self.cursor_draw();
        self.blit();
    }

    #[cfg(brand = "desk")]
    const WELCOME: &str = "Welcome to DesKOS!\nOpen source under the MIT License\n\nAlt+1: terminal\nAlt+I: install\nAlt+Q: quit\nAlt+L: toggle CN/EN";
    #[cfg(not(brand = "desk"))]
    const WELCOME: &str = "Welcome to tAbLeOS!\nClosed source, all rights reserved\n\nAlt+1: terminal\nAlt+I: install\nAlt+Q: quit\nAlt+L: toggle CN/EN";

    fn run(&mut self) -> ! {
        self.open_window("Welcome", Self::WELCOME);
        // sync the cursor with the kernel-tracked position before first draw
        let mut mb = [0i32; 3];
        mouse_poll(&mut mb);
        self.mouse_x = mb[0];
        self.mouse_y = mb[1];
        self.full_frame();
        loop {
            // event-driven: mouse movement is handled incrementally inside
            // poll_mouse (11x11 patch, ~instant) and must NOT trigger the
            // full redraw — a full blit freezes cursor updates for its
            // whole duration, which made the pointer feel sluggish
            if self.poll_key() | self.poll_mouse() | self.animate() {
                self.full_frame();
            }
            yield_cpu();
        }
    }
}

// ---------------------------------------------------------------------------
// Fonts (generated by tools/mkfont_rs.py from e1LibreOS C headers)
// ---------------------------------------------------------------------------
fn trim_nul(buf: &[u8]) -> &str {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..end]).unwrap_or("")
}

fn font8x8(c: char) -> [u8; 8] {
    let idx = c as usize;
    if idx >= 128 {
        return [0; 8];
    }
    FONT8[idx]
}

/// cjk.bin layout: 9826 * u16-LE codepoints (ascending) + 9826 * 32 bytes of
/// glyph rows (row = [left_half, right_half], MSB = leftmost pixel).
fn font16_cjk(c: char) -> [u16; 16] {
    let cp = c as u32;
    if cp > 0xFFFF {
        return [0; 16];
    }
    const DATA: &[u8] = include_bytes!("cjk.bin");
    const N: usize = DATA.len() / 34;
    let idx = |i: usize| u16::from_le_bytes([DATA[i * 2], DATA[i * 2 + 1]]) as u32;
    let mut lo = 0usize;
    let mut hi = N;
    while lo < hi {
        let mid = (lo + hi) / 2;
        let v = idx(mid);
        if v == cp {
            let base = N * 2 + mid * 32;
            let mut out = [0u16; 16];
            for r in 0..16 {
                let b0 = DATA[base + r * 2] as u16;
                let b1 = DATA[base + r * 2 + 1] as u16;
                out[r] = (b0 << 8) | b1;
            }
            return out;
        }
        if v < cp {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    [0; 16]
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        WM = Some(E1wm::new());
        WM.as_mut().unwrap().run();
    }
    exit(0)
}
