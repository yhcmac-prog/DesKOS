//! e1vfsx: interpreter-style 8086 real-mode VM for tAbLeOS / DesKOS.
//!
//! Loads a .com image at DS:0x100 in a 64K address space and interprets a
//! practical opcode subset (mov r,r/imm, jmp/jz/jnz, cmp, push/pop,
//! call/ret, add/sub/xor, inc/dec, int, hlt, nop). INT 10h AH=0E teletype
//! and INT 21h AH=02/09/4C are implemented; the virtual 80x25 text screen
//! is rendered with the 8x8 font into the framebuffer (640x200, centered),
//! and output is mirrored to the serial console (fd 1) for headless use.
//!
//! Invoked as `e1vfsx <file.com>` (arg passed in rdi by sys_exec_arg) or
//! bare, which asks for the file name on the console.

#![no_std]
#![no_main]

use libe1::*;
use core::panic::PanicInfo;

include!("font8x8.rs");

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    libe1::exit(1)
}

// ---------------------------------------------------------------- VM state

const AX: usize = 0;
const CX: usize = 1;
const DX: usize = 2;
const BX: usize = 3;
const SP: usize = 4;
const BP: usize = 5;
const SI: usize = 6;
const DI: usize = 7;

const COLS: usize = 80;
const ROWS: usize = 25;

const MAX_INSNS: u64 = 20_000_000;

struct Vm {
    regs: [u16; 8],
    ip: u16,
    zf: bool,
    running: bool,
    exit_code: u8,
    mem: [u8; 65536],
    // text screen: (char, attr) per cell
    scr: [(u8, u8); COLS * ROWS],
    cx: usize,
    cy: usize,
}

impl Vm {
    fn new() -> Self {
        let mut v = Vm {
            regs: [0; 8],
            ip: 0x100,
            zf: false,
            running: true,
            exit_code: 0,
            mem: [0; 65536],
            scr: [(0, 7); COLS * ROWS],
            cx: 0,
            cy: 0,
        };
        v.regs[SP] = 0xFFFE;
        v
    }

    fn r8(&self, i: usize) -> u8 {
        if i < 4 {
            (self.regs[i] & 0xFF) as u8
        } else {
            (self.regs[i - 4] >> 8) as u8
        }
    }

    fn set_r8(&mut self, i: usize, v: u8) {
        if i < 4 {
            self.regs[i] = (self.regs[i] & 0xFF00) | v as u16;
        } else {
            self.regs[i - 4] = (self.regs[i - 4] & 0x00FF) | ((v as u16) << 8);
        }
    }

    fn fetch8(&mut self) -> u8 {
        let b = self.mem[self.ip as usize];
        self.ip = self.ip.wrapping_add(1);
        b
    }

    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch8() as u16;
        let hi = self.fetch8() as u16;
        (hi << 8) | lo
    }

    fn push(&mut self, v: u16) {
        self.regs[SP] = self.regs[SP].wrapping_sub(2);
        let sp = self.regs[SP] as usize;
        self.mem[sp] = (v & 0xFF) as u8;
        self.mem[sp + 1] = (v >> 8) as u8;
    }

    fn pop(&mut self) -> u16 {
        let sp = self.regs[SP] as usize;
        let v = self.mem[sp] as u16 | ((self.mem[sp + 1] as u16) << 8);
        self.regs[SP] = self.regs[SP].wrapping_add(2);
        v
    }
}

// VGA 16-color palette (approximate RGB)
fn vga_color(i: u8) -> u32 {
    match i & 0xF {
        0 => 0x000000,
        1 => 0x0000AA,
        2 => 0x00AA00,
        3 => 0x00AAAA,
        4 => 0xAA0000,
        5 => 0xAA00AA,
        6 => 0xAA5500,
        7 => 0xAAAAAA,
        8 => 0x555555,
        9 => 0x5555FF,
        10 => 0x55FF55,
        11 => 0x55FFFF,
        12 => 0xFF5555,
        13 => 0xFF55FF,
        14 => 0xFFFF55,
        _ => 0xFFFFFF,
    }
}

// ---------------------------------------------------------------- graphics

struct Gui {
    fb: *mut u32,
    buf: *mut u32,
    w: i32,
    h: i32,
    stride: i32,
    ox: i32, // text area origin (640x200 centered)
    oy: i32,
}

impl Gui {
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
        Gui {
            fb: phys as *mut u32,
            buf: bb as *mut u32,
            w: w as i32,
            h: h as i32,
            stride: (pitch / 4) as i32,
            ox: (w as i32 - 640) / 2,
            oy: (h as i32 - 200) / 2,
        }
    }

    fn put(&self, x: i32, y: i32, c: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        unsafe { self.buf.offset((y * self.stride + x) as isize).write(c) };
    }

    fn blit_all(&self) {
        let n = (self.stride as i64 * self.h as i64) as isize;
        unsafe {
            for i in 0..n {
                self.fb.offset(i).write_volatile(self.buf.offset(i).read());
            }
        }
    }

    fn char_at(&self, x: i32, y: i32, ch: char, color: u32) {
        let glyph = font8x8(ch);
        for row in 0..8i32 {
            let bits = glyph[row as usize];
            for col in 0..8i32 {
                if bits & (1 << col) != 0 {
                    self.put(x + col, y + row, color);
                }
            }
        }
    }

    /// Draw the whole 80x25 text screen (640x200) centered on the desktop.
    fn render_screen(&self, vm: &Vm) {
        // dark backdrop around the virtual monitor
        for y in 0..self.h {
            for x in 0..self.w {
                let inside = x >= self.ox && x < self.ox + 640 && y >= self.oy && y < self.oy + 200;
                self.put(x, y, if inside { 0x000000 } else { 0x14141C });
        }
        }
        for row in 0..ROWS {
            for col in 0..COLS {
                let (ch, attr) = vm.scr[row * COLS + col];
                if ch == 0 {
                    continue;
                }
                self.char_at(
                    self.ox + col as i32 * 8,
                    self.oy + row as i32 * 8,
                    ch as char,
                    vga_color(attr),
                );
            }
        }
        // block cursor
        let (cx, cy, cattr) = (vm.cx, vm.cy, 7u8);
        let _ = cattr;
        for dy in 0..8i32 {
            for dx in 0..8i32 {
                self.put(self.ox + cx as i32 * 8 + dx, self.oy + cy as i32 * 8 + dy, 0x303030);
            }
        }
        self.blit_all();
    }
}

// ---------------------------------------------------------------- VM exec

fn teletype(gui: &Gui, vm: &mut Vm, ch: u8) {
    // mirror to serial console (headless testing)
    let b = [ch];
    write(1, &b);
    match ch {
        b'\r' => {
            vm.cx = 0;
        }
        b'\n' => {
            vm.cx = 0;
            vm.cy += 1;
        }
        8 => {
            // backspace
            if vm.cx > 0 {
                vm.cx -= 1;
                vm.scr[vm.cy * COLS + vm.cx] = (0, 7);
            }
        }
        _ => {
            if (ch as usize) < 128 {
                vm.scr[vm.cy * COLS + vm.cx] = (ch, 7);
                vm.cx += 1;
                if vm.cx >= COLS {
                    vm.cx = 0;
                    vm.cy += 1;
                }
            }
        }
    }
    if vm.cy >= ROWS {
        // scroll up one line
        vm.scr.copy_within(COLS..COLS * ROWS, 0);
        for c in (ROWS - 1) * COLS..ROWS * COLS {
            vm.scr[c] = (0, 7);
        }
        vm.cy = ROWS - 1;
    }
    let _ = gui;
}

fn do_int(gui: &Gui, vm: &mut Vm, n: u8) {
    match n {
        0x10 => match vm.r8(4) {
            // AH
            0x0E => teletype(gui, vm, vm.r8(0)), // AL
            0x00 => {
                // set video mode: clear screen
                for c in 0..COLS * ROWS {
                    vm.scr[c] = (0, 7);
                }
                vm.cx = 0;
                vm.cy = 0;
            }
            _ => {}
        },
        0x20 => {
            vm.running = false;
            vm.exit_code = 0;
        }
        0x21 => match vm.r8(4) {
            // AH
            0x02 => teletype(gui, vm, vm.r8(2)), // DL
            0x09 => {
                // print $-terminated string at DS:DX
                let mut off = vm.regs[DX] as usize;
                let mut guard = 0;
                while guard < 4096 {
                    let ch = vm.mem[off];
                    if ch == b'$' {
                        break;
                    }
                    teletype(gui, vm, ch);
                    off = (off + 1) & 0xFFFF;
                    guard += 1;
                }
            }
            0x4C => {
                vm.running = false;
                vm.exit_code = vm.r8(0); // AL
            }
            _ => {}
        },
        _ => {} // unsupported interrupt: ignore
    }
    gui.render_screen(vm);
}

/// Decode and run one instruction. Returns Err on unsupported opcode.
fn step(vm: &mut Vm) -> Result<(), u8> {
    let op = vm.fetch8();
    match op {
        0x90 => {} // nop
        0xF4 => {
            // hlt: stop the VM
            vm.running = false;
        }
        0xCD => {
            let n = vm.fetch8();
            do_int_ext(vm, n);
        }
        0xB0..=0xB7 => {
            let v = vm.fetch8();
            vm.set_r8((op - 0xB0) as usize, v);
        }
        0xB8..=0xBF => {
            let v = vm.fetch16();
            vm.regs[(op - 0xB8) as usize] = v;
        }
        0x50..=0x57 => {
            let v = vm.regs[(op - 0x50) as usize];
            vm.push(v);
        }
        0x58..=0x5F => {
            let v = vm.pop();
            vm.regs[(op - 0x58) as usize] = v;
        }
        0x40..=0x47 => {
            let r = (op - 0x40) as usize;
            let (v, _) = add16(vm.regs[r], 1);
            vm.regs[r] = v;
        }
        0x48..=0x4F => {
            let r = (op - 0x48) as usize;
            let (v, z) = sub16(vm.regs[r], 1);
            vm.regs[r] = v;
            vm.zf = z;
        }
        0xEB => {
            // jmp rel8
            let rel = vm.fetch8() as i8 as i16;
            vm.ip = (vm.ip as i16 + rel) as u16;
        }
        0xE9 => {
            let rel = vm.fetch16() as i16;
            vm.ip = (vm.ip as i32 + rel as i32) as u16;
        }
        0x74 => {
            let rel = vm.fetch8() as i8 as i16;
            if vm.zf {
                vm.ip = (vm.ip as i16 + rel) as u16;
            }
        }
        0x75 => {
            let rel = vm.fetch8() as i8 as i16;
            if !vm.zf {
                vm.ip = (vm.ip as i16 + rel) as u16;
            }
        }
        0xE8 => {
            // call rel16
            let rel = vm.fetch16() as i16;
            let ret_ip = vm.ip;
            vm.push(ret_ip);
            vm.ip = (vm.ip as i32 + rel as i32) as u16;
        }
        0xC3 => {
            vm.ip = vm.pop();
        }
        0x05 => {
            let v = vm.fetch16();
            let (r, z) = add16(vm.regs[AX], v);
            vm.regs[AX] = r;
            vm.zf = z;
        }
        0x2D => {
            let v = vm.fetch16();
            let (r, z) = sub16(vm.regs[AX], v);
            vm.regs[AX] = r;
            vm.zf = z;
        }
        0x35 => {
            let v = vm.fetch16();
            vm.regs[AX] ^= v;
            vm.zf = vm.regs[AX] == 0;
        }
        0x3D => {
            let v = vm.fetch16();
            vm.zf = vm.regs[AX] == v;
        }
        0x01 | 0x03 | 0x29 | 0x2B | 0x31 | 0x33 | 0x39 | 0x3B => {
            // modrm forms (register operands only: mod=11)
            let modrm = vm.fetch8();
            let m = modrm >> 6;
            let reg = ((modrm >> 3) & 7) as usize;
            let rm = (modrm & 7) as usize;
            if m != 3 {
                return Err(op);
            }
            match op {
                0x01 => {
                    let (v, z) = add16(vm.regs[rm], vm.regs[reg]);
                    vm.regs[rm] = v;
                    vm.zf = z;
                } // add rm16, r16
                0x03 => {
                    let (v, z) = add16(vm.regs[reg], vm.regs[rm]);
                    vm.regs[reg] = v;
                    vm.zf = z;
                } // add r16, rm16
                0x29 => {
                    let (v, z) = sub16(vm.regs[rm], vm.regs[reg]);
                    vm.regs[rm] = v;
                    vm.zf = z;
                } // sub rm16, r16
                0x2B => {
                    let (v, z) = sub16(vm.regs[reg], vm.regs[rm]);
                    vm.regs[reg] = v;
                    vm.zf = z;
                } // sub r16, rm16
                0x31 => {
                    vm.regs[rm] ^= vm.regs[reg];
                    vm.zf = vm.regs[rm] == 0;
                }
                0x33 => {
                    vm.regs[reg] ^= vm.regs[rm];
                    vm.zf = vm.regs[reg] == 0;
                }
                0x39 => vm.zf = vm.regs[rm] == vm.regs[reg], // cmp rm16, r16
                _ => vm.zf = vm.regs[reg] == vm.regs[rm],    // 0x3B cmp r16, rm16
            }
        }
        0x83 => {
            // group: add/sub/xor/cmp rm16, imm8 (register only)
            let modrm = vm.fetch8();
            let m = modrm >> 6;
            let ext = ((modrm >> 3) & 7) as usize;
            let rm = (modrm & 7) as usize;
            if m != 3 {
                return Err(op);
            }
            let v = vm.fetch8() as i8 as i16 as u16;
            match ext {
                0 => {
                    let (r, z) = add16(vm.regs[rm], v);
                    vm.regs[rm] = r;
                    vm.zf = z;
                }
                5 => {
                    let (r, z) = sub16(vm.regs[rm], v);
                    vm.regs[rm] = r;
                    vm.zf = z;
                }
                6 => {
                    vm.regs[rm] ^= v;
                    vm.zf = vm.regs[rm] == 0;
                }
                7 => vm.zf = vm.regs[rm] == v,
                _ => return Err(op),
            }
        }
        0x80 => {
            // group1 byte: cmp/add/sub rm8, imm8 (register only)
            let modrm = vm.fetch8();
            let m = modrm >> 6;
            let ext = ((modrm >> 3) & 7) as usize;
            let rm = (modrm & 7) as usize;
            if m != 3 {
                return Err(op);
            }
            let v = vm.fetch8();
            match ext {
                0 => {
                    let (r, z) = add8(vm.r8(rm), v);
                    vm.set_r8(rm, r);
                    vm.zf = z;
                }
                5 => {
                    let (r, z) = sub8(vm.r8(rm), v);
                    vm.set_r8(rm, r);
                    vm.zf = z;
                }
                7 => vm.zf = vm.r8(rm) == v,
                _ => return Err(op),
            }
        }
        0x88 | 0x89 | 0x8A | 0x8B => {
            // mov rm, r / r, rm (register only)
            let modrm = vm.fetch8();
            let m = modrm >> 6;
            let reg = ((modrm >> 3) & 7) as usize;
            let rm = (modrm & 7) as usize;
            if m != 3 {
                return Err(op);
            }
            if op & 1 == 0 {
                // byte
                if op == 0x88 {
                    let v = vm.r8(reg);
                    vm.set_r8(rm, v);
                } else {
                    let v = vm.r8(rm);
                    vm.set_r8(reg, v);
                }
            } else if op == 0x89 {
                let v = vm.regs[reg];
                vm.regs[rm] = v;
            } else {
                let v = vm.regs[rm];
                vm.regs[reg] = v;
            }
        }
        _ => return Err(op),
    }
    Ok(())
}

// helper so do_int can be called from step without &Gui threading through
// every match arm; the gui pointer is stashed in a static before run()
static mut GUI_P: *mut Gui = core::ptr::null_mut();
static mut FBUF: [u8; 0x10000 - 0x100] = [0u8; 0x10000 - 0x100];
// Vm is ~68KB — cannot live on 16KB user stack
static mut VM_SPACE: Vm = Vm {
    regs: [0; 8],
    ip: 0,
    zf: false,
    running: false,
    exit_code: 0,
    mem: [0; 65536],
    scr: [(0, 7); COLS * ROWS],
    cx: 0,
    cy: 0,
};

fn do_int_ext(vm: &mut Vm, n: u8) {
    unsafe {
        let gui = &mut *GUI_P;
        do_int(gui, vm, n);
    }
}

fn add16(a: u16, b: u16) -> (u16, bool) {
    let r = a.wrapping_add(b);
    (r, r == 0)
}
fn sub16(a: u16, b: u16) -> (u16, bool) {
    let r = a.wrapping_sub(b);
    (r, r == 0)
}
fn add8(a: u8, b: u8) -> (u8, bool) {
    let r = a.wrapping_add(b);
    (r, r == 0)
}
fn sub8(a: u8, b: u8) -> (u8, bool) {
    let r = a.wrapping_sub(b);
    (r, r == 0)
}

// ---------------------------------------------------------------- entry

fn puts(s: &str) {
    write(1, s.as_bytes());
}

/// cstr at an arbitrary (kernel-provided) user address.
unsafe fn arg_str(p: u64) -> Option<&'static str> {
    if p == 0 {
        return None;
    }
    let ptr = p as *const u8;
    let mut len = 0usize;
    while len < 128 && *ptr.add(len) != 0 {
        len += 1;
    }
    let slice = core::slice::from_raw_parts(ptr, len);
    core::str::from_utf8(slice).ok()
}

fn font8x8(c: char) -> [u8; 8] {
    let idx = c as usize;
    if idx >= 128 {
        return [0; 8];
    }
    FONT8[idx]
}

#[no_mangle]
pub extern "C" fn _start(arg: u64) -> ! {
    let mut gui = Gui::new();
    unsafe {
        GUI_P = &mut gui as *mut Gui;
    }

    // resolve the .com path: rdi argument or interactive prompt
    let mut ibuf = [0u8; 128];
    let path: &str = match unsafe { arg_str(arg) } {
        Some(s) if !s.is_empty() => s,
        _ => {
            puts("e1vfsx: 8086 VM (subset interpreter)\n");
            puts("file to run: ");
            let n = read(0, &mut ibuf);
            if n <= 0 {
                exit(1);
            }
            let s = core::str::from_utf8(&ibuf[..n as usize]).unwrap_or("");
            s.trim_end_matches(|c: char| c == '\r' || c == '\n' || c == ' ')
        }
    };

    let fbuf = unsafe { &mut FBUF };
    let n = read_file(path, fbuf);
    if n <= 0 {
        puts("e1vfsx: cannot read '");
        puts(path);
        puts("'\n");
        exit(1);
    }

    // use static VM space to avoid 68KB stack overflow
    let mut vm: &mut Vm = unsafe { &mut VM_SPACE };
    // Vm::new() fields: regs all zeroed below, ip/running/SP need explicit
    vm.regs = [0; 8];
    vm.ip = 0x100;
    vm.zf = false;
    vm.running = true;
    vm.exit_code = 0;
    vm.mem = [0; 65536];
    vm.scr = [(0, 7); COLS * ROWS];
    vm.cx = 0;
    vm.cy = 0;
    vm.regs[SP] = 0xFFFE;
    // classic .com termination: RET to offset 0 hits INT 20h
    vm.mem[0] = 0xCD;
    vm.mem[1] = 0x20;
    vm.mem[0x100..0x100 + n as usize].copy_from_slice(&fbuf[..n as usize]);

    gui.render_screen(&vm);

    let mut count = 0u64;
    let mut err: Option<u8> = None;
    while vm.running {
        if let Err(op) = step(&mut vm) {
            err = Some(op);
            break;
        }
        count += 1;
        if count >= MAX_INSNS {
            err = Some(0xFF);
            break;
        }
        if count % 4096 == 0 {
            yield_cpu();
        }
    }

    // final frame + pause so the user can read the virtual screen
    gui.render_screen(&vm);
    match err {
        Some(0xFF) => puts("e1vfsx: instruction limit exceeded\n"),
        Some(op) => {
            puts("e1vfsx: unsupported opcode 0x");
            put_hex8(op);
            puts("\n");
        }
        None => {}
    }
    puts("e1vfsx: VM exited, code ");
    put_dec(vm.exit_code as u64);
    puts(" (press any key)\n");
    let mut kb = [0u8; 5];
    while getkey(&mut kb) == 0 {
        yield_cpu();
    }
    exit(0)
}

fn put_hex8(v: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let b = [HEX[(v >> 4) as usize], HEX[(v & 0xF) as usize]];
    write(1, &b);
}

fn put_dec(mut v: u64) {
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    if v == 0 {
        puts("0");
        return;
    }
    while v > 0 {
        i -= 1;
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    write(1, &tmp[i..]);
}
