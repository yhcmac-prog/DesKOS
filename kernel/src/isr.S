// Interrupt service routine stubs. Each stub pushes a uniform frame and calls
// the Rust handler `isr_dispatch`.
//
// Frame layout on entry to isr_dispatch (RDI points here):
//   [rdi+0]   vector
//   [rdi+8]   error_code
//   [rdi+16]  r15 ... (pushed regs, 15 * 8)
//   then CPU frame: rip, cs, rflags, (rsp, ss if ring3)
.intel_syntax noprefix
.section .text

.macro ISR_NOERR vec
.global isr_stub_\vec
isr_stub_\vec:
    push 0                    # fake error code
    push \vec                 # vector number
    jmp isr_common
.endm

.macro ISR_ERR vec
.global isr_stub_\vec
isr_stub_\vec:
    push \vec                 # vector number (error code already pushed by CPU)
    jmp isr_common
.endm

# `syscall` instruction entry (LSTAR, enabled via EFER.SCE for e1wine Linux
# binaries which use the x86_64 syscall convention instead of int 0x80).
# On entry: RCX = user RIP, R11 = user RFLAGS, RSP = user RSP.
# SFMASK cleared IF, so no interrupt can race the SCRATCH store below.
.global syscall_entry
syscall_entry:
    mov qword ptr [rip + syscall_ksp_scratch], rsp   # stash user RSP
    mov rsp, [rip + SYSCALL_RSP0]                    # switch to kernel stack
    push 0x23                                        # user ss
    push qword ptr [rip + syscall_ksp_scratch]       # user rsp
    push r11                                         # user rflags
    push 0x1b                                        # user cs
    push rcx                                         # user rip (return addr)
    push 0                                           # fake error code
    push 0x80                                        # vector
    jmp isr_common

.section .bss
.align 8
.global syscall_ksp_scratch
syscall_ksp_scratch:
    .space 8

.section .text
isr_common:
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rbp
    push rsi
    push rdi
    push rdx
    push rcx
    push rbx
    push rax
    mov rdi, rsp              # frame ptr
    call isr_dispatch
    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rdi
    pop rsi
    pop rbp
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
    add rsp, 16               # vector + error code
    iretq

# CPU exceptions 0-31
ISR_NOERR 0
ISR_NOERR 1
ISR_NOERR 2
ISR_NOERR 3
ISR_NOERR 4
ISR_NOERR 5
ISR_NOERR 6
ISR_NOERR 7
ISR_ERR   8
ISR_NOERR 9
ISR_ERR   10
ISR_ERR   11
ISR_ERR   12
ISR_ERR   13
ISR_ERR   14
ISR_NOERR 15
ISR_NOERR 16
ISR_ERR   17
ISR_NOERR 18
ISR_NOERR 19
ISR_NOERR 20
ISR_NOERR 21
ISR_NOERR 22
ISR_NOERR 23
ISR_NOERR 24
ISR_NOERR 25
ISR_NOERR 26
ISR_NOERR 27
ISR_NOERR 28
ISR_NOERR 29
ISR_NOERR 30
ISR_NOERR 31

# IRQs 0-15 -> vectors 32-47
ISR_NOERR 32
ISR_NOERR 33
ISR_NOERR 34
ISR_NOERR 35
ISR_NOERR 36
ISR_NOERR 37
ISR_NOERR 38
ISR_NOERR 39
ISR_NOERR 40
ISR_NOERR 41
ISR_NOERR 42
ISR_NOERR 43
ISR_NOERR 44
ISR_NOERR 45
ISR_NOERR 46
ISR_NOERR 47

# syscall -> vector 0x80
ISR_NOERR 128

# Build the stub table
.global isr_stub_table
.section .rodata
.align 8
isr_stub_table:
.irp v,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47
    .quad isr_stub_\v
.endr
.quad isr_stub_128
