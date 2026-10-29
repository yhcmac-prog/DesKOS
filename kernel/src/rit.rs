//! .rit package format (DBPF style) reader and installer.
//!
//! Format:
//!   [ Header 32 bytes ]
//!   [ Index table N * 24 bytes ]
//!   [ Data blocks ]
//!
//! Header fields:
//!   0x00  magic     [8]  "RIFFrit\0"
//!   0x08  version   u32  (1)
//!   0x0C  count     u32  number of index entries
//!   0x10  index_off u32  offset from file start to index table
//!   0x14  index_sz  u32  size of index table in bytes
//!   0x18  reserved  [8]
//!
//! Index entry 24 bytes:
//!   0x00  type_id   u32  (0x0001=ELF, 0x0002=lib, 0x0010=data, 0x0100=meta)
//!   0x04  group_id  u32
//!   0x08  instance  u32
//!   0x0C  offset    u32  data block offset
//!   0x10  size      u32  uncompressed size
//!   0x14  flags     u32  (bit0=compressed?)

use crate::e1fss;
use crate::println;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;

const RIT_MAGIC: &[u8; 8] = b"RIFFrit\0";

// Package bytes come from a Vec<u8> (1-byte aligned), so every multi-byte
// field must be read unaligned — casting a &RitHeader/&[RitIndex] would be UB.
fn rd32(d: &[u8], off: usize) -> u32 {
    unsafe { core::ptr::read_unaligned(d.as_ptr().add(off) as *const u32) }
}

pub fn install(data: &[u8]) -> Result<(), &'static str> {
    if data.len() < 32 || &data[0..8] != RIT_MAGIC {
        return Err("rit: bad magic");
    }
    let count = rd32(data, 12) as usize;
    let index_off = rd32(data, 16) as usize;
    let index_sz = rd32(data, 20) as usize;
    if count == 0 || index_sz == 0 {
        return Err("rit: empty");
    }
    if index_off + count * 24 > data.len() {
        return Err("rit: bad index");
    }

    // Parse meta block to get file name mapping
    let mut pkg_name = None;
    let mut files = Vec::new();
    for i in 0..count {
        let base = index_off + i * 24;
        let type_id = rd32(data, base);
        let instance = rd32(data, base + 8);
        let offset = rd32(data, base + 12) as usize;
        let size = rd32(data, base + 16) as usize;
        if offset + size > data.len() {
            return Err("rit: bad offset");
        }
        let payload = &data[offset..offset + size];
        match type_id {
            0x0100 => {
                pkg_name = Some(String::from_utf8_lossy(payload).to_string());
            }
            0x0001 | 0x0002 | 0x0010 => {
                files.push((instance, type_id, payload));
            }
            _ => {}
        }
    }

    let name = pkg_name.as_deref().unwrap_or("unnamed");
    println!("rit: installing '{}' ({} files)", name, files.len());

    // Install files to the root dir: the first ELF keeps the package name
    // (so e1wine can run it directly), the rest are qualified with the
    // package dir prefix.
    let dir = format!("pkg_{}", name.replace(|c: char| !c.is_alphanumeric(), "_"));
    let mut named_primary = false;
    for (inst, typ, payload) in files {
        let fname = if typ == 0x0001 && !named_primary {
            named_primary = true;
            name.to_string()
        } else {
            format!("{}_{}", dir, inst)
        };
        if e1fss::create(&fname, payload).is_err() {
            println!("rit: failed to install {}", fname);
        }
    }

    // Record installation in /rit_installed
    let mut log = b"".to_vec();
    if let Ok(existing) = e1fss::read_file("rit_installed") {
        log = existing;
        log.push(b'\n');
    }
    log.extend_from_slice(name.as_bytes());
    e1fss::create("rit_installed", &log).ok();
    println!("rit: installed '{}' OK", name);
    Ok(())
}

pub fn list() -> Result<Vec<(String, u32)>, &'static str> {
    let mut out = Vec::new();
    let data = match e1fss::read_file("rit_installed") {
        Ok(d) => d,
        Err(_) => return Ok(out),
    };
    let s = core::str::from_utf8(&data).map_err(|_| "bad utf8")?;
    for line in s.lines() {
        if !line.is_empty() {
            out.push((line.to_string(), 0x0100));
        }
    }
    Ok(out)
}
