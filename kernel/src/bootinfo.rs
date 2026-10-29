//! Boot information passed by stage2 at physical 0x6000.

pub const MAGIC: u32 = 0x4C62_4174; // 'tAbL'
pub const E820_PTR: usize = 0x5000;

#[repr(C)]
pub struct BootInfo {
    pub magic: u32,        // 0x00
    pub kernel_size: u32,  // 0x04
    pub e820_count: u32,   // 0x08
    pub vbe_mode: u32,     // 0x0C
    pub fb_phys: u32,      // 0x10
    pub fb_pitch: u32,     // 0x14
    pub fb_width: u16,     // 0x18
    pub fb_height: u16,    // 0x1A
    pub fb_bpp: u8,        // 0x1C
    pub boot_drive: u8,    // 0x1D
    pub _pad: [u8; 2],     // 0x1E
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct E820Entry {
    pub base: u64,
    pub len: u64,
    pub typ: u32,
    pub acpi: u32,
}

impl BootInfo {
    pub fn e820_entries(&self) -> &[E820Entry] {
        unsafe {
            core::slice::from_raw_parts(E820_PTR as *const E820Entry, self.e820_count as usize)
        }
    }
}

static mut BOOTINFO_PTR: *const BootInfo = core::ptr::null();

pub fn set(ptr: *const BootInfo) {
    unsafe { BOOTINFO_PTR = ptr; }
}

/// Global access to boot info (valid after `set` is called from kernel_main).
pub fn get() -> &'static BootInfo {
    unsafe { &*BOOTINFO_PTR }
}
