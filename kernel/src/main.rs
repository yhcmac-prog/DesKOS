#![no_std]
#![no_main]

mod bootinfo;
mod brand;
mod console;
mod font;
mod gdt;
mod idt;
mod pic;
mod pit;
mod keyboard;
mod mouse;
mod pmm;
mod heap;
mod vmm;
mod sched;
mod elf;
mod ata;
mod e1fss;
mod syscall;
mod rit;
mod linux_compat;

use bootinfo::BootInfo;
use core::panic::PanicInfo;

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

// 64-bit kernel entry. stage2 jumps here with RDI = physical addr of bootinfo.
core::arch::global_asm!(
    ".section .text.boot",
    ".global _start",
    "_start:",
    "cli",
    "lea rsp, [rip + kernel_stack_top]",
    "xor rbp, rbp",
    "call kernel_main",
    "2: hlt",
    "jmp 2b",
    ".section .bss",
    ".align 16",
    "kernel_stack:",
    ".skip 65536",
    "kernel_stack_top:",
);

#[cfg(brand = "table")]
pub const BRAND_NAME: &str = "tAbLeOS";
#[cfg(brand = "desk")]
pub const BRAND_NAME: &str = "DesKOS";
#[cfg(not(any(brand = "table", brand = "desk")))]
pub const BRAND_NAME: &str = "tAbLeOS";

#[cfg(brand = "desk")]
pub const BRAND_LICENSE: &str = "Open source under the MIT License";
#[cfg(not(brand = "desk"))]
pub const BRAND_LICENSE: &str = "Closed source, all rights reserved";

pub const VERSION: &str = "0.3.0";

#[no_mangle]
extern "C" fn kernel_main(bootinfo_ptr: *const BootInfo) -> ! {
    // Set bootinfo BEFORE console::init — console::init reads bootinfo
    // for fb_phys/fb_width/etc. Without this, BOOTINFO_PTR is null and
    // the first FB write dereferences a null pointer → immediate crash.
    bootinfo::set(bootinfo_ptr);

    console::init();
    println!("{} kernel {} booting", BRAND_NAME, VERSION);
    println!("{}", BRAND_LICENSE);

    let bi = unsafe { &*bootinfo_ptr };
    if bi.magic != bootinfo::MAGIC {
        println!("FATAL: bad bootinfo magic {:#x}", bi.magic);
        loop { unsafe { core::arch::asm!("hlt") } }
    }

    println!(
        "bootinfo: kernel_size={} e820_count={} vbe={:#x} fb={:#x} {}x{}x{}",
        bi.kernel_size, bi.e820_count, bi.vbe_mode,
        bi.fb_phys, bi.fb_width, bi.fb_height, bi.fb_bpp
    );

    gdt::init();
    println!("gdt: ok");

    idt::init();
    println!("idt: ok");
    idt::init_syscall();
    vmm::fpu_init(); // SSE was #UD in ring3 (CR0.EM set by stage2)

    pic::init();
    pit::init();
    println!("pic/pit: ok (100Hz)");

    pmm::init(bi);

    // clear framebuffer to black — VBE set_mode leaves random content,
    // and server edition has no userland GUI to paint it
    let fb_bytes = (bi.fb_width as usize) * (bi.fb_height as usize) * (bi.fb_bpp as usize / 8);
    if fb_bytes > 0 {
        let fb_ptr = bi.fb_phys as *mut u8;
        unsafe {
            core::ptr::write_bytes(fb_ptr, 0, fb_bytes);
        }
    }

    heap::init();
    println!("heap: arena {:#x}-{:#x}", heap::HEAP_START, heap::HEAP_MAX);

    // disk + filesystem
    ata::init();
    match e1fss::init() {
        Ok(()) => println!("e1fss: mounted"),
        Err(e) => println!("e1fss: NOT mounted ({})", e),
    }

    keyboard::init();
    #[cfg(not(edition = "server"))]
    mouse::init(bi.fb_width as i32, bi.fb_height as i32);

    // heap sanity through the full path
    let mut v: Vec<u64> = Vec::new();
    for i in 0..100u64 {
        v.push(i * 3);
    }
    let b = Box::new(v.len());
    assert_eq!(*b, 100);
    drop(v);
    drop(b);
    println!("heap: OK");

    sched::init();
    println!("sched: idle ready");

    unsafe { core::arch::asm!("sti") };
    println!("interrupts enabled");

    // boot the shell if the filesystem is available
    if e1fss::is_mounted() {
        match e1fss::read_file("ush").or_else(|_| e1fss::read_file("/ush")) {
            Ok(data) => {
                let pid = sched::spawn_user(b"ush", &data, 0, 0, None);
                println!("spawn ush -> pid={:?}", pid);
            }
            Err(_) => println!("no ush binary in fs"),
        }
    }

    // become the idle loop (slot 0 context): wait for interrupts, respawn
    // the shell whenever no user process is left
    loop {
        unsafe { core::arch::asm!("hlt") };
        let mut any_user = false;
        // read scheduler state via its public API
        for pid in 1..64usize {
            if let Some(s) = sched::state_of(pid) {
                if s != sched::State::Zombie {
                    any_user = true;
                    break;
                }
            }
        }
        if !any_user && e1fss::is_mounted() {
            if let Ok(data) = e1fss::read_file("ush").or_else(|_| e1fss::read_file("/ush")) {
                let _ = sched::spawn_user(b"ush", &data, 0, 0, None);
            }
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("KERNEL PANIC: {}", info);
    loop { unsafe { core::arch::asm!("cli; hlt") } }
}
