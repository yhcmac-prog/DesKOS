//! libe1: minimal ring3 system-call bindings for tAbLeOS (int 0x80).

#![no_std]

use core::arch::asm;

// Dual brand: closed-source tAbLeOS and open-source DesKOS share one code
// base; the build passes `--cfg brand=table|desk`.
#[cfg(brand = "desk")]
pub const BRAND_NAME: &str = "DesKOS";
#[cfg(not(brand = "desk"))]
pub const BRAND_NAME: &str = "tAbLeOS";

#[cfg(brand = "desk")]
pub const BRAND_LICENSE: &str = "Open source under the MIT License";
#[cfg(not(brand = "desk"))]
pub const BRAND_LICENSE: &str = "Closed source, all rights reserved";

#[inline(always)]
unsafe fn sc1(n: u64, a: u64) -> i64 {
    let ret: i64;
    asm!("int 0x80",
        in("rax") n, in("rdi") a,
        lateout("rax") ret,
        out("rcx") _, out("r11") _);
    ret
}

#[inline(always)]
unsafe fn sc3(n: u64, a: u64, b: u64, c: u64) -> i64 {
    let ret: i64;
    asm!("int 0x80",
        in("rax") n, in("rdi") a, in("rsi") b, in("rdx") c,
        lateout("rax") ret,
        out("rcx") _, out("r11") _);
    ret
}

/// Copy `path` into a stack buffer that is guaranteed null-terminated,
/// because the kernel's cstr() walks until the first zero byte.
fn path_ptr(path: &str) -> [u8; 128] {
    let mut buf = [0u8; 128];
    let len = path.len().min(127);
    buf[..len].copy_from_slice(path.as_bytes());
    buf
}

/// 0: terminate the calling process.
pub fn exit(code: i32) -> ! {
    unsafe { sc1(0, code as u64) };
    loop { unsafe { asm!("hlt"); } }
}

/// 1: write bytes to fd (1=stdout).
pub fn write(fd: u64, buf: &[u8]) -> i64 {
    unsafe { sc3(1, fd, buf.as_ptr() as u64, buf.len() as u64) }
}

/// 2: read a line from the serial console (echoes; blocks).
pub fn read(fd: u64, buf: &mut [u8]) -> i64 {
    unsafe { sc3(2, fd, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// 8: spawn `path` as a child and block until it exits.
pub fn exec(path: &str) -> i64 {
    let p = path_ptr(path);
    unsafe { sc1(8, p.as_ptr() as u64) }
}

/// 18: milliseconds since boot.
pub fn gettimeofday() -> u64 {
    let mut ms: u64 = 0;
    unsafe { sc3(18, &mut ms as *mut u64 as u64, 0, 0) };
    ms
}

/// 25: list root directory names (newline separated) into buf.
pub fn ls(buf: &mut [u8]) -> i64 {
    unsafe { sc3(25, buf.as_mut_ptr() as u64, buf.len() as u64, 0) }
}

/// 26: read a whole file.
pub fn read_file(path: &str, buf: &mut [u8]) -> i64 {
    let p = path_ptr(path);
    unsafe { sc3(26, p.as_ptr() as u64, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// 27: create a file with content.
pub fn create_file(path: &str, data: &[u8]) -> i64 {
    let p = path_ptr(path);
    unsafe { sc3(27, p.as_ptr() as u64, data.as_ptr() as u64, data.len() as u64) }
}

/// 28: process table listing.
pub fn ps(buf: &mut [u8]) -> i64 {
    unsafe { sc3(28, buf.as_mut_ptr() as u64, buf.len() as u64, 0) }
}

/// 80: e1wine — run a Linux-ABI ELF as a child, block until it exits.
/// argv is passed as NUL-separated strings with an empty-string terminator.
pub fn wine_execv(argv: &[&str]) -> i64 {
    let mut buf = [0u8; 256];
    let mut pos = 0usize;
    for a in argv {
        let b = a.as_bytes();
        let n = b.len().min(buf.len() - pos - 2);
        if n == 0 {
            break;
        }
        buf[pos..pos + n].copy_from_slice(&b[..n]);
        pos += n + 1; // skip the NUL we just reserved
    }
    buf[pos] = 0; // empty-string terminator (buf[pos] is already 0 unless overwritten)
    unsafe { sc1(80, buf.as_ptr() as u64) }
}

/// 80: e1wine with no extra arguments.
pub fn wine_exec(path: &str) -> i64 {
    wine_execv(&[path])
}

/// 90: install a .rit package from the fs.
pub fn rit_install(path: &str) -> i64 {
    let p = path_ptr(path);
    unsafe { sc1(90, p.as_ptr() as u64) }
}

/// 91: list installed package names (newline separated).
pub fn rit_list(buf: &mut [u8]) -> i64 {
    unsafe { sc3(91, buf.as_mut_ptr() as u64, buf.len() as u64, 0) }
}

/// 92: read raw 512B sectors from the second disk (primary slave) into buf.
/// buf.len() must be a multiple of 512 (max 255*512 per call).
/// Returns bytes read or -1.
pub fn readblocks(lba: u64, buf: &mut [u8]) -> i64 {
    unsafe { sc3(92, lba, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// 96: write raw 512B sectors to the second disk (primary slave) from buf.
/// buf.len() must be a multiple of 512 (max 255*512 per call).
/// Returns bytes written or -1.
pub fn writeblocks(lba: u64, buf: &[u8]) -> i64 {
    unsafe { sc3(96, lba, buf.as_ptr() as u64, buf.len() as u64) }
}

/// 97: exec `path` as a child with one string argument (passed to the
/// child's `_start(arg: *const u8)`). Blocks until the child exits.
pub fn exec_arg(path: &str, arg: &str) -> i64 {
    let p = path_ptr(path);
    let a = path_ptr(arg);
    unsafe { sc3(97, p.as_ptr() as u64, a.as_ptr() as u64, 0) }
}

/// 93: attempt to activate with an activation key string (e.g. "XXXX-XXXX-XXXX-XXXX").
/// On success returns 0 and writes the 2-digit serial ("NN") or "PERM" into serial_out.
/// Negative = invalid key.
pub fn try_activate(key: &str, serial_out: &mut [u8; 4]) -> i64 {
    let p = path_ptr(key);
    unsafe {
        sc3(93, p.as_ptr() as u64, serial_out.as_mut_ptr() as u64, 0)
    }
}

/// 94: mark system as activated (used at boot when license.cfg is present).
pub fn set_activated() -> i64 {
    unsafe { sc1(94, 0) }
}

/// 95: returns 1 if activated (tAbLeOS premium features unlocked), 0 otherwise.
pub fn brand_status() -> i64 {
    unsafe { sc1(95, 0) }
}

// ---------------------------------------------------------------------------
// GUI / framebuffer (M5)
// ---------------------------------------------------------------------------

/// 64: read framebuffer info into buf: [phys, width, height, pitch, bpp].
pub fn fb_info(buf: &mut [u32; 5]) -> i64 {
    unsafe { sc3(64, buf.as_mut_ptr() as u64, 0, 0) }
}

/// 68: map the framebuffer into the caller's address space (identity, RW+USER).
/// Must be called once before writing to fb_info's physical address.
pub fn fb_map() -> i64 {
    unsafe { sc3(68, 0, 0, 0) }
}

/// 69: allocate + map a fb-sized back buffer (cached RAM) at a fixed VA.
/// Returns the buffer VA, or -1. Draw here, then blit to the fb.
pub fn fb_backbuf() -> i64 {
    unsafe { sc3(69, 0, 0, 0) }
}

/// 65: flush (no-op for a directly-mapped PIO framebuffer; kept for symmetry).
pub fn fb_flush() -> i64 {
    unsafe { sc3(65, 0, 0, 0) }
}

/// 66: poll a keyboard event into buf: [code, pressed, alt, ctrl, shift].
/// Returns 1 if an event was available.
pub fn getkey(buf: &mut [u8]) -> i64 {
    unsafe { sc3(66, buf.as_mut_ptr() as u64, 0, 0) }
}

/// 67: poll mouse state into buf: [x, y, buttons]. Returns 1 on left-click
/// edge, 2 on right-click edge, 0 otherwise.
pub fn mouse_poll(buf: &mut [i32]) -> i64 {
    unsafe { sc3(67, buf.as_mut_ptr() as u64, 0, 0) }
}

/// 24: yield the CPU.
pub fn yield_cpu() {
    unsafe { sc3(24, 0, 0, 0) };
}
