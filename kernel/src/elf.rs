//! ELF64 static binary loader (no dynamic linking).

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ElfHeader {
    pub ident: [u8; 16],
    pub typ: u16,
    pub machine: u16,
    pub version: u32,
    pub entry: u64,
    pub phoff: u64,
    pub shoff: u64,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Phdr {
    pub typ: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

pub struct Elf {
    pub entry: u64,
    pub phoff: u64,
    pub phnum: usize,
    pub load_phdrs: [Phdr; 8],
    pub nloads: usize,
}

impl Elf {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < core::mem::size_of::<ElfHeader>() {
            return None;
        }
        let h = unsafe { &*(data.as_ptr() as *const ElfHeader) };
        if &h.ident[..4] != b"\x7FELF" {
            return None;
        }
        if h.ident[4] != 2 || h.ident[5] != 1 {
            return None; // not ELF64 or not little-endian
        }
        if h.phentsize as usize != core::mem::size_of::<Phdr>() {
            return None;
        }
        let phoff = h.phoff as usize;
        let phent = h.phnum as usize;
        if data.len() < phoff + phent * core::mem::size_of::<Phdr>() {
            return None;
        }
        let phdrs = unsafe {
            core::slice::from_raw_parts(
                data.as_ptr().add(phoff) as *const Phdr,
                phent
            )
        };
        let mut load_phdrs = [Phdr { typ: 0, flags: 0, offset: 0, vaddr: 0, paddr: 0, filesz: 0, memsz: 0, align: 0 }; 8];
        let mut nloads = 0;
        for ph in phdrs {
            if ph.typ == 1 && nloads < 8 { // PT_LOAD
                load_phdrs[nloads] = *ph;
                nloads += 1;
            }
        }
        if nloads == 0 {
            return None;
        }
        Some(Elf { entry: h.entry, phoff: h.phoff, phnum: phent, load_phdrs, nloads })
    }
}
