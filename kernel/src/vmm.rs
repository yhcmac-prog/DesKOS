//! Paging: kernel identity map lives at PML4 0x70000 (built by stage2, 4GB,
//! 2MB pages). Processes get their own PML4 with kernel half shared by copy.

use crate::pmm;

pub const PTE_PRESENT: u64 = 1;
pub const PTE_RW: u64 = 2;
pub const PTE_USER: u64 = 4;
pub const PTE_PCD: u64 = 1 << 4; // page-level cache disable (MMIO regions)
pub const PTE_PS: u64 = 1 << 7;

pub const KERNEL_PML4: u64 = 0x70000;

/// Read current CR3.
pub fn current_cr3() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {0}, cr3", out(reg) v) };
    v
}

pub fn load_cr3(pml4_phys: u64) {
    unsafe { core::arch::asm!("mov cr3, {0}", in(reg) pml4_phys) };
}

/// Create a new user PML4: fresh page with kernel mappings copied.
/// Create a new user PML4. The low 4GB gets a fully independent tree
/// (own PDPT + 4 PDs of 2MB pages, same mapping as the stage2 identity map);
/// nothing is shared with the kernel PML4, so later user mappings split
/// pages on private structures only.
pub fn new_user_pml4() -> Option<u64> {
    let page = pmm::alloc_page()?;
    unsafe {
        let pml4 = page as *mut u64;
        core::ptr::write_bytes(page as *mut u8, 0, 4096);
        // copy higher-half kernel entries (none for now, but keep for future)
        let src = KERNEL_PML4 as *const u64;
        for i in 256..512 {
            let e = *src.add(i);
            *pml4.add(i) = if e & PTE_USER == 0 { e } else { 0 };
        }
        let pdpt = pmm::alloc_page()? as *mut u64;
        core::ptr::write_bytes(pdpt as *mut u8, 0, 4096);
        // NOTE: U/S is ANDed across the whole walk path, so every intermediate
        // entry of the user tree needs PTE_USER or user pages stay supervisor-only
        *pml4.add(0) = pdpt as u64 | PTE_PRESENT | PTE_RW | PTE_USER;
        for j in 0..4usize {
            let pd = pmm::alloc_page()? as *mut u64;
            core::ptr::write_bytes(pd as *mut u8, 0, 4096);
            let base = (j as u64) * 0x4000_0000;
            for k in 0..512usize {
                *pd.add(k) = base + (k as u64) * 0x20_0000 | PTE_PRESENT | PTE_RW | PTE_PS;
            }
            *pdpt.add(j) = pd as u64 | PTE_PRESENT | PTE_RW | PTE_USER;
        }
    }
    Some(page as u64)
}

/// Map a 4KB page `phys` at `virt` in the address space rooted at `pml4_phys`.
/// Intermediate tables are allocated from pmm as needed. If an intermediate
/// entry is a present huge page (stage2 identity map uses 2MB pages), it is
/// split into a 4KB table first — otherwise the huge frame would be misused
/// as a page table and the mapping would silently not take effect.
pub fn map_page(pml4_phys: u64, virt: u64, phys: u64, flags: u64) -> bool {
    map_page_impl(pml4_phys, virt, Some(phys), flags, false)
}

/// Like map_page, but only inserts a new leaf page when the leaf PTE is not
/// already a user mapping. PT_LOAD segments can share a 4KB page (rodata tail
/// + data head); re-mapping such a page with a fresh zero frame would wipe the
/// previous segment's data. Huge-page mirror entries are supervisor-only, so
/// they never count as an existing user mapping and still get replaced.
/// The leaf frame is allocated internally (none is wasted on a shared hit).
pub fn map_page_shared(pml4_phys: u64, virt: u64, flags: u64) -> bool {
    map_page_impl(pml4_phys, virt, None, flags, true)
}

fn map_page_impl(pml4_phys: u64, virt: u64, phys: Option<u64>, flags: u64, shared: bool) -> bool {
    unsafe {
        let pml4 = pml4_phys as *mut u64;
        let i4 = ((virt >> 39) & 0x1FF) as usize;
        let i3 = ((virt >> 30) & 0x1FF) as usize;
        let i2 = ((virt >> 21) & 0x1FF) as usize;
        let i1 = ((virt >> 12) & 0x1FF) as usize;

        let mut e = *pml4.add(i4);
        if e & PTE_PRESENT == 0 {
            let Some(t) = pmm::alloc_page() else { return false };
            e = t as u64 | PTE_PRESENT | PTE_RW | PTE_USER;
            *pml4.add(i4) = e;
        }
        let pdpt = (e & !0xFFF) as *mut u64;

        let mut e = *pdpt.add(i3);
        if e & PTE_PRESENT == 0 {
            let Some(t) = pmm::alloc_page() else { return false };
            e = t as u64 | PTE_PRESENT | PTE_RW | PTE_USER;
            *pdpt.add(i3) = e;
        } else if e & PTE_PS != 0 {
            // split a 1GB page into a PD of 2MB entries
            let Some(t) = pmm::alloc_page() else { return false };
            let base = e & !0x3FFF_FFFF;
            let fl = (e & 0xFFF & !PTE_PS) | PTE_PRESENT | PTE_USER;
            let pd = t as *mut u64;
            for j in 0..512usize {
                *pd.add(j) = base + (j as u64) * 0x20_0000 | fl & !PTE_USER | PTE_PS;
            }
            e = t as u64 | fl;
            *pdpt.add(i3) = e;
        }
        let pd = (e & !0xFFF) as *mut u64;

        let mut e = *pd.add(i2);
        if e & PTE_PRESENT == 0 {
            let Some(t) = pmm::alloc_page() else { return false };
            e = t as u64 | PTE_PRESENT | PTE_RW | PTE_USER;
            *pd.add(i2) = e;
        } else if e & PTE_PS != 0 {
            // split a 2MB page into a PT of 4KB entries. The written-back
            // table pointer carries PTE_USER (U/S is ANDed across the walk);
            // the mirrored 4KB entries themselves stay supervisor.
            let Some(t) = pmm::alloc_page() else { return false };
            let base = e & !0x1F_FFFF;
            let fl = (e & 0xFFF & !PTE_PS) | PTE_PRESENT | PTE_USER;
            let pt = t as *mut u64;
            for j in 0..512usize {
                *pt.add(j) = base + (j as u64) * 0x1000 | fl & !PTE_USER;
            }
            e = t as u64 | fl;
            *pd.add(i2) = e;
        }
        let pt = (e & !0xFFF) as *mut u64;

        let old = *pt.add(i1);
        if shared && old & PTE_PRESENT != 0 && old & PTE_USER != 0 {
            return true; // segment-shared page: keep the existing user mapping
        }
        let Some(p) = phys else {
            let Some(p) = pmm::alloc_page() else { return false };
            *pt.add(i1) = p as u64 | flags | PTE_PRESENT;
            return true;
        };
        *pt.add(i1) = p | flags | PTE_PRESENT;
    }
    true
}

/// Translate virt -> phys in an address space (None if unmapped).
pub fn translate(pml4_phys: u64, virt: u64) -> Option<u64> {
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let e4 = *pml4.add(((virt >> 39) & 0x1FF) as usize);
        if e4 & PTE_PRESENT == 0 { return None; }
        let pdpt = (e4 & !0xFFF) as *const u64;
        let e3 = *pdpt.add(((virt >> 30) & 0x1FF) as usize);
        if e3 & PTE_PRESENT == 0 { return None; }
        if e3 & PTE_PS != 0 { return Some((e3 & !0x3FFF_FFFF) | (virt & 0x3FFF_FFFF)); }
        let pd = (e3 & !0xFFF) as *const u64;
        let e2 = *pd.add(((virt >> 21) & 0x1FF) as usize);
        if e2 & PTE_PRESENT == 0 { return None; }
        if e2 & PTE_PS != 0 { return Some((e2 & !0x1F_FFFF) | (virt & 0x1F_FFFF)); }
        let pt = (e2 & !0xFFF) as *const u64;
        let e1 = *pt.add(((virt >> 12) & 0x1FF) as usize);
        if e1 & PTE_PRESENT == 0 { return None; }
        Some((e1 & !0xFFF) | (virt & 0xFFF))
    }
}

/// Recursively free user (PTE_USER) page tables and pages below 0x8000_0000_0000.
/// Huge pages (PS=1) and supervisor leaf entries are identity-mapped memory that
/// was never allocated per-frame, so they are skipped, not freed.
pub fn destroy_user_space(pml4_phys: u64) {
    unsafe {
        let pml4 = pml4_phys as *mut u64;
        for i4 in 0..512 {
            let e4 = *pml4.add(i4);
            if e4 & PTE_PRESENT == 0 || e4 & PTE_USER == 0 || e4 & PTE_PS != 0 { continue; }
            let pdpt = (e4 & !0xFFF) as *mut u64;
            for i3 in 0..512 {
                let e3 = *pdpt.add(i3);
                if e3 & PTE_PRESENT == 0 || e3 & PTE_PS != 0 { continue; }
                let pd = (e3 & !0xFFF) as *mut u64;
                for i2 in 0..512 {
                    let e2 = *pd.add(i2);
                    if e2 & PTE_PRESENT == 0 || e2 & PTE_PS != 0 { continue; }
                    let pt = (e2 & !0xFFF) as *mut u64;
                    for i1 in 0..512 {
                        let e1 = *pt.add(i1);
                        if e1 & PTE_PRESENT != 0 && e1 & PTE_USER != 0 {
                            pmm::free_page((e1 & !0xFFF) as usize);
                        }
                    }
                    pmm::free_page((e2 & !0xFFF) as usize);
                }
                pmm::free_page((e3 & !0xFFF) as usize);
            }
            pmm::free_page((e4 & !0xFFF) as usize);
        }
        pmm::free_page(pml4_phys as usize);
    }
}

// ---------------------------------------------------------------------------
// MSR access (used for per-process FS base, needed by e1wine/arch_prctl)
// ---------------------------------------------------------------------------
pub const MSR_FSBASE: u32 = 0xC000_0100;

pub fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | lo as u64
}

/// Enable SSE/SSE2 for user programs: stage2 enters long mode with CR0.EM set
/// and CR4.OSFXSR clear, which makes every SSE instruction raise #UD.
pub fn fpu_init() {
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 &= !(1u64 << 2); // clear EM (emulate coprocessor)
        cr0 |= 1u64 << 1;    // set MP (monitor coprocessor)
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack));
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= (1u64 << 9) | (1u64 << 10); // OSFXSR | OSXMMEXCPT
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack));
    }
}

pub fn write_msr(msr: u32, val: u64) {
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") val as u32,
            in("edx") (val >> 32) as u32,
            options(nomem, nostack)
        );
    }
}
