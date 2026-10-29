# DesKOS / tAbLeOS

A tiny from-scratch x86_64 hobby OS. MBR bootsector → stage2 → Rust kernel → ring3 userspace.

- **DesKOS** (`BRAND=desk`) — open source (MIT), this repository's public face
- **tAbLeOS** (`BRAND=table`) — closed source, proprietary variant

Both brands share the same codebase and produce the same disk-image layouts;
the split is purely a licensing / distribution boundary.

## Highlights

- **No Linux, no POSIX, no libc** — the kernel is 100% Rust (no std, no heap at
  boot), linked with a tiny amount of x86_64 assembly for ISRs and the two boot
  stages (MBR + stage2). Ring-3 programs link against a minimal `libe1` that
  wraps raw `syscall` instructions.
- **Tick-based cooperative scheduler** — `sched.rs` uses PIT timer IRQ 0 to
  drive multitasking; `ps` and `memmap` show process/state.
- **e1FSS filesystem** — a simple single-partition flat filesystem; `ls` / `cat`
  work from the `ush` shell, and user programs can open/read/write files via
  syscalls. Reboot-safe (metadata in fixed header, data in append-only blocks).
- **VGA text + framebuffer glyph console** — three-way output to UART, VGA text,
  and the VBE framebuffer (8×8 glyphs scaled 2×2 to 16×16 cells). IRQ-safe —
  the console lock disables interrupts during FB writes.
- **Server / Workstation dual branch**
  - `server`: UART-only headless, smallest footprint (good for VBox tcpserver
    serial, or headless CI)
  - `workstation`: boots the framebuffer glyph console so you can see output
    inside a GUI window
- **OpenRC-style init** — `openrc.rs` parses a tiny `/etc/init.d/` script tree,
  launches services in order, and supervises them.
- **100 one-time activation keys** — for public beta testing, attached to each
  release as `activation-keys.txt`.

## Quick Start

### Download prebuilt images

Get them from [GitHub Releases](../../releases):

| File | Firmware | Notes |
|---|---|---|
| `deskos-workstation-0.1-amd64.img` | BIOS | Framebuffer + VGA console (8 MB) |
| `deskos-server-0.1-amd64.img`      | BIOS | UART headless (8 MB) |

### Run in VirtualBox

```sh
# One-shot (builds + converts + boots headless with UART on localhost:4561)
./run-vbox.sh

# Or manually
VBoxManage convertfromraw deskos-workstation-0.1-amd64.img deskos.vdi --format VDI
# VirtualBox → New → Other/Unknown 64-bit → attach deskos.vdi to the IDE controller
```

The system boots straight into `ush`, the ring-3 shell. Try:

```
hello          # run the ring-3 "hello from ring3!" program
ps             # list processes
memmap         # memory map
help           # builtins
```

### Building from source

Requirements: macOS or Linux with `python3`, `cargo` (Rust nightly for
`x86_64-unknown-none`), and `zig` (used only for bootsector assembly —
downloaded once by the parent `e1LibreOS` build).

```sh
# DesKOS (open-source brand)
BRAND=desk EDITION=workstation ./build.sh     # GUI workstation
BRAND=desk EDITION=server      ./build.sh     # headless server

# Both brands × both editions (4 images)
./build.sh
```

Output lands in `build/`:
- `bootsect.bin`, `stage2.bin` — MBR + second stage bootloader
- `kernel.bin` — the Rust kernel
- `deskos-{server,workstation}-0.1-amd64.img` — bootable disk images

## Architecture at a glance

```
┌──────────────────────────────────────────────────┐
│ ring3  ush / hello / e1wm / e1dm / e1apfs / exaltop  │
│        (ELF, linked with libe1; raw syscall insns)  │
├──────────────────────────────────────────────────┤
│ syscall  →  syscall.rs  →  {vmm,sched,e1fss, ...}  │
├──────────────────────────────────────────────────┤
│ kernel  main.rs · vmm.rs · sched.rs · isr.S        │
│         idt.rs · gdt.rs · pmm.rs · heap.rs         │
│         console.rs · keyboard.rs · mouse.rs        │
│         pit.rs · pic.rs · ata.rs                    │
│         elf.rs · brand.rs · openrc.rs · e1fss.rs    │
├──────────────────────────────────────────────────┤
│ stage2  (zig-cc x86_64 asm → binary, relocates     │
│         kernel.bin to high canonical address, sets  │
│         up PML4, jumps to Rust main)                │
├──────────────────────────────────────────────────┤
│ MBR bootsect  (512 bytes, loads stage2 via int 13) │
└──────────────────────────────────────────────────┘
```

## Source layout

```
boot/           bootsect.S + stage2.S (MBR + second stage)
kernel/         Rust kernel — vmm.rs, sched.rs, syscall.rs, console.rs, ...
userspace/      ring-3 programs: ush, hello, libe1
apps/           extra ring-3 apps (e1wm, e1dm, e1apfs, e1vfsx, exaltop)
tools/          mke1fss.py, mkrit.py, mkkeys.py, mkfont_rs.py, mkimage.py
build.sh        one-shot build: bootsectors → kernel → apps → disk image
run-vbox.sh    create + boot a VirtualBox VM (headless or GUI)
sign_github.py M9 publish helper — creates GitHub repo + release + uploads
LICENSE         dual-brand (tAbLeOS closed, DesKOS MIT)
```

## License

Dual-brand — see [LICENSE](LICENSE):

- **tAbLeOS** (`BRAND=table`): closed source, all rights reserved
- **DesKOS** (`BRAND=desk`): MIT License, this repository

If you build this repository you are building the **DesKOS** variant.
