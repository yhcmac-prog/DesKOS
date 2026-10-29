//! Kernel heap: free-list allocator over a fixed identity-mapped arena,
//! exposed as the global allocator so `alloc::{vec::Vec, string::String,
//! boxed::Box}` work. The arena (HEAP_START..HEAP_MAX) is reserved in pmm.

use crate::console::SpinLock;
use crate::pmm;
use core::alloc::{GlobalAlloc, Layout};

pub const HEAP_START: usize = 0x2000000; // 32MB, inside stage2's identity map
pub const HEAP_MAX: usize = 0x4000000;   // 64MB cap

#[derive(Clone, Copy)]
struct Block {
    size: usize, // payload bytes (excluding header)
    free: bool,
    next: usize, // address of next header, 0 = end of list
}

const HDR: usize = core::mem::size_of::<Block>();

struct Heap {
    start: usize,
    committed: usize, // end of committed memory
}

impl Heap {
    const fn new() -> Self {
        Heap { start: HEAP_START, committed: 0 }
    }

    unsafe fn init(&mut self) {
        self.committed = HEAP_START;
        self.grow(8);
    }

    /// Commit `pages` more pages of the arena and add a free block for it.
    unsafe fn grow(&mut self, pages: usize) -> bool {
        if self.committed + pages * pmm::PAGE_SIZE > HEAP_MAX {
            return false;
        }
        let base = self.committed;
        self.committed = base + pages * pmm::PAGE_SIZE;
        core::ptr::write_bytes(base as *mut u8, 0, pages * pmm::PAGE_SIZE);
        if base == HEAP_START {
            // first block
            let b = base as *mut Block;
            (*b).size = pages * pmm::PAGE_SIZE - HDR;
            (*b).free = true;
            (*b).next = 0;
        } else {
            // append to tail: merge if tail free, else chain new block
            let mut cur = self.start as *mut Block;
            while (*cur).next != 0 {
                cur = (*cur).next as *mut Block;
            }
            if (*cur).free && cur as usize + HDR + (*cur).size == base {
                (*cur).size += pages * pmm::PAGE_SIZE;
            } else {
                let nb = base as *mut Block;
                (*nb).size = pages * pmm::PAGE_SIZE - HDR;
                (*nb).free = true;
                (*nb).next = 0;
                (*cur).next = base;
            }
        }
        true
    }

    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = (layout.size().max(16) + 15) & !15;
        loop {
            let mut cur = self.start as *mut Block;
            while cur as usize != 0 && (cur as usize) < self.committed {
                let b = &mut *cur;
                if b.free && b.size >= size {
                    if b.size >= size + HDR + 32 {
                        let naddr = cur as usize + HDR + size;
                        let nb = naddr as *mut Block;
                        (*nb).size = b.size - size - HDR;
                        (*nb).free = true;
                        (*nb).next = b.next;
                        b.size = size;
                        b.next = naddr;
                    }
                    b.free = false;
                    return (cur as usize + HDR) as *mut u8;
                }
                if b.next == 0 {
                    break;
                }
                cur = b.next as *mut Block;
            }
            let need = (size + HDR + pmm::PAGE_SIZE - 1) / pmm::PAGE_SIZE;
            if !self.grow(need.max(4)) {
                return core::ptr::null_mut();
            }
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        let hdr = (ptr as usize - HDR) as *mut Block;
        (*hdr).free = true;
        // coalesce adjacent free blocks
        let mut cur = self.start as *mut Block;
        while cur as usize != 0 && (*cur).next != 0 {
            let next = (*cur).next as *mut Block;
            if (*cur).free && (*next).free {
                (*cur).size += HDR + (*next).size;
                (*cur).next = (*next).next;
            } else {
                cur = next;
            }
        }
    }
}

pub struct LockedHeap(SpinLock<Heap>);

impl LockedHeap {
    pub const fn new() -> Self {
        LockedHeap(SpinLock::new(Heap::new()))
    }
    pub fn init(&self) {
        let h = self.0.lock();
        unsafe { h.init() };
        self.0.unlock();
    }
}

/// Initialize the global heap arena.
pub fn init() {
    HEAP.init();
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let h = self.0.lock();
        let p = h.alloc(layout);
        self.0.unlock();
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let h = self.0.lock();
        h.dealloc(ptr);
        self.0.unlock();
    }
}

#[global_allocator]
pub static HEAP: LockedHeap = LockedHeap::new();
