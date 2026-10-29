# tAbLeOS / DesKOS stage2 loader
# Loaded at 0x7E00 by bootsect. Runs in real mode, then transitions to long mode.
#
# Memory map used by this loader:
#   0x5000  E820 entries (24B each), count stored in bootinfo
#   0x6000  bootinfo struct (passed to kernel in RDI)
#   0x6800  VBE controller info block (512B)
#   0x6A00  VBE mode info block (256B)
#   0x7C00  bootsect / initial stack
#   0x7E00  this stage2 (up to 0xFAFF)
#   0x10000 kernel staging area (up to 0x90000, 512KB max)
#   0x70000 PML4 / 0x71000 PDPT / 0x72000-0x75000 PDx4 (identity map 4GB, 2MB pages)
#   0x100000 kernel final location
#
# bootinfo layout (0x6000):
#   +0x00 u32 magic 'tAbL' = 0x4C624174
#   +0x04 u32 kernel_size (bytes)
#   +0x08 u32 e820_count
#   +0x0C u32 vbe_mode
#   +0x10 u32 fb_phys
#   +0x14 u32 fb_pitch
#   +0x18 u16 fb_width
#   +0x1A u16 fb_height
#   +0x1C u8  fb_bpp
#   +0x1D u8  boot_drive
#   +0x1E u8  pad[2]

.intel_syntax noprefix
.code16
.section .text
.global stage2_start

stage2_start:
    jmp stage2_main
    .byte 0, 0
.global kernel_sectors
kernel_sectors: .long 0          # offset 4, patched by mkimage.py

stage2_main:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7000
    mov [boot_drive_save], dl

    # ---------------- E820 memory map ----------------
    mov di, 0x5000
    xor ebx, ebx
    xor bp, bp                    # entry count
e820_loop:
    mov eax, 0xE820
    mov ecx, 24
    mov edx, 0x534D4150           # 'SMAP'
    int 0x15
    jc e820_done
    cmp eax, 0x534D4150
    jne e820_done
    inc bp
    add di, 24
    test ebx, ebx
    jnz e820_loop
e820_done:
    mov [e820_count], bp

    # ---------------- VESA VBE mode set ----------------
    mov word ptr [chosen_mode], 0
    mov word ptr [fallback_mode], 0

    mov ax, 0x4F00
    mov di, 0x6800
    int 0x10
    cmp ax, 0x004F
    jne vesa_done
    mov eax, [0x6800]
    cmp eax, 0x41534556           # 'VESA'
    jne vesa_done

    # mode list far pointer at 0x6800+14
    mov si, [0x6800+14]
    mov ax, [0x6800+16]
    mov fs, ax
mode_scan:
    mov cx, fs:[si]
    cmp cx, 0xFFFF
    je mode_scan_done
    push si
    push cx
    mov ax, 0x4F01
    mov di, 0x6A00
    int 0x10
    pop cx
    pop si
    cmp ax, 0x004F
    jne mode_next
    # attributes: bit0 supported, bit4 graphics, bit7 LFB
    mov ax, [0x6A00]
    and ax, 0x91
    cmp ax, 0x91
    jne mode_next
    cmp byte ptr [0x6A00+25], 32  # bpp
    jne mode_next
    mov ax, [0x6A00+18]           # width
    mov bx, [0x6A00+20]           # height
    cmp ax, 1024
    jne try_640
    cmp bx, 768
    jne try_640
    mov [chosen_mode], cx
    jmp mode_scan_done
try_640:
    cmp ax, 640
    jne mode_next
    cmp bx, 480
    jne mode_next
    mov [fallback_mode], cx
mode_next:
    add si, 2
    jmp mode_scan
mode_scan_done:
    mov ax, [chosen_mode]
    or ax, ax
    jnz have_mode
    mov ax, [fallback_mode]
    or ax, ax
    jnz have_mode
    jmp vesa_done                 # no usable mode; kernel runs text-only
have_mode:
    mov [chosen_mode], ax
    # fetch mode info for chosen
    mov cx, ax
    mov ax, 0x4F01
    mov di, 0x6A00
    int 0x10
    cmp ax, 0x004F
    jne vesa_fail
    # set mode with LFB
    mov bx, [chosen_mode]
    or bx, 0x4000
    mov ax, 0x4F02
    int 0x10
    cmp ax, 0x004F
    jne vesa_fail
    # fill bootinfo framebuffer fields
    mov eax, [0x6A00+40]          # phys base
    mov [bi_fb_phys], eax
    movzx eax, word ptr [0x6A00+16]
    mov [bi_fb_pitch], eax
    mov ax, [0x6A00+18]
    mov [bi_fb_width], ax
    mov ax, [0x6A00+20]
    mov [bi_fb_height], ax
    mov al, [0x6A00+25]
    mov [bi_fb_bpp], al
    mov ax, [chosen_mode]
    mov [bi_vbe_mode], ax
    jmp vesa_done
vesa_fail:
    mov word ptr [chosen_mode], 0
vesa_done:

    # ---------------- load kernel to 0x10000 ----------------
    mov ax, 0x1000
    mov es, ax                    # dest segment
    mov ebx, 64                   # start LBA
    mov ebp, [kernel_sectors]     # remaining sectors
load_loop:
    test ebp, ebp
    jz load_done
    mov ecx, 64                   # chunk = 64 sectors (32KB)
    cmp ebp, 64
    jge chunk_ok
    mov ecx, ebp
chunk_ok:
    mov [dap2+2], cx              # count
    mov ax, es
    mov [dap2+6], ax              # segment
    mov [dap2+8], ebx             # lba low
    mov ah, 0x42
    mov dl, [boot_drive_save]
    lea si, [dap2]
    push es
    xor ax, ax
    mov es, ax
    mov ah, 0x42
    mov dl, [boot_drive_save]
    int 0x13
    pop es
    jc load_error
    movzx eax, word ptr [dap2+2]
    add ebx, eax                  # lba += count
    sub ebp, eax                  # remaining -= count
    shl eax, 5                    # count*512/16 = paragraphs
    mov cx, es
    add cx, ax
    mov es, cx
    jmp load_loop
load_error:
    xor ax, ax
    mov es, ax
    lea si, [err_load]
    call print16s2
    cli
    hlt
load_done:
    xor ax, ax
    mov es, ax

    # ---------------- enable A20 ----------------
    in al, 0x92
    or al, 2
    and al, 0xFE
    out 0x92, al

    # ---------------- build page tables ----------------
    # zero 0x70000-0x75FFF (6 pages)
    mov ax, 0x7000
    mov es, ax
    xor di, di
    xor eax, eax
    mov cx, 6*1024                # 6 pages * 4KB / 4 = 6144 dwords
    rep stosd

    # PML4[0] -> PDPT
    mov di, 0x0000
    mov dword ptr es:[di],   0x71003
    mov dword ptr es:[di+4], 0
    # PDPT[i] -> PD_i (i=0..3)
    mov di, 0x1000
    mov eax, 0x72003
    mov cx, 4
pdpt_fill:
    mov es:[di], eax
    mov dword ptr es:[di+4], 0
    add eax, 0x1000
    add di, 8
    loop pdpt_fill
    # PDs: 4 * 512 entries of 2MB pages
    mov di, 0x2000
    mov eax, 0x83                 # present | rw | PS
    mov cx, 4*512
pd_fill:
    mov es:[di], eax
    mov dword ptr es:[di+4], 0
    add eax, 0x200000
    add di, 8
    loop pd_fill

    # ---------------- enter long mode ----------------
    cli
    lgdt [gdt_ptr]

    mov eax, cr4
    or eax, 1 << 5                # CR4.PAE
    mov cr4, eax

    mov eax, 0x70000
    mov cr3, eax

    mov ecx, 0xC0000080           # EFER
    rdmsr
    or eax, 1 << 8                # LME
    wrmsr

    mov eax, cr0
    or eax, 0x80000001            # PG | PE
    mov cr0, eax

    .att_syntax prefix
    ljmp $0x08, $long_mode_start
    .intel_syntax noprefix

load_msg_str: .asciz ""
err_load:     .asciz "tAbLeOS: kernel load error\r\n"
boot_drive_save: .byte 0
e820_count:   .word 0
chosen_mode:  .word 0
fallback_mode:.word 0

print16s2:
    lodsb
    or al, al
    jz 1f
    mov ah, 0x0E
    int 0x10
    jmp print16s2
1:  ret

.align 4
dap2:
    .byte 0x10
    .byte 0
    .word 0           # +2 count
    .word 0           # +4 offset
    .word 0           # +6 segment
    .quad 0           # +8 lba
dap2_ptr = dap2

# GDT: null / code64 / data64
.align 8
gdt:
    .quad 0
    .quad 0x00AF9A000000FFFF      # code64: L=1
    .quad 0x00CF92000000FFFF      # data64
gdt_ptr:
    .word 23
    .long gdt

# bootinfo values collected in real mode (copied to 0x6000 in 64-bit step)
bi_vbe_mode:  .word 0, 0
bi_fb_phys:   .long 0
bi_fb_pitch:  .long 0
bi_fb_width:  .word 0
bi_fb_height: .word 0
bi_fb_bpp:    .byte 0

.code64
long_mode_start:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax

    # ---- write bootinfo at 0x6000 ----
    mov dword ptr [0x6000], 0x4C624174    # magic 'tAbL'
    mov eax, [kernel_sectors]
    shl eax, 9
    mov [0x6004], eax                     # kernel_size
    movzx eax, word ptr [e820_count]
    mov [0x6008], eax                     # e820_count
    movzx eax, word ptr [bi_vbe_mode]
    mov [0x600C], eax
    mov eax, [bi_fb_phys]
    mov [0x6010], eax
    mov eax, [bi_fb_pitch]
    mov [0x6014], eax
    mov ax, [bi_fb_width]
    mov [0x6018], ax
    mov ax, [bi_fb_height]
    mov [0x601A], ax
    mov al, [bi_fb_bpp]
    mov [0x601C], al
    mov al, [boot_drive_save]
    mov [0x601D], al
    mov byte ptr [0x601E], 0
    mov byte ptr [0x601F], 0

    # ---- copy kernel 0x10000 -> 0x100000 ----
    mov esi, 0x10000
    mov edi, 0x100000
    mov ecx, [kernel_sectors]
    shl ecx, 9
    rep movsb

    # ---- jump to kernel, RDI = bootinfo ----
    mov edi, 0x6000
    mov eax, 0x100000
    jmp rax
