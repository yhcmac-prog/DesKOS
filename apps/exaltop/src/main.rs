//! exaltop: Gershwin-inspired universal desktop for tAbLeOS / DesKOS.
//!
//! Visual language differs from e1wm (purple-red, taskbar): dark-blue
//! desktop, a 24px top menu bar (exalTOP logo + Session menu + uptime) and
//! a 48px bottom dock with three launcher icons — closer to macOS.
//! Shares the proven e1wm engine: double-buffered drawing into cached RAM
//! with a single blit per frame, incremental 11x11 cursor patching.

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

// Window states
const WIN_OPEN: u8 = 1;
const WIN_CLOSING: u8 = 2;

// palette
const DESKTOP_BG: u32 = 0x1E3A5F; // dark blue
const MENUBAR_BG: u32 = 0x2A2A2A;
const DOCK_BG: u32 = 0x666668;
const TITLE_BG: u32 = 0x3A3A3C;

const MENUBAR_H: i32 = 24;
const DOCK_H: i32 = 48;
const ICON: i32 = 32;

// dock icon slots (x origins), icons 32px wide with 24px gaps
const DOCK_X: [i32; 3] = [16, 72, 128];

struct Window {
    id: usize,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    title: [u8; 64],
    content: [u8; 2048],
    state: u8,
}

struct Exaltop {
    fb: *mut u32,  // real framebuffer (uncacheable MMIO — blit target only)
    buf: *mut u32, // back buffer in cached RAM — all drawing happens here
    fb_w: u32,
    fb_h: u32,
    fb_pitch: u32,
    wins: [Option<Window>; MAX_WINS],
    mouse_x: i32,
    mouse_y: i32,
    mouse_buttons: u8,
    cur_patch: [u32; 121], // fb pixels under the cursor (11x11), for erase
    lang: u8,              // 0=CN, 1=EN
    menu_open: bool,
}

static mut TOP: Option<Exaltop> = None;

impl Exaltop {
    fn new() -> Self {
        let mut fbinfo = [0u32; 5];
        fb_info(&mut fbinfo);
        let (fb_phys, w, h, pitch, _bpp) = (fbinfo[0], fbinfo[1], fbinfo[2], fbinfo[3], fbinfo[4]);
        if fb_map() != 0 {
            exit(1);
        }
        let bb = fb_backbuf();
        if bb < 0 {
            exit(1);
        }
        let fb = fb_phys as *mut u32;
        Exaltop {
            fb,
            buf: bb as *mut u32,
            fb_w: w,
            fb_h: h,
            fb_pitch: pitch,
            wins: [const { None }; MAX_WINS],
            mouse_x: 0,
            mouse_y: 0,
            mouse_buttons: 0,
            cur_patch: [0; 121],
            lang: 0,
            menu_open: false,
        }
    }

    // ------------------------------------------------------------- drawing

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

    /// Fake bold: draw twice with a 1px offset (menu bar logo).
    fn draw_string_bold(&self, x: i32, y: i32, s: &str, color: u32) {
        self.draw_string(x, y, s, color);
        self.draw_string(x + 1, y, s, color);
    }

    fn draw_string_cjk(&self, x: i32, y: i32, s: &str, color: u32) {
        let mut cx = x;
        for c in s.chars() {
            if c.is_ascii() {
                self.draw_char_8x8(cx, y, c, color);
                cx += 8;
            } else {
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

    // ------------------------------------------------------------- scene

    fn uptime_str(&self) -> [u8; 16] {
        // "up NNNs" — fixed width buffer, returns written prefix
        let ms = gettimeofday();
        let s = ms / 1000;
        let mut out = [0u8; 16];
        let mut q = 0usize;
        for &b in b"up " {
            out[q] = b;
            q += 1;
        }
        if s == 0 {
            out[q] = b'0';
            q += 1;
        } else {
            let mut tmp = [0u8; 12];
            let mut n = 0usize;
            let mut v = s;
            while v > 0 {
                tmp[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                out[q] = tmp[n];
                q += 1;
            }
        }
        out[q] = b's';
        q += 1;
        out[q] = 0;
        out
    }

    fn draw_menubar(&self) {
        let h = self.fb_h as i32;
        self.fill_rect(0, 0, self.fb_w, MENUBAR_H as u32, MENUBAR_BG);
        // logo (fake bold) + Session menu
        self.draw_string_bold(8, 8, "exalTOP", 0xFFFFFF);
        self.draw_string(76, 8, "Session", 0xE8E8E8);
        // uptime at the right edge
        let up = self.uptime_str();
        let mut len = 0usize;
        while len < up.len() && up[len] != 0 {
            len += 1;
        }
        self.draw_string(self.fb_w as i32 - 8 - (len * 8) as i32, 8, core::str::from_utf8(&up[..len]).unwrap_or(""), 0xC0C0C0);
        let _ = h;
    }

    fn draw_dock(&self) {
        let dy = self.fb_h as i32 - DOCK_H;
        self.fill_rect(0, dy, self.fb_w, DOCK_H as u32, DOCK_BG);
        // three 32px icon tiles with white labels
        for k in 0..3 {
            let x = DOCK_X[k];
            let base = match k {
                0 => 0x101418u32, // terminal: near-black
                1 => 0x1E6FD9,    // files: blue
                _ => 0x8A8A8E,    // about: gray
            };
            self.fill_rect(x, dy + 3, ICON as u32, ICON as u32, base);
            self.draw_rect(x, dy + 3, ICON as u32, ICON as u32, 0xDDDDDD);
            if k == 0 {
                // ">_" prompt
                self.draw_string(x + 5, dy + 13, ">_", 0x3AFF6E);
            } else if k == 1 {
                // folder tab
                self.fill_rect(x + 6, dy + 10, 10, 3, 0x9FCBFF);
                self.fill_rect(x + 6, dy + 13, 20, 12, 0x9FCBFF);
            } else {
                // "i"
                self.fill_rect(x + 15, dy + 9, 2, 2, 0xFFFFFF);
                self.fill_rect(x + 15, dy + 13, 2, 11, 0xFFFFFF);
            }
        }
        let labels = ["Terminal", "Files", "About"];
        for k in 0..3 {
            let x = DOCK_X[k];
            self.draw_string(x - 4, dy + 38, labels[k], 0xFFFFFF);
        }
    }

    fn draw_desktop(&self) {
        self.fill_rect(0, 0, self.fb_w, self.fb_h, DESKTOP_BG);
        self.draw_menubar();
        self.draw_dock();
    }

    #[cfg(brand = "desk")]
    fn brand_greeting(&self) -> &'static str {
        if self.lang == 0 { "DesKOS 桌面" } else { "DesKOS Desktop" }
    }
    #[cfg(not(brand = "desk"))]
    fn brand_greeting(&self) -> &'static str {
        if self.lang == 0 { "tAbLeOS 桌面" } else { "tAbLeOS Desktop" }
    }

    #[cfg(brand = "desk")]
    const WELCOME: &str = "Welcome to exalTOP on DesKOS!\nOpen source under the MIT License\n\nDock: Terminal / Files / About\nAlt+Q: quit\nAlt+L: toggle CN/EN";
    #[cfg(not(brand = "desk"))]
    const WELCOME: &str = "Welcome to exalTOP on tAbLeOS!\nClosed source, all rights reserved\n\nDock: Terminal / Files / About\nAlt+Q: quit\nAlt+L: toggle CN/EN";

    #[cfg(brand = "desk")]
    const ABOUT: &str = "exalTOP 0.1\nA Gershwin-inspired desktop\nfor DesKOS\n\nMIT License applies to DesKOS";
    #[cfg(not(brand = "desk"))]
    const ABOUT: &str = "exalTOP 0.1\nA Gershwin-inspired desktop\nfor tAbLeOS\n\ntAbLeOS is closed source";

    fn draw_window(&self, idx: usize) {
        if let Some(ref w) = self.wins[idx] {
            if w.state == 0 {
                return;
            }
            self.fill_rect(w.x + 4, w.y + 4, w.w, w.h, 0x000000); // shadow
            self.draw_rect(w.x, w.y, w.w, w.h, 0x888888);
            self.fill_rect(w.x, w.y, w.w, 20, TITLE_BG);
            // traffic lights
            self.fill_rect(w.x + 4, w.y + 4, 8, 8, 0xFF5F57);
            self.fill_rect(w.x + 16, w.y + 4, 8, 8, 0xFEBC2E);
            self.fill_rect(w.x + 28, w.y + 4, 8, 8, 0x28C840);
            let title = trim_nul(&w.title);
            self.draw_string(w.x + 40, w.y + 4, title, 0xFFFFFF);
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

    fn draw_session_menu(&self) {
        if !self.menu_open {
            return;
        }
        // dropdown under the Session label
        let (x, y, w, h) = (72, MENUBAR_H, 96, 34);
        self.fill_rect(x + 2, y + 2, w, h, 0x000000); // shadow
        self.fill_rect(x, y, w, h, 0xF2F2F2);
        self.draw_rect(x, y, w, h, 0x999999);
        self.draw_string(x + 8, y + 5, "About", 0x111111);
        self.draw_string(x + 8, y + 20, "Log Out", 0x111111);
    }

    // ------------------------------------------------------------- cursor

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

    // ------------------------------------------------------------- windows

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
                // keep windows below the menu bar
                let w = Window {
                    id,
                    x: 60 + (i as i32 % 4) * 30,
                    y: MENUBAR_H + 20 + (i as i32 % 4) * 30,
                    w: 400,
                    h: 240,
                    title: t,
                    content: c,
                    state: WIN_OPEN,
                };
                self.wins[i] = Some(w);
                return i;
            }
        }
        0
    }

    fn close_window(&mut self, idx: usize) {
        self.wins[idx] = None;
    }

    // ------------------------------------------------------------- input

    /// Poll mouse state. Moves the cursor incrementally; returns true if the
    /// scene changed (menu toggled / window closed) and a full redraw is due.
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
            let (mx, my) = (self.mouse_x, self.mouse_y);
            // 1. menu bar: Session toggles the dropdown
            if my < MENUBAR_H && mx >= 72 && mx < 128 {
                self.menu_open = !self.menu_open;
                return true;
            }
            // 2. dropdown items
            if self.menu_open && mx >= 72 && mx < 168 && my >= MENUBAR_H && my < MENUBAR_H + 34 {
                self.menu_open = false;
                if my < MENUBAR_H + 17 {
                    self.open_window("About", Self::ABOUT);
                } else {
                    exit(0); // Log Out
                }
                return true;
            }
            if self.menu_open {
                self.menu_open = false;
                changed = true;
            }
            // 3. dock icons (32px tiles; labels included in the hit area)
            let dy = self.fb_h as i32 - DOCK_H;
            if my >= dy {
                for k in 0..3 {
                    let x = DOCK_X[k];
                    if mx >= x - 4 && mx < x + ICON + 20 {
                        match k {
                            0 => {
                                self.open_window("Terminal", "ush v0.1\nType 'exit' to quit");
                            }
                            1 => {
                                let mut lb = [0u8; 512];
                                let n = ls(&mut lb);
                                let n = if n < 0 { 0 } else { n as usize };
                                let listing = core::str::from_utf8(&lb[..n]).unwrap_or("");
                                let mut content = [0u8; 2048];
                                let mut q = 0usize;
                                for part in [b"/ \n".as_slice(), listing.as_bytes(), b"\n(read-only v1)".as_slice()] {
                                    let take = part.len().min(content.len() - q - 1);
                                    content[q..q + take].copy_from_slice(&part[..take]);
                                    q += take;
                                }
                                let s = core::str::from_utf8(&content[..q]).unwrap_or("/\n");
                                self.open_window("Files", s);
                            }
                            _ => {
                                self.open_window("About", Self::ABOUT);
                            }
                        }
                        return true;
                    }
                }
            }
            // 4. windows: traffic lights
            for i in 0..MAX_WINS {
                if let Some(ref w) = self.wins[i] {
                    if mx >= w.x && mx < w.x + w.w as i32 && my >= w.y && my < w.y + w.h as i32 {
                        if my < w.y + 16 {
                            let lx = mx - w.x;
                            if lx >= 4 && lx < 12 {
                                self.close_window(i);
                                changed = true;
                            }
                        }
                        break;
                    }
                }
            }
        } else if ev == 2 {
            // right click closes whatever is under the pointer
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
                exit(0); // quit exaltop, back to ush
            } else if alt == 1 && key == b'1' {
                self.open_window("Terminal", "ush v0.1\nType 'exit' to quit");
                return true;
            } else if alt == 1 && key == b'a' {
                self.open_window("About", Self::ABOUT);
                return true;
            } else if key == b' ' && self.menu_open {
                self.menu_open = false;
                return true;
            }
        }
        false
    }

    /// One full frame: redraw the scene into the back buffer (with the
    /// cursor) and blit. Expensive — call only when the scene changed.
    fn full_frame(&mut self) {
        self.draw_desktop();
        for i in 0..MAX_WINS {
            self.draw_window(i);
        }
        self.draw_session_menu();
        self.cursor_save();
        self.cursor_draw();
        self.blit();
    }

    fn run(&mut self) -> ! {
        let greet = self.brand_greeting();
        self.open_window(greet, Self::WELCOME);
        let mut mb = [0i32; 3];
        mouse_poll(&mut mb);
        self.mouse_x = mb[0];
        self.mouse_y = mb[1];
        self.full_frame();
        loop {
            // event-driven like e1wm: incremental cursor patches during
            // movement, full redraw only on scene changes
            if self.poll_key() | self.poll_mouse() {
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
        TOP = Some(Exaltop::new());
        TOP.as_mut().unwrap().run();
    }
    exit(0)
}
