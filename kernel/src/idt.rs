//! IDT: exception dump, IRQ routing, syscall gate.

use crate::{println, gdt};

// Assemble the ISR stubs (integrated assembler, GNU syntax .S file).
core::arch::global_asm!(include_str!("isr.S"));

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    off0: u16,
    sel: u16,
    ist: u8,
    flags: u8,
    off1: u16,
    off2: u32,
    zero: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        IdtEntry { off0: 0, sel: 0, ist: 0, flags: 0, off1: 0, off2: 0, zero: 0 }
    }
    fn set(&mut self, handler: u64, dpl: u8) {
        self.off0 = handler as u16;
        self.off1 = (handler >> 16) as u16;
        self.off2 = (handler >> 32) as u32;
        self.sel = gdt::KERNEL_CS;
        self.ist = 0;
        self.flags = 0x8E | ((dpl & 3) << 5); // present, interrupt gate
        self.zero = 0;
    }
}

#[repr(C, packed)]
struct IdtPtr {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::missing(); 256];

extern "C" {
    static isr_stub_table: [u64; 49]; // 0-47 + 128
    static syscall_entry: u8; // asm trampoline for the `syscall` instruction
}

/// Enable the `syscall`/`sysret` instructions for e1wine Linux binaries.
/// EFER.SCE + STAR (kernel CS 0x08, SS 0x10 via +8) + LSTAR + SFMASK
/// (entry with IF=0). Returns still go through iretq; sysret is never used.
pub fn init_syscall() {
    const EFER: u32 = 0xC000_0080;
    const STAR: u32 = 0xC000_0081;
    const LSTAR: u32 = 0xC000_0082;
    const SFMASK: u32 = 0xC000_0084;
    unsafe {
        let entry = core::ptr::addr_of!(syscall_entry) as u64;
        let efer = crate::vmm::read_msr(EFER);
        crate::vmm::write_msr(EFER, efer | 1); // SCE
        // STAR: SYSCALL CS=STAR[47:32], SS=+8; SYSRET base (unused) 0x0B
        crate::vmm::write_msr(STAR, (0x0Bu64 << 48) | (0x08u64 << 32));
        crate::vmm::write_msr(LSTAR, entry);
        // clear all RFLAGS except bit 1 (always 1) -> IF=0 in kernel entry
        crate::vmm::write_msr(SFMASK, 0xFFFF_FFFF_FFFF_FFFD);
    }
    println!("idt: syscall insn enabled");
}

/// Register frame pushed by isr_common (see isr.S), RSP points here in handler.
#[repr(C)]
pub struct IsrFrame {
    // pushed by isr_common (reverse order of push)
    pub rax: u64, pub rbx: u64, pub rcx: u64, pub rdx: u64,
    pub rdi: u64, pub rsi: u64, pub rbp: u64,
    pub r8: u64, pub r9: u64, pub r10: u64, pub r11: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
    pub vector: u64,
    pub error: u64,
    // CPU frame
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

static EXCEPTION_NAMES: [&str; 32] = [
    "#DE Divide Error", "#DB Debug", "NMI", "#BP Breakpoint",
    "#OF Overflow", "#BR Bound Range", "#UD Invalid Opcode", "#NM No FPU",
    "#DF Double Fault", "Coprocessor", "#TS Invalid TSS", "#NP Segment Missing",
    "#SS Stack Fault", "#GP General Protection", "#PF Page Fault", "Reserved",
    "#MF x87 FP", "#AC Alignment Check", "#MC Machine Check", "#XM SIMD FP",
    "#VE Virtualization", "#CP Control Protection", "Reserved", "Reserved",
    "Reserved", "Reserved", "Reserved", "#HV Hypervisor", "#VC VMM Comm",
    "#SX Security", "Reserved", "Reserved",
];

pub fn init() {
    unsafe {
        let table = core::ptr::addr_of!(isr_stub_table) as *const u64;
        for v in 0..48usize {
            IDT[v].set(*table.add(v), 0);
        }
        IDT[0x80].set(*table.add(48), 3); // syscall callable from ring3

        let ptr = IdtPtr {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        core::arch::asm!("lidt [{0}]", in(reg) &ptr);
    }
}

#[no_mangle]
extern "C" fn isr_dispatch(frame: &mut IsrFrame) {
    let v = frame.vector as usize;
    match v {
        0..=31 => {
            let cr2: u64;
            unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem)) };
            println!("EXCEPTION {} err={:#x} cr2={:#x}", EXCEPTION_NAMES[v], frame.error, cr2);
            println!(
                "  rip={:#018x} cs={:#x} rflags={:#x} rsp={:#018x} ss={:#x}",
                frame.rip, frame.cs, frame.rflags, frame.rsp, frame.ss
            );
            println!(
                "  rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x}",
                frame.rax, frame.rbx, frame.rcx, frame.rdx
            );
            println!(
                "  rdi={:#x} rsi={:#x} rbp={:#x} r8={:#x} r9={:#x}",
                frame.rdi, frame.rsi, frame.rbp, frame.r8, frame.r9
            );
            println!(
                "  r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
                frame.r10, frame.r11, frame.r12, frame.r13, frame.r14, frame.r15
            );
            if frame.cs & 3 == 3 {
                // user process fault: kill it
                #[cfg(has_sched)]
                crate::sched::kill_current();
                #[cfg(not(has_sched))]
                println!("ring3 fault (no scheduler)");
            } else {
                println!("kernel exception, halting");
                loop { unsafe { core::arch::asm!("cli; hlt") } }
            }
        }
        32 => crate::pit::irq_handler(frame),
        33 => crate::keyboard::irq_handler(),
        44 => crate::mouse::irq_handler(),
        0x80 => {
            #[cfg(has_sched)]
            crate::syscall::dispatch(frame);
            #[cfg(not(has_sched))]
            println!("syscall ignored (no scheduler)");
        }
        _ => {}
    }
    if (32..=47).contains(&v) {
        crate::pic::eoi(v as u8 - 32);
    }
}
