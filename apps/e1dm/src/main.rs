//! e1dm: minimal graphical display manager for tAbLeOS / DesKOS.
//!
//! Fullscreen dark selector: big brand title, two session entries
//! (e1wm Desktop / exalTOP), keyboard selection, 10s timeout that starts
//! the default from desktop.cfg ("session=e1wm|exaltop").

#![no_std]
#![no_main]

use libe1::*;
use core::panic::PanicInfo;

include!("font8x8.rs");

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    libe1::exit(1)
}

const BG: u32 = 0x101018;
const SEL_BG: u32 = 0x2A4A7A;
const ROW_BG: u32 = 0x1A1A22;
const TIMEOUT_MS: u64 = 10_000;

// brand must come from this crate's cfg (deps are compiled without it)
#[cfg(brand = "desk")]
const BRAND: &str = "DesKOS";
#[cfg(not(brand = "desk"))]
const BRAND: &str = "tAbLeOS";

const SESSIONS: [&str; 2] = ["e1wm", "exaltop"];
const LABELS: [&str; 2] = ["1. e1wm Desktop", "2. exalTOP"];

struct Dm {
    fb: *mut u32,
    buf: *mut u32,
    w: i32,
    h: i32,
    stride: i32,
    sel: usize,
}

impl Dm {
    fn new() -> Self {
        let mut fbinfo = [0u32; 5];
        fb_info(&mut fbinfo);
        let (phys, w, h, pitch, _bpp) = (fbinfo[0], fbinfo[1], fbinfo[2], fbinfo[3], fbinfo[4]);
        if fb_map() != 0 {
            exit(1);
        }
        let bb = fb_backbuf();
        if bb < 0 {
            exit(1);
        }
        Dm {
            fb: phys as *mut u32,
            buf: bb as *mut u32,
            w: w as i32,
            h: h as i32,
            stride: (pitch / 4) as i32,
            sel: default_session(),
        }
    }

    fn put(&self, x: i32, y: i32, c: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        unsafe { self.buf.offset((y * self.stride + x) as isize).write(c) };
    }

    fn fill(&self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        for dy in 0..h {
            for dx in 0..w {
                self.put(x + dx, y + dy, c);
            }
        }
    }

    /// Scaled ASCII char (f = pixel size per font pixel).
    fn char_scaled(&self, x: i32, y: i32, ch: char, f: i32, color: u32) {
        let glyph = font8x8(ch);
        for row in 0..8i32 {
            let bits = glyph[row as usize];
            for col in 0..8i32 {
                if bits & (1 << col) != 0 {
                    self.fill(x + col * f, y + row * f, f, f, color);
                }
            }
        }
    }

    fn string_scaled(&self, x: i32, y: i32, s: &str, f: i32, color: u32) {
        let mut cx = x;
        for ch in s.chars() {
            self.char_scaled(cx, y, ch, f, color);
            cx += 8 * f;
        }
    }

    fn string(&self, x: i32, y: i32, s: &str, color: u32) {
        self.string_scaled(x, y, s, 1, color);
    }

    fn draw_scene(&self) {
        self.fill(0, 0, self.w, self.h, BG);
        // big brand title (4x), centered
        let tw = BRAND.len() as i32 * 32;
        self.string_scaled((self.w - tw) / 2, self.h / 4, BRAND, 4, 0xF0F0F0);
        // CJK subtitle
        let sub = "请选择会话 - Select a session";
        let sw = cjk_width(sub);
        self.draw_cjk((self.w - sw) / 2, self.h / 4 + 48, sub, 0x9098A8);
        // session rows
        let rw = 320i32;
        let rx = (self.w - rw) / 2;
        for k in 0..2 {
            let ry = self.h / 2 - 20 + k as i32 * 44;
            let (bg, fg) = if k == self.sel {
                (SEL_BG, 0xFFFFFF)
            } else {
                (ROW_BG, 0x9AA0AC)
            };
            self.fill(rx, ry, rw, 32, bg);
            if k == self.sel {
                // selection border
                self.fill(rx, ry, rw, 1, 0x5A8AD0);
                self.fill(rx, ry + 31, rw, 1, 0x5A8AD0);
                self.fill(rx, ry, 1, 32, 0x5A8AD0);
                self.fill(rx + rw - 1, ry, 1, 32, 0x5A8AD0);
            }
            self.string(rx + 12, ry + 12, LABELS[k], fg);
        }
    }

    fn draw_cjk(&self, x: i32, y: i32, s: &str, color: u32) {
        let mut cx = x;
        for c in s.chars() {
            if c.is_ascii() {
                let buf = [c as u8];
                let sub = core::str::from_utf8(&buf).unwrap_or("?");
                self.string(cx, y + 4, sub, color);
                cx += 8;
            } else {
                let glyph = font16_cjk(c);
                for row in 0..16i32 {
                    let bits = glyph[row as usize];
                    for col in 0..16i32 {
                        if bits & (1 << (15 - col)) != 0 {
                            self.put(cx + col, y + row, color);
                        }
                    }
                }
                cx += 16;
            }
        }
    }

    /// Footer with live countdown, blitted as a small region.
    fn draw_footer(&self, secs_left: u64) {
        let mut num = [0u8; 20];
        let mut n = 0usize;
        let mut v = secs_left;
        if v == 0 {
            num[0] = b'0';
            n = 1;
        }
        while v > 0 {
            num[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        let mut txt = [0u8; 64];
        let mut q = 0usize;
        for part in [
            b"Enter: launch   1/2: select   up/down: move   timeout ".as_slice(),
            &num[..n],
            b"s -> ".as_slice(),
            SESSIONS[self.sel].as_bytes(),
        ] {
            let take = part.len().min(txt.len() - q - 1);
            txt[q..q + take].copy_from_slice(&part[..take]);
            q += take;
        }
        let s = core::str::from_utf8(&txt[..q]).unwrap_or("");
        let fw = s.len() as i32 * 8;
        let fy = self.h - 40;
        self.fill((self.w - fw) / 2 - 8, fy - 4, fw + 16, 18, BG);
        self.string((self.w - fw) / 2, fy, s, 0x707888);
        // region blit (footer strip only)
        let x0 = (self.w - fw) / 2 - 8;
        for dy in 0..18 {
            for dx in 0..fw + 16 {
                let off = ((fy - 4 + dy) * self.stride + x0 + dx) as isize;
                unsafe {
                    self.fb.offset(off).write_volatile(self.buf.offset(off).read());
                }
            }
        }
    }

    fn blit_all(&self) {
        let n = (self.stride as i64 * self.h as i64) as isize;
        unsafe {
            for i in 0..n {
                self.fb.offset(i).write_volatile(self.buf.offset(i).read());
            }
        }
    }
}

fn cjk_width(s: &str) -> i32 {
    let mut w = 0i32;
    for c in s.chars() {
        w += if c.is_ascii() { 8 } else { 16 };
    }
    w
}

fn default_session() -> usize {
    let mut buf = [0u8; 256];
    let n = read_file("desktop.cfg", &mut buf);
    if n > 0 {
        let s = core::str::from_utf8(&buf[..n as usize]).unwrap_or("");
        for line in s.lines() {
            if let Some(v) = line.trim().strip_prefix("session=") {
                if v.trim() == "exaltop" {
                    return 1;
                }
            }
        }
    }
    0
}

fn font8x8(c: char) -> [u8; 8] {
    let idx = c as usize;
    if idx >= 128 {
        return [0; 8];
    }
    FONT8[idx]
}

/// cjk.bin layout: 9826 * u16-LE codepoints + 9826 * 32 bytes of glyph rows.
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
    let mut dm = Dm::new();
    loop {
        dm.sel = default_session();
        dm.draw_scene();
        dm.blit_all();
        let deadline = gettimeofday() + TIMEOUT_MS;
        let mut last_sec = TIMEOUT_MS / 1000;
        dm.draw_footer(last_sec);
        // selection loop
        let chosen;
        loop {
            let mut kb = [0u8; 5];
            if getkey(&mut kb) > 0 {
                let key = kb[0];
                if key == b'\n' {
                    chosen = dm.sel;
                    break;
                } else if key == b'1' || key == b'2' {
                    chosen = (key - b'1') as usize;
                    break;
                } else if key == 0x80 {
                    // up
                    if dm.sel > 0 {
                        dm.sel -= 1;
                    } else {
                        dm.sel = 1;
                    }
                    dm.draw_scene();
                    dm.blit_all();
                    dm.draw_footer(last_sec);
                } else if key == 0x81 {
                    // down
                    if dm.sel < 1 {
                        dm.sel += 1;
                    } else {
                        dm.sel = 0;
                    }
                    dm.draw_scene();
                    dm.blit_all();
                    dm.draw_footer(last_sec);
                }
            }
            let now = gettimeofday();
            if now >= deadline {
                chosen = dm.sel;
                break;
            }
            let left = (deadline - now) / 1000;
            if left != last_sec {
                last_sec = left;
                dm.draw_footer(left);
            }
            yield_cpu();
        }
        // "Starting ..." splash then exec (blocks until the session quits)
        let name = SESSIONS[chosen.min(1)];
        dm.fill(0, 0, dm.w, dm.h, BG);
        let msg_len = name.len() as i32 * 8 + 80;
        dm.string((dm.w - msg_len) / 2, dm.h / 2, "Starting ", 0xF0F0F0);
        dm.string((dm.w - msg_len) / 2 + 72, dm.h / 2, name, 0xF0F0F0);
        dm.string((dm.w - msg_len) / 2 + 72 + name.len() as i32 * 8, dm.h / 2, " ...", 0xF0F0F0);
        dm.blit_all();
        exec(name);
        // session ended: fall back to the selector
    }
}
