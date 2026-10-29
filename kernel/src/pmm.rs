//! Physical memory manager: 4KB page bitmap built from the E820 map.

use crate::bootinfo::BootInfo;
use crate::println;

const MAX_PHYS: usize = 1024 * 1024 * 1024; // manage first 1GB
pub const PAGE_SIZE: usize = 4096;
const MAX_PAGES: usize = MAX_PHYS / PAGE_SIZE;
const BITMAP_BYTES: usize = MAX_PAGES / 8;

static mut BITMAP: [u8; BITMAP_BYTES] = [0; BITMAP_BYTES]; // 1 = used
static mut FREE_PAGES: usize = 0;
static mut TOTAL_PAGES: usize = 0;

extern "C" {
    static _kernel_end: u8;
}

fn set_bit(page: usize) {
    unsafe { BITMAP[page / 8] |= 1 << (page % 8) };
}
fn clear_bit(page: usize) {
    unsafe { BITMAP[page / 8] &= !(1 << (page % 8)) };
}
fn test_bit(page: usize) -> bool {
    unsafe { BITMAP[page / 8] & (1 << (page % 8)) != 0 }
}

pub fn init(bi: &BootInfo) {
    let kend = unsafe { core::ptr::addr_of!(_kernel_end) as usize };
    unsafe {
        for b in BITMAP.iter_mut() {
            *b = 0xFF; // default: used
        }
        TOTAL_PAGES = 0;
        FREE_PAGES = 0;
    }
    // free RAM regions from E820
    for e in bi.e820_entries() {
        if e.typ != 1 {
            continue;
        }
        let mut base = e.base as usize;
        let mut len = e.len as usize;
        if base >= MAX_PHYS {
            continue;
        }
        if base + len > MAX_PHYS {
            len = MAX_PHYS - base;
        }
        // align up to page
        let start = (base + PAGE_SIZE - 1) / PAGE_SIZE;
        let end = (base + len) / PAGE_SIZE;
        for p in start..end {
            clear_bit(p);
            unsafe {
                TOTAL_PAGES += 1;
                FREE_PAGES += 1;
            }
        }
    }
    // reserve everything below kernel end (includes page tables, VGA, bootinfo)
    let reserve_pages = (kend + PAGE_SIZE - 1) / PAGE_SIZE;
    for p in 0..reserve_pages.min(MAX_PAGES) {
        if !test_bit(p) {
            unsafe { FREE_PAGES -= 1 };
        }
        set_bit(p);
    }
    // reserve the kernel heap arena (heap.rs manages it directly)
    let hstart = crate::heap::HEAP_START / PAGE_SIZE;
    let hend = crate::heap::HEAP_MAX / PAGE_SIZE;
    for p in hstart..hend.min(MAX_PAGES) {
        if !test_bit(p) {
            unsafe { FREE_PAGES -= 1 };
        }
        set_bit(p);
    }
    // Reserve physical ranges whose addresses collide with user VAs: the
    // kernel writes fresh frames identity-mapped (VA == PA), but while a
    // user process runs CR3 points at the user page tables, so a frame in
    // the user's ELF/brk/mmap VA ranges would be redirected by the user's
    // 4KB PTEs and alloc_page's zero-fill would wipe user memory instead.
    // User VA map: ELF 0x400000.., brk 0x5000000..0x5800000, mmap
    // 0x6000000..0x7000000 (BB/fb/stack VAs exceed physical RAM here).
    let ranges = [
        (0x0040_0000usize, 0x0100_0000usize), // user ELF/bss region
        (0x0500_0000, 0x0580_0000),           // brk
        (0x0600_0000, 0x0700_0000),           // mmap
    ];
    for (rb, re) in ranges {
        for p in (rb / PAGE_SIZE)..(re / PAGE_SIZE).min(MAX_PAGES) {
            if !test_bit(p) {
                unsafe { FREE_PAGES -= 1 };
            }
            set_bit(p);
        }
    }
    println!(
        "pmm: {} MB total, {} MB free",
        unsafe { TOTAL_PAGES } * 4 / 1024,
        unsafe { FREE_PAGES } * 4 / 1024
    );
}

/// Allocate one physical page; returns its physical address, or None.
pub fn alloc_page() -> Option<usize> {
    unsafe {
        if FREE_PAGES == 0 {
            return None;
        }
    }
    for p in ((0x100000 / PAGE_SIZE)..MAX_PAGES) {
        if !test_bit(p) {
            set_bit(p);
            unsafe { FREE_PAGES -= 1 };
            let addr = p * PAGE_SIZE;
            unsafe { core::ptr::write_bytes(addr as *mut u8, 0, PAGE_SIZE) };
            return Some(addr);
        }
    }
    None
}

pub fn free_page(addr: usize) {
    let p = addr / PAGE_SIZE;
    if p < MAX_PAGES && test_bit(p) {
        clear_bit(p);
        unsafe { FREE_PAGES += 1 };
    }
}

pub fn free_mb() -> usize {
    (unsafe { FREE_PAGES }) * 4 / 1024
}

pub fn total_mb() -> usize {
    (unsafe { TOTAL_PAGES }) * 4 / 1024
}
