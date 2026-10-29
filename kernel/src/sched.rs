//! Preemptive round-robin scheduler: one idle kernel process (slot 0) plus
//! ring3 user processes. Context switch replaces the ISR frame registers; on
//! iretq the CPU resumes the target process.

use crate::idt::IsrFrame;
use crate::pmm;
use crate::println;
use crate::vmm;
use crate::gdt;
use alloc::vec::Vec;

const MAX_PROC: usize = 64;
const KERNEL_STACK_PAGES: usize = 2;
const USER_STACK_PAGES: usize = 4;
const IDLE_STACK_PAGES: usize = 2;

// Linux-ABI process memory regions (e1wine)
const BRK_BASE: u64 = 0x0500_0000;
const BRK_MAX: u64 = 0x0580_0000;
const MMAP_BASE: u64 = 0x0600_0000;
const MMAP_MAX: u64 = 0x0700_0000;

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Ready,
    Running,
    Blocked,
    Zombie,
}

#[repr(C)]
pub struct Context {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl Context {
    const fn zero() -> Self {
        Context {
            rax: 0, rbx: 0, rcx: 0, rdx: 0, rdi: 0, rsi: 0, rbp: 0,
            r8: 0, r9: 0, r10: 0, r11: 0, r12: 0, r13: 0, r14: 0, r15: 0,
            rip: 0, cs: 0, rflags: 0x202, rsp: 0, ss: 0,
        }
    }
}

pub struct Process {
    pub pid: usize,
    pub state: State,
    pub ctx: Context,
    pub pml4: u64,
    pub kstack_top: u64,
    pub name: [u8; 32],
    pub exit_code: i32,
    pub parent: usize,
    // e1wine / Linux-ABI state
    pub is_linux: bool,
    pub fs_base: u64,
    pub brk: u64,
    pub mmap_next: u64,
    // SSE/FP state (fxsave image, 16-byte aligned)
    pub fpu: FpuArea,
    pub fpu_saved: bool,
}

/// FXSAVE/FXRSTOR image; must be 16-byte aligned or the CPU raises #GP.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FpuArea(pub [u8; 512]);

static mut INIT_FPU: FpuArea = FpuArea([0; 512]);

pub fn fpu_save(area: &mut FpuArea) {
    unsafe {
        core::arch::asm!("fxsave [{}]", in(reg) area.0.as_mut_ptr(), options(nostack));
    }
}

pub fn fpu_restore(area: &FpuArea) {
    unsafe {
        core::arch::asm!("fxrstor [{}]", in(reg) area.0.as_ptr(), options(nostack, readonly));
    }
}

static mut PROCS: [Option<Process>; MAX_PROC] = [const { None }; MAX_PROC];
static mut CURRENT: usize = 0;

// idle process stack (identity-mapped .bss, no pmm needed)
static mut IDLE_STACK: [u8; IDLE_STACK_PAGES * 4096] = [0; IDLE_STACK_PAGES * 4096];

fn idle_loop() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt") };
        // auto-respawn the shell when there are no user processes left
        let mut any_user = false;
        unsafe {
            for p in PROCS.iter().skip(1) {
                if let Some(p) = p {
                    if p.state != State::Zombie {
                        any_user = true;
                        break;
                    }
                }
            }
        }
        if !any_user {
            if let Some(data) = crate::e1fss::read_file("/ush").ok().or_else(|| crate::e1fss::read_file("ush").ok()) {
                let _ = spawn_user(b"ush", &data, 0, 0, None);
            }
        }
    }
}

pub fn init() {
    unsafe {
        // capture a clean default FPU/SSE state for first-time dispatch
        core::arch::asm!("fninit", options(nostack));
        let mxcsr: u32 = 0x1F80;
        core::arch::asm!("ldmxcsr [{}]", in(reg) &mxcsr as *const u32, options(nostack));
        fpu_save(&mut *core::ptr::addr_of_mut!(INIT_FPU));

        let stack_top = core::ptr::addr_of!(IDLE_STACK) as u64 + (IDLE_STACK_PAGES * 4096) as u64;
        let ctx = Context {
            rip: idle_loop as u64,
            cs: gdt::KERNEL_CS as u64,
            rflags: 0x202,
            rsp: stack_top,
            ss: gdt::KERNEL_DS as u64,
            ..Context::zero()
        };
        PROCS[0] = Some(Process {
            pid: 0,
            state: State::Running,
            ctx,
            pml4: vmm::current_cr3(),
            kstack_top: stack_top,
            name: copy_name(b"idle"),
            exit_code: 0,
            parent: 0,
            is_linux: false,
            fs_base: 0,
            brk: 0,
            mmap_next: 0,
            fpu: FpuArea([0; 512]),
            fpu_saved: false,
        });
        CURRENT = 0;
        // seed the `syscall` trampoline's ring0 stack before any user runs
        gdt::set_kernel_stack(stack_top);
    }
}

// Cross-CGU callers (linux_compat, syscall) inline these tiny fns, which
// leaves dangling references to the hidden CURRENT/PROCS symbols at link
// time — so never inline anything that touches scheduler statics.
#[inline(never)]
pub fn current_pid() -> usize {
    unsafe { CURRENT }
}

#[inline(never)]
pub fn current() -> Option<&'static mut Process> {
    unsafe {
        let pid = CURRENT;
        PROCS[pid].as_mut()
    }
}

/// Address space of the running process (for fb_map and similar syscalls
/// that patch the caller's page tables).
#[inline(never)]
pub fn current_pml4() -> u64 {
    unsafe {
        let pid = CURRENT;
        PROCS[pid].as_ref().map_or(0, |p| p.pml4)
    }
}

fn alloc_pid() -> Option<usize> {
    unsafe {
        for i in 1..MAX_PROC {
            if PROCS[i].is_none() {
                return Some(i);
            }
        }
        None
    }
}

fn alloc_kstack() -> Option<u64> {
    let mut first = None;
    for _ in 0..KERNEL_STACK_PAGES {
        let p = pmm::alloc_page()?;
        if first.is_none() {
            first = Some(p);
        }
    }
    Some((first? + KERNEL_STACK_PAGES * pmm::PAGE_SIZE) as u64)
}

fn alloc_ustack(pml4: u64) -> Option<(u64, usize)> {
    let stack_top = 0x7FFF_FFF0_0000u64;
    // map the page containing stack_top too, so a first-instruction `pop`
    // (reads [rsp] before decrementing) does not fault
    let mut first_pa = 0usize;
    for i in 0..USER_STACK_PAGES {
        let p = pmm::alloc_page()?;
        if i == 0 {
            first_pa = p;
        }
        let va = stack_top - i as u64 * pmm::PAGE_SIZE as u64;
        if !vmm::map_page(pml4, va, p as u64, vmm::PTE_RW | vmm::PTE_USER) {
            return None;
        }
    }
    Some((stack_top, first_pa))
}

fn copy_name(name: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let len = name.len().min(31);
    out[..len].copy_from_slice(&name[..len]);
    out
}

/// Map and fill all PT_LOAD segments. Pages are zeroed by pmm::alloc_page,
/// so BSS (memsz > filesz) starts zeroed.
fn load_segments(pml4: u64, elf_data: &[u8], elf: &crate::elf::Elf) -> Option<()> {
    for ph in elf.load_phdrs.iter().take(elf.nloads) {
        let flags = vmm::PTE_USER | vmm::PTE_RW;
        let seg_start_page = ph.vaddr & !0xFFF;
        let seg_end = ph.vaddr + ph.memsz;
        let seg_end_page = (seg_end + 0xFFF) & !0xFFF;
        for page in (seg_start_page..seg_end_page).step_by(pmm::PAGE_SIZE) {
            // map_page_shared keeps an existing user mapping when segments
            // share a 4KB page (rodata tail + data head); huge-page mirrors
            // are supervisor-only and still get replaced
            if !vmm::map_page_shared(pml4, page, flags) {
                return None;
            }
        }
        // copy filesz bytes page by page (handles unaligned vaddr)
        let mut src = ph.offset as usize;
        let mut dst_va = ph.vaddr;
        let mut remaining = ph.filesz as usize;
        while remaining > 0 {
            let page_va = dst_va & !0xFFF;
            let in_page = (dst_va & 0xFFF) as usize;
            let chunk = remaining.min(pmm::PAGE_SIZE - in_page);
            let pa = vmm::translate(pml4, page_va)?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    elf_data.as_ptr().add(src),
                    (pa + in_page as u64) as *mut u8,
                    chunk,
                );
            }
            src += chunk;
            dst_va += chunk as u64;
            remaining -= chunk;
        }
    }
    Some(())
}

/// Spawn a native user ELF. `arg` (optional) is copied into the top of the
/// still-empty user stack and its VA is passed to the child in rdi; a child
/// that wants it declares `extern "C" fn _start(arg: u64)`.
pub fn spawn_user(
    name: &[u8],
    elf_data: &[u8],
    elf_base: u64,
    parent: usize,
    arg: Option<&[u8]>,
) -> Option<usize> {
    let pid = alloc_pid()?;
    let pml4 = vmm::new_user_pml4()?;
    let kstack_top = alloc_kstack()?;
    let (ustack_top, first_pa) = alloc_ustack(pml4)?;

    let elf = crate::elf::Elf::parse(elf_data)?;
    load_segments(pml4, elf_data, &elf)?;

    // place the argument string near the top of the first mapped stack page;
    // rsp starts at ustack_top so pushes never reach above it
    let mut arg_va = 0u64;
    if let Some(a) = arg {
        let len = a.len().min(120);
        unsafe {
            // identity-mapped PA -> kernel VA
            let dst = (first_pa + 64) as *mut u8;
            core::ptr::copy_nonoverlapping(a.as_ptr(), dst, len);
            *dst.add(len) = 0;
        }
        arg_va = ustack_top + 64;
    }

    let ctx = Context {
        rdi: arg_va,
        rflags: 0x202,
        rip: elf_base + elf.entry,
        cs: gdt::USER_CS as u64,
        rsp: ustack_top,
        ss: gdt::USER_DS as u64,
        ..Context::zero()
    };
    unsafe {
        PROCS[pid] = Some(Process {
            pid,
            state: State::Ready,
            ctx,
            pml4,
            kstack_top,
            name: copy_name(name),
            exit_code: 0,
            parent,
            is_linux: false,
            fs_base: 0,
            brk: 0,
            mmap_next: 0,
            fpu: FpuArea([0; 512]),
            fpu_saved: false,
        });
    }
    Some(pid)
}

// Linux auxv keys used to build the initial stack.
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_FLAGS: u64 = 8;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_HWCAP: u64 = 16;
const AT_CLKTCK: u64 = 17;
const AT_SECURE: u64 = 23;
const AT_RANDOM: u64 = 25;
const AT_EXECFN: u64 = 31;

/// Build a Linux-ABI initial stack (argc/argv/envp/auxv + strings) inside the
/// already-mapped user stack, musl/glibc-style. Returns the new rsp.
fn build_linux_stack(
    pml4: u64,
    ustack_top: u64,
    elf: &crate::elf::Elf,
    argv: &[&[u8]],
) -> Option<u64> {
    // vaddr of the program header table (musl walks it for PT_TLS)
    let mut phdr_va = 0u64;
    for ph in elf.load_phdrs.iter().take(elf.nloads) {
        if elf.phoff >= ph.offset && elf.phoff < ph.offset + ph.filesz {
            phdr_va = ph.vaddr + (elf.phoff - ph.offset);
            break;
        }
    }

    // strings area layout (after the pointer block):
    //   [argv strings + NULs][16 random bytes]["PATH=/\0"]
    let auxv_pairs = 17usize;
    let ptr_area = 8 * (2 + argv.len() + 2 + auxv_pairs * 2); // argc+argv+NULL, envp+NULL, auxv
    let mut str_area: Vec<u8> = Vec::new();
    let mut arg_offs: Vec<u64> = Vec::new();
    for a in argv {
        arg_offs.push(ptr_area as u64 + str_area.len() as u64);
        str_area.extend_from_slice(a);
        str_area.push(0);
    }
    let rand_off = ptr_area + str_area.len();
    str_area.resize(str_area.len() + 16, 0);
    let env_bytes = b"PATH=/\0";
    let env_off = ptr_area + str_area.len();
    str_area.extend_from_slice(env_bytes);
    let total = ((ptr_area + str_area.len()) + 15) & !15;

    let base = ustack_top - total as u64;
    let str_base = base + ptr_area as u64;
    let rand_ptr = base + rand_off as u64;
    let env_ptr = base + env_off as u64;

    let mut v: Vec<u8> = Vec::new();
    let mut push8 = |x: u64| v.extend_from_slice(&x.to_le_bytes());

    push8(argv.len() as u64); // argc
    for &off in &arg_offs {
        push8(base + off); // arg_offs are already absolute from base (ptr_area + str offset)
    }
    push8(0); // argv NULL
    push8(env_ptr); // envp[0]
    push8(0); // envp NULL
    push8(AT_PHDR);
    push8(phdr_va);
    push8(AT_PHENT);
    push8(56);
    push8(AT_PHNUM);
    push8(elf.phnum as u64);
    push8(AT_PAGESZ);
    push8(4096);
    push8(AT_BASE);
    push8(0);
    push8(AT_FLAGS);
    push8(0);
    push8(AT_ENTRY);
    push8(elf.entry);
    push8(AT_UID);
    push8(0);
    push8(AT_EUID);
    push8(0);
    push8(AT_GID);
    push8(0);
    push8(AT_EGID);
    push8(0);
    push8(AT_HWCAP);
    push8(0);
    push8(AT_CLKTCK);
    push8(100);
    push8(AT_SECURE);
    push8(0);
    push8(AT_RANDOM);
    push8(rand_ptr);
    push8(AT_EXECFN);
    push8(str_base); // argv[0]
    push8(AT_NULL);
    push8(0);
    debug_assert_eq!(v.len(), ptr_area);

    v.extend_from_slice(&str_area);
    // 16 pseudo-random bytes from the TSC (used for stack protector)
    for i in 0..16usize {
        let lo: u32;
        let hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)) };
        v[rand_off + i] = (lo as u8) ^ (hi as u8).wrapping_add(i as u8);
    }
    v.resize(total, 0); // pad to the 16-byte aligned total
    debug_assert_eq!(v.len(), total);

    // copy into the user stack across page boundaries
    let mut off = 0usize;
    while off < v.len() {
        let va = base + off as u64;
        let page_va = va & !0xFFF;
        let in_page = (va & 0xFFF) as usize;
        let chunk = (v.len() - off).min(pmm::PAGE_SIZE - in_page);
        let pa = vmm::translate(pml4, page_va)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                v.as_ptr().add(off),
                (pa + in_page as u64) as *mut u8,
                chunk,
            );
        }
        off += chunk;
    }
    Some(base)
}

/// Spawn a statically-linked Linux x86_64 ELF under e1wine translation.
/// argv[0] is conventionally the program name.
pub fn spawn_linux(argv: &[&[u8]], elf_data: &[u8], parent: usize) -> Option<usize> {
    if argv.is_empty() {
        return None;
    }
    macro_rules! step {
        ($name:expr, $e:expr) => {
            match $e {
                Some(v) => v,
                None => {
                    crate::println!("spawn_linux: failed at {}", $name);
                    return None;
                }
            }
        };
    }
    let pid = step!("alloc_pid", alloc_pid());
    let pml4 = step!("new_user_pml4", vmm::new_user_pml4());
    let kstack_top = step!("alloc_kstack", alloc_kstack());
    let (ustack_top, _first_pa) = step!("alloc_ustack", alloc_ustack(pml4));

    let elf = step!("elf_parse", crate::elf::Elf::parse(elf_data));
    step!("load_segments", load_segments(pml4, elf_data, &elf));
    let rsp = step!("build_stack", build_linux_stack(pml4, ustack_top, &elf, argv));

    let ctx = Context {
        rflags: 0x202,
        rip: elf.entry,
        cs: gdt::USER_CS as u64,
        rsp,
        ss: gdt::USER_DS as u64,
        ..Context::zero()
    };
    unsafe {
        PROCS[pid] = Some(Process {
            pid,
            state: State::Ready,
            ctx,
            pml4,
            kstack_top,
            name: copy_name(argv[0]),
            exit_code: 0,
            parent,
            is_linux: true,
            fs_base: 0,
            brk: BRK_BASE,
            mmap_next: MMAP_BASE,
            fpu: FpuArea([0; 512]),
            fpu_saved: false,
        });
    }
    Some(pid)
}

/// Called from the timer ISR. Saves the running process, reaps zombies,
/// wakes blocked parents, and loads the next ready process into `frame`.
pub fn tick(frame: &mut IsrFrame) {
    unsafe {
        let cur = CURRENT;

        // save current state if it was running; a preempted Running process
        // must go back to Ready or it can never be scheduled again
        if let Some(ref mut p) = PROCS[cur] {
            if p.state == State::Running {
                snapshot_into(p, frame);
                p.state = State::Ready;
                p.fs_base = vmm::read_msr(vmm::MSR_FSBASE);
            }
        }

        // pick next ready process (slot 0 = idle as fallback)
        let mut next = 0usize;
        for i in 1..MAX_PROC {
            if let Some(p) = PROCS[i].as_ref() {
                if p.state == State::Ready {
                    next = i;
                    break;
                }
            }
        }

        let Some(new_proc) = PROCS[next].as_mut() else { return };
        if next == cur && new_proc.state == State::Running {
            return; // nothing to switch
        }
        new_proc.state = State::Running;
        CURRENT = next;
        gdt::set_kernel_stack(new_proc.kstack_top);
        vmm::write_msr(vmm::MSR_FSBASE, new_proc.fs_base);

        if new_proc.pml4 != vmm::current_cr3() {
            vmm::load_cr3(new_proc.pml4);
        }

        // reap zombie AFTER switching CR3 away from its address space
        if let Some(p) = PROCS[cur].as_ref() {
            if p.state == State::Zombie && p.pid != 0 {
                let parent = p.parent;
                let pml4 = p.pml4;
                vmm::destroy_user_space(pml4);
                PROCS[cur] = None;
                if parent != cur {
                    if let Some(ref mut par) = PROCS[parent] {
                        if par.state == State::Blocked {
                            par.state = State::Ready;
                        }
                    }
                }
            }
        }

        // load the target's FPU/SSE state (clean defaults on its first run)
        if new_proc.fpu_saved {
            fpu_restore(&new_proc.fpu);
        } else {
            fpu_restore(&*core::ptr::addr_of!(INIT_FPU));
        }

        let ctx = &new_proc.ctx;
        frame.rax = ctx.rax;
        frame.rbx = ctx.rbx;
        frame.rcx = ctx.rcx;
        frame.rdx = ctx.rdx;
        frame.rdi = ctx.rdi;
        frame.rsi = ctx.rsi;
        frame.rbp = ctx.rbp;
        frame.r8 = ctx.r8;
        frame.r9 = ctx.r9;
        frame.r10 = ctx.r10;
        frame.r11 = ctx.r11;
        frame.r12 = ctx.r12;
        frame.r13 = ctx.r13;
        frame.r14 = ctx.r14;
        frame.r15 = ctx.r15;
        frame.rip = ctx.rip;
        frame.cs = ctx.cs;
        frame.rflags = ctx.rflags;
        frame.rsp = ctx.rsp;
        frame.ss = ctx.ss;
    }
}

/// Trigger a reschedule (used by block/exit/kill from a syscall context).
fn reschedule() {
    unsafe { core::arch::asm!("int 0x20") };
}

/// Voluntary yield: snapshot current state, mark Ready, reschedule.
pub fn yield_current(frame: &IsrFrame) {
    unsafe {
        snapshot_into(PROCS[CURRENT].as_mut().unwrap(), frame);
        PROCS[CURRENT].as_mut().unwrap().state = State::Ready;
    }
    reschedule();
}

/// Unconditionally copy `frame` into the current process's saved context.
pub fn snapshot_current(frame: &IsrFrame) {
    unsafe {
        if let Some(ref mut p) = PROCS[CURRENT] {
            snapshot_into(p, frame);
        }
    }
}

fn snapshot_into(p: &mut Process, frame: &IsrFrame) {
    p.ctx.rax = frame.rax;
    p.ctx.rbx = frame.rbx;
    p.ctx.rcx = frame.rcx;
    p.ctx.rdx = frame.rdx;
    p.ctx.rdi = frame.rdi;
    p.ctx.rsi = frame.rsi;
    p.ctx.rbp = frame.rbp;
    p.ctx.r8 = frame.r8;
    p.ctx.r9 = frame.r9;
    p.ctx.r10 = frame.r10;
    p.ctx.r11 = frame.r11;
    p.ctx.r12 = frame.r12;
    p.ctx.r13 = frame.r13;
    p.ctx.r14 = frame.r14;
    p.ctx.r15 = frame.r15;
    p.ctx.rip = frame.rip;
    p.ctx.cs = frame.cs;
    p.ctx.rflags = frame.rflags;
    p.ctx.rsp = frame.rsp;
    p.ctx.ss = frame.ss;
    // the kernel is built soft-float, so the FPU/SSE registers still belong
    // to the interrupted process — capture them here before anyone else runs
    fpu_save(&mut p.fpu);
    p.fpu_saved = true;
}

pub fn block_current() {
    unsafe {
        if let Some(ref mut p) = PROCS[CURRENT] {
            p.state = State::Blocked;
        }
    }
    reschedule();
}

pub fn kill_current() {
    unsafe {
        if let Some(ref mut p) = PROCS[CURRENT] {
            p.state = State::Zombie;
            p.exit_code = -1;
        }
    }
    reschedule();
}

pub fn exit(code: i32) {
    // the space is freed by tick's zombie reaper after the CR3 switch
    unsafe {
        if let Some(ref mut p) = PROCS[CURRENT] {
            p.state = State::Zombie;
            p.exit_code = code;
        }
    }
    reschedule();
}

pub fn list(buf: &mut [(usize, [u8; 32], State)]) -> usize {
    unsafe {
        let mut n = 0;
        for p in PROCS.iter() {
            if let Some(p) = p {
                if n < buf.len() {
                    buf[n] = (p.pid, p.name, p.state);
                    n += 1;
                }
            }
        }
        n
    }
}

#[inline(never)]
pub fn state_of(pid: usize) -> Option<State> {
    unsafe { PROCS.get(pid).and_then(|p| p.as_ref().map(|p| p.state)) }
}
