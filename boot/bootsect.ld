/* Link MBR at its runtime VMA 0x7C00, but flat-binary load addresses start 0
   so objcopy -O binary produces exactly 512 bytes with no leading padding. */
SECTIONS {
    . = 0x7C00;
    .text : AT(0) {
        *(.text*)
        *(.rodata*)
        *(.data*)
    }
  /DISCARD/ : { *(.eh_frame*) *(.note*) *(.comment*) }
}
