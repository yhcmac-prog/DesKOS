//! ATA PIO driver for primary IDE channel (master/slave).

use crate::console::{inb, outb};
use crate::println;

const BASE: u16 = 0x1F0;
const CTRL: u16 = 0x3F6;

unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack));
    v
}

pub struct Disk {
    pub present: bool,
    pub slave: bool,
    pub sectors: u32,
}

static mut DISKS: [Disk; 2] = [
    Disk { present: false, slave: false, sectors: 0 },
    Disk { present: false, slave: true, sectors: 0 },
];

/// Drive-select byte on 0x1F6: LBA bit pattern + master(0)/slave(1).
fn drive_byte(slot: usize, lba: u32) -> u8 {
    0xE0 | ((slot as u8) << 4) | ((lba >> 24) & 0x0F) as u8
}

fn wait_not_busy() {
    unsafe {
        while inb(BASE + 7) & 0x80 != 0 {}
    }
}

fn wait_drq() {
    unsafe {
        while inb(BASE + 7) & 0x08 == 0 {}
    }
}

fn delay_400ns() {
    for _ in 0..4 {
        unsafe { inb(CTRL) };
    }
}

pub fn init() {
    for slot in 0..2 {
        identify(slot);
    }
}

fn identify(slot: usize) {
    unsafe {
        // Try to identify the drive on the primary channel
        outb(BASE + 6, 0xA0 | ((slot as u8) << 4));
        delay_400ns();
        outb(BASE + 7, 0xEC);
        delay_400ns();
        if inb(BASE + 7) == 0 {
            println!("ata: no disk on primary {}", if slot == 0 { "master" } else { "slave" });
            return;
        }
        wait_not_busy();
        // wait for DRQ or ERR
        for _ in 0..100000 {
            let st = inb(BASE + 7);
            if st & 0x08 != 0 {
                break;
            }
            if st & 0x01 != 0 {
                println!("ata: identify failed");
                return;
            }
        }
        // read identify data
        let mut buf = [0u16; 256];
        for i in 0..256 {
            buf[i] = inw(BASE);
        }
        let sectors = (buf[60] as u32) | ((buf[61] as u32) << 16);
        DISKS[slot].present = true;
        DISKS[slot].sectors = sectors;
        println!(
            "ata: primary {} present, {} sectors ({} MB)",
            if slot == 0 { "master" } else { "slave" },
            sectors,
            sectors / 2048
        );
    }
}

pub fn disk0() -> &'static Disk {
    unsafe { &DISKS[0] }
}

pub fn disk1() -> &'static Disk {
    unsafe { &DISKS[1] }
}

/// Read `count` sectors starting at LBA28 `lba` from disk `slot` (512B each).
pub fn read_sectors_on(slot: usize, lba: u32, count: u8, buf: &mut [u8]) -> Result<(), &'static str> {
    if count == 0 {
        return Ok(());
    }
    if buf.len() < count as usize * 512 {
        return Err("buffer too small");
    }
    unsafe {
        wait_not_busy();
        outb(BASE + 6, drive_byte(slot, lba));
        delay_400ns();
        outb(BASE + 2, count);
        outb(BASE + 3, (lba & 0xFF) as u8);
        outb(BASE + 4, ((lba >> 8) & 0xFF) as u8);
        outb(BASE + 5, ((lba >> 16) & 0xFF) as u8);
        outb(BASE + 7, 0x20); // READ SECTORS
        for s in 0..count as usize {
            wait_not_busy();
            wait_drq();
            let ptr = buf.as_mut_ptr().add(s * 512) as *mut u16;
            for i in 0..256 {
                *ptr.add(i) = inw(BASE);
            }
        }
    }
    Ok(())
}

/// Read from the primary master (existing e1fss API).
pub fn read_sectors(lba: u32, count: u8, buf: &mut [u8]) -> Result<(), &'static str> {
    read_sectors_on(0, lba, count, buf)
}

/// Write `count` sectors from `buf` starting at LBA28 `lba` to disk `slot`.
pub fn write_sectors_on(slot: usize, lba: u32, count: u8, buf: &[u8]) -> Result<(), &'static str> {
    if count == 0 {
        return Ok(());
    }
    if buf.len() < count as usize * 512 {
        return Err("buffer too small");
    }
    unsafe {
        wait_not_busy();
        outb(BASE + 6, drive_byte(slot, lba));
        delay_400ns();
        outb(BASE + 2, count);
        outb(BASE + 3, (lba & 0xFF) as u8);
        outb(BASE + 4, ((lba >> 8) & 0xFF) as u8);
        outb(BASE + 5, ((lba >> 16) & 0xFF) as u8);
        outb(BASE + 7, 0x30); // WRITE SECTORS
        for s in 0..count as usize {
            wait_not_busy();
            wait_drq();
            // the data port is 16-bit: must use outw, two outb's would both
            // land on the low byte and corrupt the sector
            let ptr = buf.as_ptr().add(s * 512) as *const u16;
            for i in 0..256 {
                crate::console::outw(BASE, *ptr.add(i));
            }
        }
        // flush cache
        outb(BASE + 7, 0xE7);
        wait_not_busy();
    }
    Ok(())
}

/// Write to the primary master (existing e1fss API).
pub fn write_sectors(lba: u32, count: u8, buf: &[u8]) -> Result<(), &'static str> {
    write_sectors_on(0, lba, count, buf)
}
