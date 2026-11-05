/* Memory layout of xip-slot-blinky (RZ/A1L), linked with rza1-rt's link_xip.x.
 *
 * The image is the body of boot slot A: it executes in place right after the slot's 256-byte
 * header page, at flash offset 0x010100, and stage-0 branches to it there. It owns RAM from
 * then on, so it uses the same RAM as stage-0. */

MEMORY {
    /* Slot A (flash 0x010000, 448 KB) less its header page */
    FLASH (rx)  : ORIGIN = 0x18010100, LENGTH = 0x6FF00

    /* 64 KB of on-chip SRAM, bottom of the non-retention banks */
    RAM   (rwx) : ORIGIN = 0x20020000, LENGTH = 0x10000
}

PROGRAM_STACK_SIZE = 0x2000;   /* 8 KB — SYS mode */
IRQ_STACK_SIZE     = 0x400;
FIQ_STACK_SIZE     = 0x400;
SVC_STACK_SIZE     = 0x400;
ABT_STACK_SIZE     = 0x400;

INTERNAL_RAM_END = 0x20030000;
