//! libe1: minimal runtime + syscall stubs for tAbLeOS userspace programs.
//!
//! Every app links this crate. `_start` is the ELF entry point.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// ---------------------------------------------------------------------------
// Syscall numbers (must match kernel/src/syscall.rs)
// ---------------------------------------------------------------------------
pub const SYS_EXIT: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_READ: u64 = 2;
pub const SYS_OPEN: u64 = 3;
pub const SYS_CLOSE: u64 = 4;
pub const SYS_READDIR: u64 = 5;
pub const SYS_FORK: u64 = 6;
pub const SYS_WAITPID: u64 = 7;
pub const SYS_EXEC: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MUNMAP: u64 = 10;
pub const SYS_BRK: u64 = 11;
pub const SYS_GETPID: u64 = 12;
pub const SYS_GETCWD: u64 = 13;
pub const SYS_CHDIR: u64 = 14;
pub const SYS_IOCTL: u64 = 15;
pub const SYS_FCNTL: u64 = 16;
pub const SYS_STAT: u64 = 17;
pub const SYS_GETTIMEOFDAY: u64 = 18;
pub const SYS_GETPPID: u64 = 19;
pub const SYS_DUP2: u64 = 20;
pub const SYS_PIPE: u64 = 21;
pub const SYS_ALARM: u64 = 22;
pub const SYS_KILL: u64 = 23;
pub const SYS_YIELD: u64 = 24;

pub const SYS_FB_INFO: u64 = 64;
pub const SYS_FB_FLUSH: u64 = 65;
pub const SYS_GETKEY: u64 = 66;
pub const SYS_MOUSE_POLL: u64 = 67;

pub const SYS_LINUX_EXEC: u64 = 80;

pub const SYS_RIT_INSTALL: u64 = 90;
pub const SYS_RIT_LIST: u64 = 91;

// ---------------------------------------------------------------------------
// Syscall wrappers
// ---------------------------------------------------------------------------
#[inline(always)]
pub fn syscall0(n: u64) -> i64 {
    let r: i64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            options(nostack)
        );
    }
    r
}

#[inline(always)]
pub fn syscall1(n: u64, a: u64) -> i64 {
    let r: i64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a,
            options(nostack)
        );
    }
    r
}

#[inline(always)]
pub fn syscall2(n: u64, a: u64, b: u64) -> i64 {
    let r: i64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a,
            in("rsi") b,
            options(nostack)
        );
    }
    r
}

#[inline(always)]
pub fn syscall3(n: u64, a: u64, b: u64, c: u64) -> i64 {
    let r: i64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            options(nostack)
        );
    }
    r
}

#[inline(always)]
pub fn syscall4(n: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    let r: i64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            in("r10") d,
            options(nostack)
        );
    }
    r
}

// ---------------------------------------------------------------------------
// High-level wrappers
// ---------------------------------------------------------------------------
pub fn write(fd: u64, buf: &[u8]) -> i64 {
    syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64)
}

pub fn read(fd: u64, buf: &mut [u8]) -> i64 {
    syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64)
}

pub fn open(path: &str) -> i64 {
    syscall3(SYS_OPEN, path.as_ptr() as u64, 0, 0)
}

pub fn close(fd: u64) -> i64 {
    syscall1(SYS_CLOSE, fd)
}

pub fn exit(code: i32) -> ! {
    syscall1(SYS_EXIT, code as u64);
    loop {}
}

pub fn getpid() -> i64 {
    syscall0(SYS_GETPID)
}

pub fn yield_cpu() {
    syscall0(SYS_YIELD);
}

pub fn getcwd(buf: &mut [u8]) -> i64 {
    syscall2(SYS_GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64)
}

pub fn chdir(path: &str) -> i64 {
    syscall1(SYS_CHDIR, path.as_ptr() as u64)
}

pub fn gettimeofday(tv: &mut [u64; 2]) -> i64 {
    syscall1(SYS_GETTIMEOFDAY, tv.as_mut_ptr() as u64)
}

pub fn fb_info(buf: &mut [u32; 5]) -> i64 {
    syscall1(SYS_FB_INFO, buf.as_mut_ptr() as u64)
}

pub fn fb_flush() -> i64 {
    syscall0(SYS_FB_FLUSH)
}

pub fn getkey(buf: &mut [u8; 5]) -> i64 {
    syscall1(SYS_GETKEY, buf.as_mut_ptr() as u64)
}

pub fn mouse_poll(buf: &mut [i32; 3]) -> i64 {
    syscall1(SYS_MOUSE_POLL, buf.as_mut_ptr() as u64)
}

pub fn rit_install(path: &str) -> i64 {
    syscall1(SYS_RIT_INSTALL, path.as_ptr() as u64)
}

pub fn rit_list(buf: &mut [u8]) -> i64 {
    syscall2(SYS_RIT_LIST, buf.as_mut_ptr() as u64, buf.len() as u64)
}

// ---------------------------------------------------------------------------
// Print macros
// ---------------------------------------------------------------------------
pub struct Print;
impl core::fmt::Write for Print {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write(1, s.as_bytes());
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let _ = write!($crate::Print, $($arg)*);
    });
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ({
        $crate::print!("{}\n", format_args!($($arg)*));
    });
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------
core::arch::global_asm!(
    ".section .text.start",
    ".global _start",
    "_start:",
    "call main",
    "mov rdi, rax",
    "mov rax, 0",
    "int 0x80",
);

extern "C" {
    fn main() -> i32;
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1);
}
