//! e1FSS: flat 64-entry directory + contiguous file blocks, block 4096 bytes.

use crate::ata;
use crate::println;
use alloc::vec::Vec;
use core::str;

const BLOCK_SIZE: usize = 4096;
const E1FSS_LBA: u32 = 2048; // 1MB offset
const SUPERBLOCK_OFF: usize = 0;
const MAGIC: &[u8; 8] = b"e1FSSv1\0";

#[repr(C)]
#[derive(Clone, Copy)]
struct Superblock {
    magic: [u8; 8],
    block_size: u32,
    total_blocks: u32,
    inode_count: u32,
    bitmap_start: u32,
    inode_table_start: u32,
    data_start: u32,
    root_ino: u32,
    free_blocks: u32,
    free_inodes: u32,
    volume_name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Inode {
    typ: u16,        // 1=file, 2=dir
    nlinks: u16,
    size: u32,
    blocks: [u32; 12],
    indirect: u32,
    ctime: u32,
    mtime: u32,
    _pad: [u8; 60],   // pad to 128 bytes
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Dirent {
    ino: u32,
    name_len: u8,
    name: [u8; 59],
}

pub struct File {
    pub ino: u32,
    pub size: u32,
    pub is_dir: bool,
}

pub struct DirEntry {
    pub name: [u8; 59],
    pub ino: u32,
    pub is_dir: bool,
}

static mut MOUNTED: bool = false;
static mut SB: Superblock = unsafe { core::mem::zeroed() };

fn read_block(block: u32, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < BLOCK_SIZE {
        return Err("buffer too small");
    }
    let lba = E1FSS_LBA + block * (BLOCK_SIZE as u32 / 512);
    ata::read_sectors(lba, 8, buf)
}

fn write_block(block: u32, buf: &[u8]) -> Result<(), &'static str> {
    if buf.len() < BLOCK_SIZE {
        return Err("buffer too small");
    }
    let lba = E1FSS_LBA + block * (BLOCK_SIZE as u32 / 512);
    ata::write_sectors(lba, 8, buf)
}

fn read_inode(ino: u32, inode: &mut Inode) -> Result<(), &'static str> {
    let sb = unsafe { &SB };
    let inodes_per_block = BLOCK_SIZE / 128;
    let block = sb.inode_table_start + ino / inodes_per_block as u32;
    let offset = (ino % inodes_per_block as u32) as usize * 128;
    let mut buf = [0u8; BLOCK_SIZE];
    read_block(block, &mut buf)?;
    let ptr = unsafe { buf.as_ptr().add(offset) } as *const Inode;
    unsafe { *inode = ptr.read_unaligned() };
    Ok(())
}

fn write_inode(ino: u32, inode: &Inode) -> Result<(), &'static str> {
    let sb = unsafe { &SB };
    let inodes_per_block = BLOCK_SIZE / 128;
    let block = sb.inode_table_start + ino / inodes_per_block as u32;
    let offset = (ino % inodes_per_block as u32) as usize * 128;
    let mut buf = [0u8; BLOCK_SIZE];
    read_block(block, &mut buf)?;
    let ptr = unsafe { buf.as_mut_ptr().add(offset) } as *mut Inode;
    unsafe { ptr.write_unaligned(*inode) };
    write_block(block, &buf)
}

fn alloc_block() -> Option<u32> {
    let sb = unsafe { &mut SB };
    if sb.free_blocks == 0 {
        return None;
    }
    let bitmap_start = sb.bitmap_start;
    let total_bits = sb.total_blocks;
    let mut buf = [0u8; BLOCK_SIZE];
    let blocks_per_bitmap_block = BLOCK_SIZE * 8;
    for bb in 0..(total_bits + blocks_per_bitmap_block as u32 - 1) / blocks_per_bitmap_block as u32 {
        read_block(bitmap_start + bb, &mut buf).ok()?;
        for i in 0..BLOCK_SIZE {
            if buf[i] != 0xFF {
                for bit in 0..8 {
                    let mask = 1 << bit;
                    if buf[i] & mask == 0 {
                        let block = bb * blocks_per_bitmap_block as u32 + i as u32 * 8 + bit;
                        buf[i] |= mask;
                        write_block(bitmap_start + bb, &buf).ok()?;
                        sb.free_blocks -= 1;
                        // zero the block
                        let zero = [0u8; BLOCK_SIZE];
                        write_block(block, &zero).ok()?;
                        return Some(block);
                    }
                }
            }
        }
    }
    None
}

fn free_block(block: u32) {
    let sb = unsafe { &mut SB };
    let bitmap_start = sb.bitmap_start;
    let bb = block / (BLOCK_SIZE as u32 * 8);
    let bit = block % (BLOCK_SIZE as u32 * 8);
    let byte = bit / 8;
    let bit_in_byte = bit % 8;
    let mut buf = [0u8; BLOCK_SIZE];
    if read_block(bitmap_start + bb, &mut buf).is_err() {
        return;
    }
    buf[byte as usize] &= !(1 << bit_in_byte);
    if write_block(bitmap_start + bb, &buf).is_ok() {
        sb.free_blocks += 1;
    }
}

fn alloc_inode() -> Option<u32> {
    let sb = unsafe { &mut SB };
    if sb.free_inodes == 0 {
        return None;
    }
    for ino in 0..sb.inode_count {
        let mut inode = unsafe { core::mem::zeroed::<Inode>() };
        if read_inode(ino, &mut inode).is_err() {
            continue;
        }
        if inode.typ == 0 {
            inode.typ = 1;
            write_inode(ino, &inode).ok()?;
            sb.free_inodes -= 1;
            return Some(ino);
        }
    }
    None
}

pub fn init() -> Result<(), &'static str> {
    let mut buf = [0u8; BLOCK_SIZE];
    read_block(0, &mut buf)?;
    let sb = unsafe { &*(buf.as_ptr() as *const Superblock) };
    if &sb.magic != MAGIC {
        return Err("bad e1FSS magic");
    }
    unsafe {
        SB = *sb;
        MOUNTED = true;
    }
    println!("e1fss: mounted, volume {:?}", str::from_utf8(&sb.volume_name).unwrap_or("?"));
    Ok(())
}

pub fn is_mounted() -> bool {
    unsafe { MOUNTED }
}

pub fn list_dir(path: &str) -> Result<Vec<DirEntry>, &'static str> {
    if !is_mounted() {
        return Err("e1FSS not mounted");
    }
    // v1: only root dir supported
    if path != "/" && path != "" {
        return Err("subdirs not supported");
    }
    let root_ino = unsafe { SB.root_ino };
    let mut inode = unsafe { core::mem::zeroed::<Inode>() };
    read_inode(root_ino, &mut inode)?;
    if inode.typ != 2 {
        return Err("root is not a dir");
    }
    let mut entries = Vec::new();
    let mut buf = [0u8; BLOCK_SIZE];
    for i in 0..12 {
        let b = inode.blocks[i];
        if b == 0 {
            break;
        }
        read_block(b, &mut buf)?;
        for off in (0..BLOCK_SIZE).step_by(64) {
            let de = unsafe { &*(buf.as_ptr().add(off) as *const Dirent) };
            if de.ino == 0 {
                continue;
            }
            let mut child = unsafe { core::mem::zeroed::<Inode>() };
            if read_inode(de.ino, &mut child).is_err() {
                continue;
            }
            entries.push(DirEntry {
                name: de.name,
                ino: de.ino,
                is_dir: child.typ == 2,
            });
        }
    }
    Ok(entries)
}

pub fn open(path: &str) -> Result<File, &'static str> {
    if !is_mounted() {
        return Err("e1FSS not mounted");
    }
    if path == "/" || path == "" {
        return Err("cannot open dir as file");
    }
    let name = if path.starts_with('/') { &path[1..] } else { path };
    let entries = list_dir("/")?;
    for e in entries {
        let nlen = e.name.iter().position(|&c| c == 0).unwrap_or(59);
        let ename = str::from_utf8(&e.name[..nlen]).map_err(|_| "bad name")?;
        if ename == name {
            let mut inode = unsafe { core::mem::zeroed::<Inode>() };
            read_inode(e.ino, &mut inode)?;
            return Ok(File {
                ino: e.ino,
                size: inode.size,
                is_dir: inode.typ == 2,
            });
        }
    }
    Err("not found")
}

pub fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let f = open(path)?;
    if f.is_dir {
        return Err("is a directory");
    }
    let mut inode = unsafe { core::mem::zeroed::<Inode>() };
    read_inode(f.ino, &mut inode)?;
    let mut out = Vec::with_capacity(f.size as usize);
    let mut remaining = f.size;
    let mut buf = [0u8; BLOCK_SIZE];
    for i in 0..12 {
        if remaining == 0 {
            break;
        }
        let b = inode.blocks[i];
        if b == 0 {
            break;
        }
        read_block(b, &mut buf)?;
        let to_copy = remaining.min(BLOCK_SIZE as u32) as usize;
        out.extend_from_slice(&buf[..to_copy]);
        remaining -= to_copy as u32;
    }
    if remaining > 0 && inode.indirect != 0 {
        let mut ind_buf = [0u8; BLOCK_SIZE];
        read_block(inode.indirect, &mut ind_buf)?;
        let ind = unsafe { core::slice::from_raw_parts(ind_buf.as_ptr() as *const u32, BLOCK_SIZE / 4) };
        for &b in ind.iter() {
            if remaining == 0 {
                break;
            }
            if b == 0 {
                continue;
            }
            read_block(b, &mut buf)?;
            let to_copy = remaining.min(BLOCK_SIZE as u32) as usize;
            out.extend_from_slice(&buf[..to_copy]);
            remaining -= to_copy as u32;
        }
    }
    Ok(out)
}

pub fn create(path: &str, data: &[u8]) -> Result<(), &'static str> {
    if !is_mounted() {
        return Err("e1FSS not mounted");
    }
    if path == "/" || path == "" {
        return Err("invalid name");
    }
    let name = if path.starts_with('/') { &path[1..] } else { path };
    if name.len() > 59 {
        return Err("name too long");
    }
    // check if exists
    let entries = list_dir("/")?;
    for e in entries {
        let nlen = e.name.iter().position(|&c| c == 0).unwrap_or(59);
        let ename = str::from_utf8(&e.name[..nlen]).map_err(|_| "bad name")?;
        if ename == name {
            return Err("exists");
        }
    }
    let ino = alloc_inode().ok_or("no free inodes")?;
    let mut inode = unsafe { core::mem::zeroed::<Inode>() };
    inode.typ = 1;
    inode.size = data.len() as u32;
    let blocks_needed = (data.len() + BLOCK_SIZE - 1) / BLOCK_SIZE;
    let mut buf = [0u8; BLOCK_SIZE];
    let mut written = 0;
    for i in 0..12 {
        if written >= data.len() {
            break;
        }
        let b = alloc_block().ok_or("no free blocks")?;
        inode.blocks[i] = b;
        let to_copy = (data.len() - written).min(BLOCK_SIZE);
        buf[..to_copy].copy_from_slice(&data[written..written + to_copy]);
        write_block(b, &buf)?;
        written += to_copy;
    }
    if written < data.len() {
        let ind = alloc_block().ok_or("no free blocks for indirect")?;
        inode.indirect = ind;
        let mut ind_buf = [0u8; BLOCK_SIZE];
        let ind_ptr = ind_buf.as_mut_ptr() as *mut u32;
        let mut idx = 0;
        while written < data.len() && idx < BLOCK_SIZE / 4 {
            let b = alloc_block().ok_or("no free blocks")?;
            unsafe { *ind_ptr.add(idx) = b };
            let to_copy = (data.len() - written).min(BLOCK_SIZE);
            buf[..to_copy].copy_from_slice(&data[written..written + to_copy]);
            write_block(b, &buf)?;
            written += to_copy;
            idx += 1;
        }
        write_block(ind, &ind_buf)?;
        if written < data.len() {
            return Err("file too large");
        }
    }
    write_inode(ino, &inode)?;
    // add dirent to root
    let root_ino = unsafe { SB.root_ino };
    let mut root_inode = unsafe { core::mem::zeroed::<Inode>() };
    read_inode(root_ino, &mut root_inode)?;
    let mut added = false;
    for i in 0..12 {
        let b = root_inode.blocks[i];
        if b == 0 {
            // allocate new dir block
            let nb = alloc_block().ok_or("no free blocks for dir")?;
            root_inode.blocks[i] = nb;
            write_inode(root_ino, &root_inode)?;
            let mut dbuf = [0u8; BLOCK_SIZE];
            let de = Dirent {
                ino,
                name_len: name.len() as u8,
                name: {
                    let mut n = [0u8; 59];
                    n[..name.len()].copy_from_slice(name.as_bytes());
                    n
                },
            };
            unsafe { core::ptr::write(dbuf.as_mut_ptr() as *mut Dirent, de) };
            write_block(nb, &dbuf)?;
            added = true;
            break;
        }
        let mut dbuf = [0u8; BLOCK_SIZE];
        read_block(b, &mut dbuf)?;
        for off in (0..BLOCK_SIZE).step_by(64) {
            let de = unsafe { &mut *(dbuf.as_mut_ptr().add(off) as *mut Dirent) };
            if de.ino == 0 {
                de.ino = ino;
                de.name_len = name.len() as u8;
                let mut n = [0u8; 59];
                n[..name.len()].copy_from_slice(name.as_bytes());
                de.name = n;
                write_block(b, &dbuf)?;
                added = true;
                break;
            }
        }
        if added {
            break;
        }
    }
    if !added {
        return Err("root dir full");
    }
    Ok(())
}
