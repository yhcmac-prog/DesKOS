/* tAbLeOS user program: linked at 0x400000 */
ENTRY(_start)

SECTIONS {
    . = 0x400000;

    .text : { *(.text .text.*) }

    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) }

    . = ALIGN(4096);
    .data : { *(.data .data.*) }

    . = ALIGN(4096);
    .bss : { *(.bss .bss.*) *(COMMON) }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment*)
    }
}
