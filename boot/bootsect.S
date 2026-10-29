# tAbLeOS / DesKOS MBR boot sector
# BIOS loads this at 0x7C00. Loads stage2 (LBA 1, 62 sectors) to 0x7E00 and jumps.
.intel_syntax noprefix
.code16
.section .text
.global _start

_start:
    cli
    # VGA marker: write 'B' (0x0E42) to first char cell at 0xB800
    mov ax, 0xB800
    mov es, ax
    xor ax, ax
    mov di, ax
    mov ax, 0x0E42
    stosw
    # Now continue normal boot

    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti
    mov [boot_drive], dl

    # INT 13h AH=42h EDD read: stage2
    mov ah, 0x42
    mov dl, [boot_drive]
    lea si, [dap]
    int 0x13
    jc disk_error

    .att_syntax prefix
    ljmp $0x0000, $0x7E00
    .intel_syntax noprefix

disk_error:
    lea si, [err_msg]
    call print16
    cli
    hlt

print16:
    lodsb
    or al, al
    jz 1f
    mov ah, 0x0E
    int 0x10
    jmp print16
1:  ret

err_msg:    .asciz "tAbLeOS: disk read error\r\n"
boot_drive: .byte 0

.align 4
dap:
    .byte 0x10          # DAP size
    .byte 0             # reserved
    .word 62            # sector count -> 31KB stage2
    .word 0x7E00        # buffer offset
    .word 0x0000        # buffer segment
    .quad 1             # start LBA

.org 510
.word 0xAA55
