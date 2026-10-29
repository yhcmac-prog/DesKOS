//! hello: first ring3 user program. Self-contained (no libe1) to validate
//! the minimal syscall path.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

unsafe fn sc3(n: u64, a: u64, b: u64, c: u64) -> i64 {
    let ret: i64;
    asm!("int 0x80",
        in("rax") n, in("rdi") a, in("rsi") b, in("rdx") c,
        lateout("rax") ret,
        out("rcx") _, out("r11") _);
    ret
}

fn write(fd: u64, buf: &[u8]) -> i64 {
    unsafe { sc3(1, fd, buf.as_ptr() as u64, buf.len() as u64) }
}

fn exit(code: i32) -> ! {
    unsafe { asm!("int 0x80", in("rax") 0u64, in("rdi") code as u64, options(noreturn)) }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write(1, b"hello from ring3!\n");
    write(1, b"exit(0) now\n");
    exit(0);
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    exit(1);
}
