/* Memory layout of stage-0 (RZ/A1L), linked with rza1-rt's link_xip.x.
 *
 * Stage-0 executes in place from the start of the SPI flash window, where the boot ROM branches
 * after setting the controller to plain 03h reads. Its RAM is only needed until it hands off:
 * the boot it enters owns all of RAM afterwards. The ROM's own work area
 * (0x20020000-0x20023FFF) is finished with by the time stage-0 runs. */

MEMORY {
    /* Flash offset 0x000000, one 64 KB erase block */
    FLASH (rx)  : ORIGIN = 0x18000000, LENGTH = 0x10000

    /* 64 KB of on-chip SRAM, bottom of the non-retention banks */
    RAM   (rwx) : ORIGIN = 0x20020000, LENGTH = 0x10000
}

PROGRAM_STACK_SIZE = 0x2000;   /* 8 KB — SYS mode */
IRQ_STACK_SIZE     = 0x400;
FIQ_STACK_SIZE     = 0x400;
SVC_STACK_SIZE     = 0x400;
ABT_STACK_SIZE     = 0x400;

INTERNAL_RAM_END = 0x20030000;
