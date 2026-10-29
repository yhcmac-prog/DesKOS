//! e1apfs: read-only APFS browser for the second disk (primary slave).
//!
//! Interactive commands: info / ls [path] / cat <path> / exit
//!
//! Layout knowledge verified against an hdiutil-made APFS image
//! (see /tmp/apfs_p16.py prototype): APFS partition starts at LBA 40,
//! block N lives at LBA 40 + N*8 (4 KiB blocks).

#![no_std]
#![no_main]

use libe1::{exit, read, write};

const BS: usize = 4096;
const PART_LBA: u64 = 40;

// ---------------------------------------------------------------- helpers

fn puts(s: &str) {
    write(1, s.as_bytes());
}

fn put_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if v == 0 {
        puts("0");
        return;
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    write(1, &buf[i..]);
}

fn cstr(b: &[u8]) -> &str {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    core::str::from_utf8(&b[..end]).unwrap_or("?")
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd64(b: &[u8], o: usize) -> u64 {
    let mut t = [0u8; 8];
    t.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(t)
}

// ---------------------------------------------------------------- block io

static mut BLOCK: [u8; BS] = [0u8; BS];
static mut BLOCK2: [u8; BS] = [0u8; BS];
static mut LEAF_OUT: [u8; BS] = [0u8; BS]; // write-path scratch (leaf_insert output)
static mut LEAF_BUF: [u8; BS] = [0u8; BS]; // write-path scratch (leaf read buffer)
static mut DBLK_BUF: [u8; BS] = [0u8; BS]; // write-path scratch (data block)
static mut R1: NewRec = NewRec { key: [0; 64], klen: 0, val: [0; 256], vlen: 0 };
static mut R2: NewRec = NewRec { key: [0; 64], klen: 0, val: [0; 256], vlen: 0 };
static mut R3: NewRec = NewRec { key: [0; 64], klen: 0, val: [0; 256], vlen: 0 };
const CRC32C_TABLE: [u32; 256] = crc32c_table();

const fn crc32c_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256u32 {
        let mut c = i;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0x82F63B78 } else { c >> 1 };
            j += 1;
        }
        t[i as usize] = c;
        i += 1;
    }
    t
}

fn block() -> &'static mut [u8; BS] {
    unsafe { &mut *core::ptr::addr_of_mut!(BLOCK) }
}
fn block2() -> &'static mut [u8; BS] {
    unsafe { &mut *core::ptr::addr_of_mut!(BLOCK2) }
}

fn read_block(n: u32, out: &mut [u8; BS]) -> bool {
    libe1::readblocks(PART_LBA + (n as u64) * 8, out) == (BS as i64)
}

// ---------------------------------------------------------------- b-tree

const BTNODE_ROOT: u16 = 1;
const BTNODE_LEAF: u16 = 2;
const BTNODE_FIXED: u16 = 4;

struct Node<'a> {
    b: &'a [u8],
    flags: u16,
    level: u16,
    nkeys: u32,
    keys_start: usize,
    vals_start: usize,
    fixed: bool,
}

impl<'a> Node<'a> {
    fn new(b: &'a [u8]) -> Node<'a> {
        let flags = rd16(b, 32);
        let tlen = rd16(b, 42) as usize;
        Node {
            b,
            flags,
            level: rd16(b, 34),
            nkeys: rd32(b, 36),
            keys_start: 56 + tlen,
            vals_start: if flags & BTNODE_ROOT != 0 { BS - 40 } else { BS },
            fixed: flags & BTNODE_FIXED != 0,
        }
    }

    /// Returns (key, val) slices; val is empty for BTOFF_INVALID.
    fn entry(&self, i: u32) -> (&'a [u8], &'a [u8]) {
        let b = self.b;
        if self.fixed {
            let (k, v) = (rd16(b, 56 + 4 * i as usize), rd16(b, 56 + 4 * i as usize + 2));
            let key = &b[self.keys_start + k as usize..self.keys_start + k as usize + 16];
            let val = if v == 0xFFFF {
                &b[0..0]
            } else {
                &b[self.vals_start - v as usize..self.vals_start - v as usize + 16]
            };
            (key, val)
        } else {
            let o = 56 + 8 * i as usize;
            let (ko, kl, vo, vl) = (
                rd16(b, o) as i16,
                rd16(b, o + 2) as i16,
                rd16(b, o + 4) as i16,
                rd16(b, o + 6) as i16,
            );
            let key = &b[self.keys_start + ko as usize..self.keys_start + ko as usize + kl as usize];
            let val = if vo == -1 {
                &b[0..0]
            } else {
                &b[self.vals_start - vo as usize..self.vals_start - vo as usize + vl as usize]
            };
            (key, val)
        }
    }

    fn is_leaf(&self) -> bool {
        self.flags & BTNODE_LEAF != 0
    }
}

// ---------------------------------------------------------------- mount state

struct Mount {
    mount_xid: u64,
    fs_oid: u64,
    nx_block: u32,
    omap_tree: u32,
    apsb_block: u32,
    vol_omap_tree: u32,
    fs_root: u32,
    volname: [u8; 16],
}

static mut MOUNT: Option<Mount> = None;

// scanned fs tree contents
const MAX_DRECS: usize = 128;
const MAX_EXT: usize = 64;
const MAX_SIZES: usize = 64;

#[derive(Clone, Copy)]
struct Drec {
    parent: u64,
    fid: u64,
    dt: u8,
    name: [u8; 32],
    nlen: usize,
}
#[derive(Clone, Copy)]
struct Ext {
    ino: u64,
    logical: u64,
    phys: u64,
}

static mut DRECS: [Drec; MAX_DRECS] = [Drec { parent: 0, fid: 0, dt: 0, name: [0; 32], nlen: 0 }; MAX_DRECS];
static mut NDREC: usize = 0;
static mut EXTS: [Ext; MAX_EXT] = [Ext { ino: 0, logical: 0, phys: 0 }; MAX_EXT];
static mut NEXTS: usize = 0;
static mut SIZES: [(u64, u64); MAX_SIZES] = [(0, 0); MAX_SIZES]; // (ino, size)
static mut NSIZES: usize = 0;

fn drecs() -> &'static mut [Drec; MAX_DRECS] {
    unsafe { &mut *core::ptr::addr_of_mut!(DRECS) }
}
fn exts() -> &'static mut [Ext; MAX_EXT] {
    unsafe { &mut *core::ptr::addr_of_mut!(EXTS) }
}
fn sizes() -> &'static mut [(u64, u64); MAX_SIZES] {
    unsafe { &mut *core::ptr::addr_of_mut!(SIZES) }
}

fn ndrec() -> usize {
    unsafe { *core::ptr::addr_of!(NDREC) }
}
fn nexts() -> usize {
    unsafe { *core::ptr::addr_of!(NEXTS) }
}
fn nsizes() -> usize {
    unsafe { *core::ptr::addr_of!(NSIZES) }
}

// ---------------------------------------------------------------- cpm + omap

struct Cpm {
    map: [(u64, u64); 16], // (oid, paddr)
    n: usize,
}

/// Resolve a virtual oid: CPM first, then the hdiutil metadata-zone
/// convention (paddr == oid for unmapped container objects).
fn resolve_oid(cpm: &Cpm, oid: u64) -> Option<u32> {
    for i in 0..cpm.n {
        if cpm.map[i].0 == oid {
            return Some(cpm.map[i].1 as u32);
        }
    }
    let b = block2();
    if read_block(oid as u32, b) && rd64(b, 8) == oid {
        return Some(oid as u32);
    }
    None
}

/// Fixed-kv omap lookup: key {oid,xid} -> val {flags,size,paddr}.
fn omap_lookup(tree: u32, oid: u64, max_xid: u64) -> Option<(u64, u32)> {
    let b = block();
    if !read_block(tree, b) {
        return None;
    }
    let n = Node::new(b);
    let mut best: Option<(&[u8], &[u8])> = None;
    for i in 0..n.nkeys {
        let (k, v) = n.entry(i);
        if rd64(k, 0) == oid && rd64(k, 8) <= max_xid {
            let better = match best {
                None => true,
                Some((bk, _)) => rd64(k, 8) > rd64(bk, 8),
            };
            if better {
                best = Some((k, v));
            }
        }
    }
    let (_, v) = best?;
    Some((rd64(v, 8), rd32(v, 4)))
}

// ---------------------------------------------------------------- mount

fn mount() -> Result<&'static mut Mount, &'static str> {
    unsafe {
        if let Some(m) = &mut *core::ptr::addr_of_mut!(MOUNT) {
            return Ok(m);
        }
    }
    // 1. find the newest NXSB in the descriptor region
    let mut nx_blk = 0u32;
    let mut nx_xid = 0u64;
    let mut found = false;
    for rb in 0..16u32 {
        let b = block();
        if !read_block(rb, b) {
            break;
        }
        if (rd32(b, 24) & 0xFFFF) == 1 && &b[32..36] == b"NXSB" {
            let xid = rd64(b, 16);
            if !found || xid > nx_xid {
                found = true;
                nx_blk = rb;
                nx_xid = xid;
            }
        }
    }
    if !found {
        return Err("no APFS container found on second disk");
    }
    let nx = block();
    if !read_block(nx_blk, nx) {
        return Err("read nx failed");
    }
    let desc_base = rd64(nx, 112);
    let desc_index = rd32(nx, 136);
    let desc_len = rd32(nx, 140);
    let nx_omap_oid = rd64(nx, 160);
    let fs_oid = rd64(nx, 184);

    // 2. checkpoint map
    let mut cpm = Cpm { map: [(0, 0); 16], n: 0 };
    for k in 0..(desc_len as u64 - 1) {
        let b = block();
        if !read_block((desc_base + desc_index as u64 + k) as u32, b) {
            return Err("read cpm failed");
        }
        let cnt = rd32(b, 36) as usize;
        for i in 0..cnt.min(8) {
            let o = 40 + 40 * i;
            if cpm.n < 16 {
                cpm.map[cpm.n] = (rd64(b, o + 24), rd64(b, o + 32));
                cpm.n += 1;
            }
        }
    }

    // 3. container omap -> tree
    let om_blk = resolve_oid(&cpm, nx_omap_oid).ok_or("cannot map container omap")?;
    let om = block();
    if !read_block(om_blk, om) {
        return Err("read omap failed");
    }
    let om_tree_oid = rd64(om, 48);
    let omap_tree = resolve_oid(&cpm, om_tree_oid).ok_or("cannot map omap tree")?;

    // 4. APSB
    let (apsb_blk, _) = omap_lookup(omap_tree, fs_oid, nx_xid).ok_or("volume not found in omap")?;
    let ab = block();
    if !read_block(apsb_blk as u32, ab) {
        return Err("read apsb failed");
    }
    if &ab[32..36] != b"APSB" {
        return Err("bad APSB magic");
    }
    let aps_omap_oid = rd64(ab, 128);
    let root_tree_oid = rd64(ab, 136);
    let mut volname = [0u8; 16];
    volname.copy_from_slice(&ab[704..720]);

    // 5. volume omap -> fs tree root
    let vo_blk = resolve_oid(&cpm, aps_omap_oid).ok_or("cannot map volume omap")?;
    let vo = block();
    if !read_block(vo_blk, vo) {
        return Err("read volume omap failed");
    }
    let vo_tree_oid = rd64(vo, 48);
    let vol_omap_tree = resolve_oid(&cpm, vo_tree_oid).ok_or("cannot map volume omap tree")?;
    let (fs_root, _) = omap_lookup(vol_omap_tree, root_tree_oid, nx_xid).ok_or("fs tree not found")?;

    let m = Mount {
        mount_xid: nx_xid,
        fs_oid,
        nx_block: nx_blk,
        omap_tree,
        apsb_block: apsb_blk as u32,
        vol_omap_tree,
        fs_root: fs_root as u32,
        volname,
    };
    unsafe {
        *core::ptr::addr_of_mut!(MOUNT) = Some(m);
    }
    scan_fs_tree();
    Ok(unsafe { &mut *core::ptr::addr_of_mut!(MOUNT) }.as_mut().unwrap())
}

// ---------------------------------------------------------------- fs tree scan

/// Walk the fs tree collecting drecs / extents / file sizes.
fn scan_fs_tree() {
    let mut pending: [u32; 8] = [0; 8];
    let mut npending = 0usize;
    let mut cur: u32;
    unsafe { cur = (*core::ptr::addr_of!(MOUNT)).as_ref().unwrap().fs_root };
    loop {
        let b = block();
        if !read_block(cur, b) { break; }
        let n = Node::new(b);
        if n.level == 0 {
            for i in 0..n.nkeys {
                let (key, val) = n.entry(i);
                let hdr = rd64(key, 0);
                let typ = (hdr >> 60) as u8;
                let oid = hdr & 0x0FFF_FFFF_FFFF_FFFF;
                if typ == 9 && !val.is_empty() {
                    let parent = hdr & 0x0FFF_FFFF_FFFF_FFFF;
                    let nlh = rd32(key, 8);
                    let nlen = (nlh & 0x3FF) as usize;
                    let fid = rd64(val, 0);
                    let dt = (rd16(val, 16) & 0xF) as u8;
                    if ndrec() < MAX_DRECS && nlen >= 1 && nlen <= 31 && key.len() >= 12 + nlen {
                        let d = &mut drecs()[ndrec()];
                        d.parent = parent;
                        d.fid = fid;
                        d.dt = dt;
                        d.name = [0; 32];
                        d.name[..nlen - 1].copy_from_slice(&key[12..12 + nlen - 1]);
                        d.nlen = nlen - 1;
                        unsafe { NDREC += 1; }
                    }
                } else if typ == 8 && !val.is_empty() {
                    if nexts() < MAX_EXT {
                        let e = &mut exts()[nexts()];
                        e.ino = oid;
                        e.logical = rd64(key, 8);
                        e.phys = rd64(val, 8);
                        unsafe { NEXTS += 1; }
                    }
                } else if typ == 3 && !val.is_empty() {
                    let num = rd16(val, 92) as usize;
                    if num > 0 && num <= 16 {
                        let mut data = 96 + 4 * num;
                        data = (data + 7) & !7;
                        for e in 0..num {
                            let xt = val[96 + 4 * e];
                            let xs = rd16(val, 96 + 4 * e + 2) as usize;
                            if xt == 8 && nsizes() < MAX_SIZES {
                                sizes()[nsizes()] = (oid, rd64(val, data));
                                unsafe { NSIZES += 1; }
                            }
                            data = (data + xs + 7) & !7;
                        }
                    }
                }
            }
        } else if npending < 8 {
            let (_, v) = n.entry(n.nkeys - 1);
            let child_oid = rd64(v, 0);
            let m = unsafe { (*core::ptr::addr_of!(MOUNT)).as_ref().unwrap() };
            if let Some((paddr, _)) = omap_lookup(m.vol_omap_tree, child_oid, m.mount_xid) {
                pending[npending] = paddr as u32;
                npending += 1;
            }
        }
        if npending == 0 { break; }
        npending -= 1;
        cur = pending[npending];
    }
}

fn file_size(ino: u64) -> Option<u64> {
    for i in 0..nsizes() {
        if sizes()[i].0 == ino {
            return Some(sizes()[i].1);
        }
    }
    None
}

/// Resolve a slash path from the root dir (inode 2). Returns file id.
fn resolve(path: &str) -> Option<u64> {
    let mut dir: u64 = 2;
    for comp in path.split('/') {
        if comp.is_empty() {
            continue;
        }
        let cb = comp.as_bytes();
        let mut hit = None;
        for i in 0..ndrec() {
            let d = &drecs()[i];
            if d.parent == dir && &d.name[..d.nlen] == cb {
                hit = Some(d.fid);
                break;
            }
        }
        dir = hit?;
    }
    Some(dir)
}

// ---------------------------------------------------------------- commands

fn cmd_info(m: &Mount) {
    puts("e1apfs info\n");
    puts("  container block: ");
    put_u64(m.nx_block as u64);
    puts("  xid: ");
    put_u64(m.mount_xid);
    puts("\n  volume: ");
    puts(cstr(&m.volname));
    puts("  (oid ");
    put_u64(m.fs_oid);
    puts(")\n  fs tree root: block ");
    put_u64(m.fs_root as u64);
    puts("\n  entries: ");
    put_u64(ndrec() as u64);
    puts("  extents: ");
    put_u64(nexts() as u64);
    puts("\n");
}

fn cmd_ls(path: &str) {
    let dir = if path.is_empty() { 2 } else { match resolve(path) {
        Some(f) => f,
        None => {
            puts("ls: not found\n");
            return;
        }
    } };
    // collect + insertion sort by name
    let mut idx: [usize; MAX_DRECS] = [0; MAX_DRECS];
    let mut cnt = 0usize;
    for i in 0..ndrec() {
        if drecs()[i].parent == dir {
            idx[cnt] = i;
            cnt += 1;
        }
    }
    for a in 1..cnt {
        let mut b = a;
        while b > 0 {
            let (x, y) = (idx[b - 1], idx[b]);
            if drecs()[x].name[..drecs()[x].nlen] > drecs()[y].name[..drecs()[y].nlen] {
                idx.swap(b - 1, b);
                b -= 1;
            } else {
                break;
            }
        }
    }
    for k in 0..cnt {
        let d = &drecs()[idx[k]];
        if d.dt == 4 {
            puts("dir  ");
            write(1, &d.name[..d.nlen]);
            puts("\n");
        } else {
            puts("file ");
            write(1, &d.name[..d.nlen]);
            if let Some(s) = file_size(d.fid) {
                puts(" size=");
                put_u64(s);
            }
            puts("\n");
        }
    }
}

fn cmd_cat(path: &str) {
    let fid = match resolve(path) {
        Some(f) => f,
        None => {
            puts("cat: not found\n");
            return;
        }
    };
    let size = match file_size(fid) {
        Some(s) => s as usize,
        None => {
            puts("cat: not a regular file\n");
            return;
        }
    };
    // extents sorted by logical offset
    let mut order: [usize; MAX_EXT] = [0; MAX_EXT];
    let mut cnt = 0usize;
    for i in 0..nexts() {
        if exts()[i].ino == fid {
            order[cnt] = i;
            cnt += 1;
        }
    }
    for a in 1..cnt {
        let mut b = a;
        while b > 0 && exts()[order[b - 1]].logical > exts()[order[b]].logical {
            order.swap(b - 1, b);
            b -= 1;
        }
    }
    let mut left = size;
    for k in 0..cnt {
        if left == 0 {
            break;
        }
        let phys = exts()[order[k]].phys as u32;
        let b = block2();
        if !read_block(phys, b) {
            puts("cat: read error\n");
            return;
        }
        let take = left.min(BS);
        write(1, &b[..take]);
        left -= take;
    }
    puts("\n");
}

// ---------------------------------------------------------------- write (M6 tail, v1)

// free physical blocks near the volume tail of the 16 MB test image
const DATA_AREA_START: u32 = 1000;

/// APFS drec name hash: CRC32C (no final xor) over UTF-32LE bytes of the
/// name, low 22 bits. Verified against existing image entries (p21).
fn name_hash22(name: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &c in name {
        crc = (crc >> 8) ^ CRC32C_TABLE[((crc ^ c as u32) & 0xFF) as usize];
        crc = (crc >> 8) ^ CRC32C_TABLE[((crc ^ 0) & 0xFF) as usize];
        crc = (crc >> 8) ^ CRC32C_TABLE[((crc ^ 0) & 0xFF) as usize];
        crc = (crc >> 8) ^ CRC32C_TABLE[((crc ^ 0) & 0xFF) as usize];
    }
    crc & 0x3F_FFFF
}

/// Recompute the APFS fletcher64 checksum of a 4K object block.
/// Derived from Fletcher64/VerifyBlock (apfs-fuse): verify is
/// F64(words[2..],0) then F64(words[0..1],cs) == 0; solved for cs and
/// validated against 23 real blocks of the image (p20).
fn fletcher64_store(b: &mut [u8; BS]) {
    let m: u64 = 0xFFFF_FFFF;
    let mut s1: u64 = 0;
    let mut s2: u64 = 0;
    let mut i = 8; // skip the 8-byte checksum field
    while i < BS {
        let w = rd32(b, i) as u64;
        s1 += w;
        s2 += s1;
        i += 4;
    }
    let a1 = s1 % m;
    let a2 = s2 % m;
    let c_lo = (m - (a2 + a1) % m) % m;
    let c_hi = (m - (a1 + c_lo) % m) % m;
    b[0..4].copy_from_slice(&(c_lo as u32).to_le_bytes());
    b[4..8].copy_from_slice(&(c_hi as u32).to_le_bytes());
}

struct NewRec {
    key: [u8; 64],
    klen: usize,
    val: [u8; 256],
    vlen: usize,
}

/// Rebuild a leaf block with the three new records inserted, keeping
/// b-tree key order (cmp = raw byte compare of the 8-byte type header,
/// then key bytes — matches APFS b-tree ordering for our key types).
fn leaf_insert(leaf: &mut [u8; BS], recs: &[&NewRec]) -> Result<(), &'static str> {
    let flags = rd16(leaf, 32);
    let nkeys = rd32(leaf, 36) as usize;
    let tlen = rd16(leaf, 42) as usize;
    let vals_start = if flags & BTNODE_ROOT != 0 { BS - 40 } else { BS };
    if nkeys + 3 > 128 || tlen + 24 > BS / 2 {
        return Err("leaf full (v1 does not split)");
    }

    // collect existing entries: (key bytes, val bytes)
    const MAX_LEAF: usize = 128;
    let mut keys: [&[u8]; MAX_LEAF] = [&[]; MAX_LEAF];
    let mut vals: [&[u8]; MAX_LEAF] = [&[]; MAX_LEAF];
    for i in 0..nkeys {
        let (k, v) = Node::new(leaf).entry(i as u32);
        keys[i] = k;
        vals[i] = v;
    }

    // build the combined key/val list sorted by key bytes
    let total = nkeys + 3;
    let mut order: [usize; MAX_LEAF + 3] = [0; MAX_LEAF + 3];
    for i in 0..total {
        order[i] = i;
    }
    // new records get indices nkeys..nkeys+3; simple insertion sort over all
    let key_of = |idx: usize| -> &[u8] {
        if idx < nkeys {
            keys[idx]
        } else {
            &recs[idx - nkeys].key[..recs[idx - nkeys].klen]
        }
    };
    for a in 1..total {
        let mut b = a;
        while b > 0 {
            let (x, y) = (order[b - 1], order[b]);
            if key_of(x) > key_of(y) {
                order.swap(b - 1, b);
                b -= 1;
            } else {
                break;
            }
        }
    }

    // rebuild the block (write into static LEAF_OUT to avoid stack overflow)
    let out = unsafe { &mut *core::ptr::addr_of_mut!(LEAF_OUT) };
    out.fill(0);
    out[..56].copy_from_slice(&leaf[..56]); // header (nkeys/tlen patched below)
    // toc + keys + vals are laid out linearly
    let keys_abs_start = 56 + tlen + 24; // toc grows by 3 entries
    let mut keys_write = keys_abs_start;
    let mut toc_entries: [(u16, u16, i16, i16); MAX_LEAF + 3] = [(0, 0, 0, 0); MAX_LEAF + 3];
    // vals grow downward from vals_start; place them in reverse entry order
    // (last key's val at the top) — physical order doesn't matter, offsets do
    let mut vtop = vals_start;
    let mut val_abs: [usize; MAX_LEAF + 3] = [0; MAX_LEAF + 3];
    for &idx in order[..total].iter().rev() {
        let vl = if idx < nkeys { vals[idx].len() } else { recs[idx - nkeys].vlen };
        vtop -= vl;
        val_abs[idx] = vtop;
    }
    // toc entries + key data
    for (slot, &idx) in order[..total].iter().enumerate() {
        let (kb, vb) = if idx < nkeys {
            (keys[idx], vals[idx])
        } else {
            let r = &recs[idx - nkeys];
            (&r.key[..r.klen], &r.val[..r.vlen])
        };
        let ko = (keys_write - keys_abs_start) as u16;
        let kl = kb.len() as u16;
        let vo = if vb.is_empty() { -1i16 } else { (vals_start - val_abs[idx]) as i16 };
        let vlen = vb.len() as i16;
        toc_entries[slot] = (ko, kl, vo, vlen);
        out[keys_write..keys_write + kb.len()].copy_from_slice(kb);
        keys_write += kb.len();
    }
    // write the toc
    for (slot, e) in toc_entries[..total].iter().enumerate() {
        let o = 56 + 8 * slot;
        out[o..o + 2].copy_from_slice(&e.0.to_le_bytes());
        out[o + 2..o + 4].copy_from_slice(&e.1.to_le_bytes());
        out[o + 4..o + 6].copy_from_slice(&e.2.to_le_bytes());
        out[o + 6..o + 8].copy_from_slice(&e.3.to_le_bytes());
    }
    // write the vals
    for &idx in order[..total].iter() {
        let vb = if idx < nkeys { vals[idx] } else { &recs[idx - nkeys].val[..recs[idx - nkeys].vlen] };
        if !vb.is_empty() {
            let off = val_abs[idx];
            out[off..off + vb.len()].copy_from_slice(vb);
        }
    }
    // patch header: nkeys, tlen (toc size), keep toff (child area) untouched
    let new_tlen = (tlen + 24) as u16;
    out[36..40].copy_from_slice(&(total as u32).to_le_bytes());
    out[42..44].copy_from_slice(&new_tlen.to_le_bytes());

    // sanity: the rebuilt block must parse back with the same entry count
    let chk = Node::new(out);
    if chk.nkeys as usize != total {
        return Err("leaf rebuild failed");
    }
    leaf.copy_from_slice(out);
    Ok(())
}

fn cmd_write(path: &str, text: &str) {
    // v1: root directory only (no '/' in path)
    if path.contains('/') || path.is_empty() {
        puts("write: v1 supports root-dir files only (no '/')\n");
        return;
    }
    if text.len() > 4000 {
        puts("write: text too long (max 4000 bytes, single block v1)\n");
        return;
    }
    let m = unsafe { (*core::ptr::addr_of!(MOUNT)).as_ref().unwrap() };
    if ndrec() + 3 > MAX_DRECS {
        puts("write: drec table full\n");
        return;
    }
    // reject duplicates
    for i in 0..ndrec() {
        if drecs()[i].parent == 2 && &drecs()[i].name[..drecs()[i].nlen] == path.as_bytes() {
            puts("write: file exists\n");
            return;
        }
    }
    // globally unique inode counter — init on first write from drecs+sizes
    static mut NEXT_INO: u64 = 0;
    let ino = unsafe {
        if NEXT_INO == 0 {
            let mut m = 2u64;
            for i in 0..ndrec() { m = m.max(drecs()[i].fid); }
            for i in 0..nsizes() { m = m.max(sizes()[i].0); }
            NEXT_INO = m + 1;
        }
        let v = NEXT_INO;
        NEXT_INO += 1;
        v
    };
    let nlen = path.len() + 1; // includes NUL, like existing entries
    if nlen > 60 {
        puts("write: name too long\n");
        return;
    }

    // templates from an existing file (hello.txt) in the leaf
    let mut drec_val_tpl = [0u8; 16];
    let mut inode_tpl: Option<(&[u8], u64)> = None; // (val bytes, its ino)
    let mut ext_val_tpl = [0u8; 24];
    let mut have_ext = false;
    {
        let b = block();
        if !read_block(m.fs_root, b) {
            puts("write: read leaf failed\n");
            return;
        }
        let n = Node::new(b);
        for i in 0..n.nkeys {
            let (k, v) = n.entry(i);
            let hdr = rd64(k, 0);
            let typ = (hdr >> 60) as u8;
            if typ == 9 && !v.is_empty() {
                if drec_val_tpl == [0u8; 16] {
                    drec_val_tpl[..16].copy_from_slice(&v[..16.min(v.len())]);
                }
            } else if typ == 3 && !v.is_empty() {
                let num = rd16(v, 92) as usize;
                if num > 0 && num <= 16 {
                    let has_dstream = (0..num).any(|e| v[96 + 4 * e] == 8);
                    let should_take = match inode_tpl {
                        None => true,
                        Some(_) => has_dstream,
                    };
                    if should_take {
                        let vlen = v.len();
                        let vcopy: &[u8] = unsafe { core::slice::from_raw_parts(v.as_ptr(), vlen) };
                        inode_tpl = Some((vcopy, hdr & 0x0FFF_FFFF_FFFF_FFFF));
                    }
                }
            } else if typ == 8 && !v.is_empty() && !have_ext {
                ext_val_tpl[..24.min(v.len())].copy_from_slice(&v[..24.min(v.len())]);
                have_ext = true;
            }
        }
    }
    let inode_tpl = match inode_tpl {
        Some(t) => t,
        None => {
            puts("write: no inode template found\n");
            return;
        }
    };

    // ---- record 1: drec key/val
    let r1 = unsafe { &mut *core::ptr::addr_of_mut!(R1) };
    *r1 = NewRec { key: [0; 64], klen: 0, val: [0; 256], vlen: 16 };
    let parent: u64 = 2;
    r1.key[0..8].copy_from_slice(&(0x9000_0000_0000_0000u64 | parent).to_le_bytes());
    let hsh = name_hash22(path.as_bytes());
    r1.key[8..12].copy_from_slice(&(((hsh << 10) | (nlen as u32 & 0x3FF)) as u32).to_le_bytes());
    let pbytes = path.as_bytes();
    r1.key[12..12 + pbytes.len()].copy_from_slice(pbytes);
    r1.key[12 + pbytes.len()] = 0; // NUL terminator
    r1.klen = 12 + nlen;
    r1.val[..16].copy_from_slice(&drec_val_tpl);
    r1.val[0..8].copy_from_slice(&ino.to_le_bytes()); // fid

    // ---- record 2: inode key/val (template with patched fields)
    let r2 = unsafe { &mut *core::ptr::addr_of_mut!(R2) };
    let (tpl, _tpl_ino) = inode_tpl;
    *r2 = NewRec { key: [0; 64], klen: 0, val: [0; 256], vlen: tpl.len() };
    r2.key[0..8].copy_from_slice(&(0x3000_0000_0000_0000u64 | ino).to_le_bytes());
    r2.klen = 8;
    r2.val[..tpl.len()].copy_from_slice(tpl);
    // inode fields: parent_id @0, private_id @8
    r2.val[0..8].copy_from_slice(&parent.to_le_bytes());
    r2.val[8..16].copy_from_slice(&ino.to_le_bytes());
    // DSTREAM xfield: find it in the template and patch the size
    let num = rd16(&r2.val, 92) as usize;
    let mut data = 96 + 4 * num;
    data = (data + 7) & !7;
    let mut patched = false;
    for e in 0..num {
        let xt = r2.val[96 + 4 * e];
        if xt == 8 {
            r2.val[data..data + 8].copy_from_slice(&(text.len() as u64).to_le_bytes());
            patched = true;
            break;
        }
        data = (data + rd16(&r2.val, 96 + 4 * e + 2) as usize + 7) & !7;
    }
    if !patched {
        puts("write: template inode has no DSTREAM\n");
        return;
    }

    // ---- record 3: file extent key/val
    let r3 = unsafe { &mut *core::ptr::addr_of_mut!(R3) };
    *r3 = NewRec { key: [0; 64], klen: 0, val: [0; 256], vlen: 24 };
    r3.key[0..8].copy_from_slice(&(0x8000_0000_0000_0000u64 | ino).to_le_bytes());
    r3.key[8..16].copy_from_slice(&0u64.to_le_bytes()); // logical offset 0
    r3.klen = 16;
    r3.val[..24].copy_from_slice(&ext_val_tpl);
    let len_off = ((text.len() as u64) << 8) | (u64::from_le_bytes(ext_val_tpl[0..8].try_into().unwrap()) & 0xFF);
    r3.val[0..8].copy_from_slice(&len_off.to_le_bytes());

    // ---- data block: first all-zero free block from DATA_AREA_START
    let phys = {
        let mut found = None;
        let mut pb = block2(); // reuse BLOCK2 as read scratch
        for cand in DATA_AREA_START..DATA_AREA_START + 64 {
            if !read_block(cand, pb) {
                break;
            }
            if pb.iter().all(|&x| x == 0) {
                found = Some(cand);
                break;
            }
        }
        match found {
            Some(p) => p,
            None => {
                puts("write: no free data block found\n");
                return;
            }
        }
    };
    let dblk = unsafe { &mut *core::ptr::addr_of_mut!(DBLK_BUF) };
    dblk.fill(0);
    dblk[..text.len()].copy_from_slice(text.as_bytes());
    // data blocks are not checksummed APFS objects — plain content
    if libe1::writeblocks(PART_LBA + (phys as u64) * 8, dblk) != BS as i64 {
        puts("write: data block write failed\n");
        return;
    }
    r3.val[8..16].copy_from_slice(&(phys as u64).to_le_bytes());

    // ---- leaf insert + checksum + write back
    let leaf = unsafe { &mut *core::ptr::addr_of_mut!(LEAF_BUF) };
    if !read_block(m.fs_root, leaf) {
        puts("write: leaf read failed\n");
        return;
    }
    let recs: [&NewRec; 3] = [r1, r2, r3];
    if let Err(e) = leaf_insert(leaf, &recs) {
        puts("write: ");
        puts(e);
        puts("\n");
        return;
    }
    fletcher64_store(leaf);
    if libe1::writeblocks(PART_LBA + (m.fs_root as u64) * 8, leaf) != BS as i64 {
        puts("write: leaf write failed\n");
        return;
    }
    // append new entries to in-memory tables so ls/cat see them
    if ndrec() < MAX_DRECS {
        let d = &mut drecs()[ndrec()];
        d.parent = 2;
        d.fid = ino;
        d.dt = 0;
        d.name[..nlen - 1].copy_from_slice(path.as_bytes());
        d.nlen = nlen - 1;
        unsafe { NDREC += 1; }
    }
    if nexts() < MAX_EXT {
        let e = &mut exts()[nexts()];
        e.ino = ino;
        e.logical = 0;
        e.phys = phys as u64;
        unsafe { NEXTS += 1; }
    }
    if nsizes() < MAX_SIZES {
        sizes()[nsizes()] = (ino, text.len() as u64);
        unsafe { NSIZES += 1; }
    }
    puts("write: created '");
    write(1, path.as_bytes());
    puts("' ino=");
    put_u64(ino);
    puts(" phys=");
    put_u64(phys as u64);
    puts(" size=");
    put_u64(text.len() as u64);
    puts("\n");
}

// ---------------------------------------------------------------- entry

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! {
    exit(1)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    puts("e1apfs: APFS read-only browser (second disk)\n");
    let m = match mount() {
        Ok(m) => m,
        Err(e) => {
            puts("mount failed: ");
            puts(e);
            puts("\n");
            exit(1);
        }
    };
    puts("volume ");
    puts(cstr(&m.volname));
    puts(" mounted\n");
    puts("commands: info | ls [path] | cat <path> | write <path> <text> | exit\n");

    let mut line = [0u8; 128];
    loop {
        puts("e1apfs> ");
        let n = read(0, &mut line);
        if n <= 0 {
            continue;
        }
        let s = cstr(&line[..n as usize]);
        let s = s.trim_end_matches(|c: char| c == '\r' || c == '\n' || c == ' ');
        if s == "exit" || s == "quit" {
            exit(0);
        } else if s == "info" {
            let m = unsafe { (*core::ptr::addr_of!(MOUNT)).as_ref().unwrap() };
            cmd_info(m);
        } else if s == "ls" {
            cmd_ls("");
        } else if let Some(p) = s.strip_prefix("ls ") {
            cmd_ls(p.trim());
        } else if let Some(p) = s.strip_prefix("cat ") {
            cmd_cat(p.trim());
        } else if let Some(p) = s.strip_prefix("write ") {
            let rest = p.trim();
            let mut parts = rest.splitn(2, ' ');
            let path = parts.next().unwrap_or("");
            let text = parts.next().unwrap_or("");
            if path.is_empty() {
                puts("write: usage: write <path> <text>\n");
            } else {
                cmd_write(path, text);
            }
        } else if !s.is_empty() {
            puts("unknown command\n");
        }
    }
}
