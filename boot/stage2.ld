/* Link stage2 at its runtime VMA 0x7E00; flat binary starts at LMA 0. */
ENTRY(stage2_start)
SECTIONS {
    . = 0x7E00;
    .text : AT(0) {
        *(.text*)
        *(.rodata*)
        *(.data*)
    }
  /DISCARD/ : { *(.eh_frame*) *(.note*) *(.comment*) }
}
