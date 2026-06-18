/* Board-specific memory layout for the bootloader-flasher (RZ/A1L) — RTT enabled.
 * This file is INCLUDE'd by rza1l_rtt.x (from the rza1l-hal crate). */

MEMORY {
    RAM (rwx) : ORIGIN = 0x20020000, LENGTH = 0x002E0000

    /* 64 KB RTT ring buffer just below the stacks */
    RTT_RAM        (rw)  : ORIGIN = 0x202B0000, LENGTH = 0x00010000
    NCACHE_RTT_RAM (rw)  : ORIGIN = 0x602B0000, LENGTH = 0x00010000

    SDRAM (rwx) : ORIGIN = 0x0C000000, LENGTH = 0x04000000
}

PROGRAM_STACK_SIZE = 0x8000;
IRQ_STACK_SIZE     = 0x2000;
FIQ_STACK_SIZE     = 0x2000;
SVC_STACK_SIZE     = 0x2000;
ABT_STACK_SIZE     = 0x2000;

INTERNAL_RAM_END = 0x20300000;
