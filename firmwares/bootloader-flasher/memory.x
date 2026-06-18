/* Board-specific memory layout for the bootloader-flasher (RZ/A1L) — RTT disabled.
 * This file is INCLUDE'd by rza1l.x (from the rza1l-hal crate).
 *
 * The flasher is loaded directly into SRAM over SWD/JTAG (J-Link) and runs from
 * SRAM only — never from flash — so it can take the flash bus into manual SPI
 * mode and erase/program sector 0 (the FSB) safely. It uses the same SRAM window
 * as the app-loader and main firmware. */

MEMORY {
    /* 2.875 MB on-chip SRAM (pages 0–4, non-retention banks).
       0x20000000–0x2001FFFF is data-retention RAM. */
    RAM (rwx) : ORIGIN = 0x20020000, LENGTH = 0x002E0000

    /* 64 MB external SDRAM (CS3) — used as the image staging buffer */
    SDRAM (rwx) : ORIGIN = 0x0C000000, LENGTH = 0x04000000
}

/* Stack sizes */
PROGRAM_STACK_SIZE = 0x8000;   /* 32 KB */
IRQ_STACK_SIZE     = 0x2000;   /*  8 KB */
FIQ_STACK_SIZE     = 0x2000;   /*  8 KB */
SVC_STACK_SIZE     = 0x2000;   /*  8 KB */
ABT_STACK_SIZE     = 0x2000;   /*  8 KB */

/* Top byte address of on-chip SRAM */
INTERNAL_RAM_END = 0x20300000;
