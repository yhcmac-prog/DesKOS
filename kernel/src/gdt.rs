//! GDT with kernel/user segments + TSS (for ring3 -> ring0 stack switch).

use core::mem::size_of;

pub const KERNEL_CS: u16 = 0x08;
pub const KERNEL_DS: u16 = 0x10;
pub const USER_CS: u16 = 0x1B;   // ring3, index 3 | RPL3
pub const USER_DS: u16 = 0x23;   // ring3, index 4 | RPL3
pub const TSS_SEL: u16 = 0x28;   // index 5

#[repr(C, packed)]
struct GdtEntry {
    limit0: u16,
    base0: u16,
    base1: u8,
    access: u8,
    gran: u8,
    base2: u8,
}

impl GdtEntry {
    const fn zero() -> Self {
        GdtEntry { limit0: 0, base0: 0, base1: 0, access: 0, gran: 0, base2: 0 }
    }
    const fn new(access: u8, gran: u8) -> Self {
        GdtEntry { limit0: 0xFFFF, base0: 0, base1: 0, access, gran, base2: 0 }
    }
}

// System segment (TSS) is 16 bytes: occupies two GDT slots.
#[repr(C, packed)]
struct TssEntry {
    limit: u16,
    base_low: u16,
    base_mid1: u8,
    access: u8,
    gran: u8,
    base_mid2: u8,
    base_high: u32,
    reserved: u32,
}

#[repr(C, packed)]
pub struct Tss {
    reserved0: u32,
    pub rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    reserved2: u64,
    reserved3: u16,
    iopb: u16,
}

impl Tss {
    const fn new() -> Self {
        Tss {
            reserved0: 0, rsp0: 0, rsp1: 0, rsp2: 0, reserved1: 0,
            ist1: 0, ist2: 0, ist3: 0, ist4: 0, ist5: 0, ist6: 0, ist7: 0,
            reserved2: 0, reserved3: 0, iopb: size_of::<Tss>() as u16,
        }
    }
}

#[repr(C, packed)]
struct Gdt {
    entries: [GdtEntry; 5],
    tss: TssEntry,
}

#[repr(C, packed)]
struct GdtPtr {
    limit: u16,
    base: u64,
}

static mut GDT: Gdt = Gdt {
    entries: [
        GdtEntry::zero(),
        GdtEntry::new(0x9A, 0xAF), // kernel code64
        GdtEntry::new(0x92, 0xCF), // kernel data
        GdtEntry::new(0xFA, 0xAF), // user code64 (DPL3)
        GdtEntry::new(0xF2, 0xCF), // user data (DPL3)
    ],
    tss: TssEntry { limit: 0, base_low: 0, base_mid1: 0, access: 0x89, gran: 0, base_mid2: 0, base_high: 0, reserved: 0 },
};

pub static mut TSS: Tss = Tss::new();

/// Current ring0 stack top, read by the `syscall` trampoline (LSTAR) because
/// the syscall instruction does not switch stacks. Kept in sync by
/// set_kernel_stack.
#[no_mangle]
pub static mut SYSCALL_RSP0: u64 = 0;

pub fn init() {
    unsafe {
        let tss_addr = core::ptr::addr_of!(TSS) as u64;
        let limit = (size_of::<Tss>() - 1) as u16;
        GDT.tss.limit = limit;
        GDT.tss.base_low = (tss_addr & 0xFFFF) as u16;
        GDT.tss.base_mid1 = ((tss_addr >> 16) & 0xFF) as u8;
        GDT.tss.base_mid2 = ((tss_addr >> 24) & 0xFF) as u8;
        GDT.tss.base_high = ((tss_addr >> 32) & 0xFFFF_FFFF) as u32;

        let gdt_ptr = GdtPtr {
            limit: (size_of::<Gdt>() - 1) as u16,
            base: core::ptr::addr_of!(GDT) as u64,
        };
        let tss_sel = TSS_SEL as u64;
        core::arch::asm!(
            "lgdt [{0}]",
            "mov ax, {1}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "push {2}",              // reload CS via far return
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov cx, {3:x}",
            "ltr cx",
            in(reg) &gdt_ptr,
            const KERNEL_DS,
            const KERNEL_CS as u64,
            in(reg) tss_sel,
            options(preserves_flags),
        );
    }
}

/// Set the ring0 stack used when a user process traps into the kernel.
pub fn set_kernel_stack(rsp0: u64) {
    unsafe {
        TSS.rsp0 = rsp0;
        SYSCALL_RSP0 = rsp0;
    }
}
