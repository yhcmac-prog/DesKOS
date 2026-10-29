//! e1wine: Linux syscall ABI translation layer for statically-linked x86_64
//! ELF binaries (musl/glibc static). Translates Linux syscall numbers and
//! semantics onto tAbLeOS kernel primitives. Enabled per-process via the
//! `is_linux` flag set by sched::spawn_linux.

use crate::idt::IsrFrame;
use crate::sched;
use crate::vmm;

// Linux x86_64 syscall numbers
const L_READ: u64 = 0;
const L_WRITE: u64 = 1;
const L_OPEN: u64 = 2;
const L_CLOSE: u64 = 3;
const L_STAT: u64 = 4;
const L_BRK: u64 = 12;
const L_IOCTL: u64 = 16;
const L_WRITEV: u64 = 20;
const L_MMAP: u64 = 9;
const L_MPROTECT: u64 = 10;
const L_MUNMAP: u64 = 11;
const L_GETPID: u64 = 39;
const L_UNAME: u64 = 63;
const L_GETCWD: u64 = 79;
const L_CHDIR: u64 = 80;
const L_GETUID: u64 = 102;
const L_GETGID: u64 = 104;
const L_GETEUID: u64 = 107;
const L_GETEGID: u64 = 108;
const L_GETTIMEOFDAY: u64 = 96;
const L_GETPPID: u64 = 110;
const L_FCNTL: u64 = 72;
const L_ARCH_PRCTL: u64 = 158;
const L_SET_TID_ADDRESS: u64 = 218;
const L_EXIT: u64 = 60;
const L_EXIT_GROUP: u64 = 231;

// arch_prctl subcommands
const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;

// e1wine memory regions (must match sched::spawn_linux defaults)
const BRK_MAX: u64 = 0x0580_0000;
const MMAP_MAX: u64 = 0x0700_0000;

/// Dispatch a Linux syscall for the current (is_linux) process.
pub fn linux_dispatch(frame: &mut IsrFrame) -> i64 {
    match frame.rax {
        L_WRITE => {
            sys_write(frame.rdi as usize, frame.rsi as *const u8, frame.rdx as usize)
        }
        L_WRITEV => {
            let iov = frame.rsi as *const u64;
            let cnt = frame.rdx as usize;
            let mut total: i64 = 0;
            for i in 0..cnt {
                let base = unsafe { *iov.add(i * 2) } as *const u8;
                let len = unsafe { *iov.add(i * 2 + 1) } as usize;
                total += sys_write(1, base, len);
            }
            total
        }
        L_READ => -1,
        L_OPEN => {
            let _path = frame.rdi as *const u8;
            -1 // no user-space fd table yet; uname does not need open
        }
        L_CLOSE => 0,
        L_BRK => do_brk(frame.rdi),
        L_MMAP => do_mmap(frame.rdi, frame.rsi),
        L_MUNMAP | L_MPROTECT => 0,
        L_IOCTL => -1, // not a tty -> musl switches stdout to block buffering
        L_FCNTL => 0,
        L_EXIT | L_EXIT_GROUP => {
            sched::exit(frame.rdi as i32);
            0
        }
        L_UNAME => {
            let buf = frame.rdi as *mut u8;
            unsafe {
                // struct utsname: 6 fields x 65 bytes (musl)
                core::ptr::write_bytes(buf, 0, 6 * 65);
                let fields: [&[u8]; 6] = [
                    b"tAbLeOS\0",            // sysname
                    b"tAbLeOS\0",            // nodename
                    b"1.0.0-e1wine\0",       // release
                    b"#1 e1wine ABI\0",      // version
                    b"x86_64\0",             // machine
                    b"(none)\0",             // domainname
                ];
                for (i, f) in fields.iter().enumerate() {
                    core::ptr::copy_nonoverlapping(f.as_ptr(), buf.add(i * 65), f.len());
                }
            }
            0
        }
        L_GETPID => sched::current_pid() as i64,
        L_GETPPID => 0,
        L_GETUID | L_GETGID | L_GETEUID | L_GETEGID => 0,
        L_GETCWD => {
            let buf = frame.rdi as *mut u8;
            let cwd = b"/\0";
            let l = cwd.len().min(frame.rsi as usize);
            unsafe { core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf, l) };
            l as i64
        }
        L_CHDIR => 0,
        L_GETTIMEOFDAY => {
            let tv = frame.rdi as *mut u64;
            if !tv.is_null() {
                unsafe {
                    let ms = crate::pit::ticks() * 10;
                    *tv = ms / 1000;
                    *tv.add(1) = (ms % 1000) * 1000;
                }
            }
            0
        }
        L_ARCH_PRCTL => {
            let code = frame.rdi;
            let arg = frame.rsi;
            match code {
                ARCH_SET_FS => {
                    if let Some(p) = sched::current() {
                        p.fs_base = arg;
                    }
                    vmm::write_msr(vmm::MSR_FSBASE, arg);
                    0
                }
                ARCH_GET_FS => match sched::current() {
                    Some(p) => p.fs_base as i64,
                    None => -1,
                },
                _ => -1,
            }
        }
        L_SET_TID_ADDRESS => sched::current_pid() as i64,
        _ => -1, // silent: musl probes many optional syscalls (rseq, prlimit...)
    }
}

fn sys_write(fd: usize, buf: *const u8, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    match fd {
        1 | 2 => {
            let slice = unsafe { core::slice::from_raw_parts(buf, len) };
            // irq-off: a preempted lock holder would deadlock the next printer
            let (flags, c) = crate::console::CONSOLE.lock_irq();
            for &b in slice {
                c.put_byte(b);
            }
            unsafe { crate::console::CONSOLE.unlock_irq(flags) };
            len as i64
        }
        _ => -1,
    }
}

/// Linux brk: grow the heap by really mapping pages in [cur, addr).
fn do_brk(addr: u64) -> i64 {
    let Some(p) = sched::current() else { return -1 };
    if addr == 0 {
        return p.brk as i64;
    }
    if addr <= p.brk || addr > BRK_MAX {
        return p.brk as i64; // shrink unsupported; failure returns current break
    }
    let pml4 = p.pml4;
    let mut a = (p.brk + 0xFFF) & !0xFFF;
    let end = (addr + 0xFFF) & !0xFFF;
    while a < end {
        let Some(pa) = crate::pmm::alloc_page() else {
            return p.brk as i64;
        };
        if !vmm::map_page(pml4, a, pa as u64, vmm::PTE_USER | vmm::PTE_RW) {
            return p.brk as i64;
        }
        a += 0x1000;
    }
    p.brk = addr;
    addr as i64
}

/// Anonymous mmap: really map the range, allocating at a per-process bump
/// region when no address is requested.
fn do_mmap(addr: u64, len: u64) -> i64 {
    let pages = (len + 0xFFF) / 0x1000;
    if pages == 0 {
        return -1;
    }
    let Some(p) = sched::current() else { return -1 };
    let va = if addr == 0 {
        let v = p.mmap_next;
        if v + pages * 0x1000 > MMAP_MAX {
            return -1;
        }
        p.mmap_next += pages * 0x1000;
        v
    } else {
        addr
    };
    let pml4 = p.pml4;
    for i in 0..pages {
        let Some(pa) = crate::pmm::alloc_page() else {
            return -1;
        };
        if !vmm::map_page(pml4, va + i * 0x1000, pa as u64, vmm::PTE_USER | vmm::PTE_RW) {
            return -1;
        }
    }
    va as i64
}
