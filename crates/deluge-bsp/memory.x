/* Memory layout of the Synthstrom Deluge (RZ/A1L), RTT disabled. deluge-bsp's build.rs puts it
 * on the linker search path as memory.x, and rza1-rt's link.x INCLUDEs it. */

MEMORY {
    /* 2.875 MB on-chip SRAM (pages 0–4, non-retention banks).
       0x20000000–0x2001FFFF is data-retention RAM; the bootloader refuses to
       copy a firmware image whose CODE_START falls below 0x20020000, so the
       firmware image must start at or above that address. */
    RAM (rwx) : ORIGIN = 0x20020000, LENGTH = 0x002E0000

    /* 64 MB external SDRAM (CS3) */
    SDRAM (rwx) : ORIGIN = 0x0C000000, LENGTH = 0x04000000
}

/* Stack sizes. PROVIDE, so a binary can override one from its build.rs with
 * `cargo:rustc-link-arg-bins=--defsym=PROGRAM_STACK_SIZE=<bytes>`, printed before its -Tlink.x
 * (lld applies --defsym in command-line order; after the script it would not move the stacks). */
PROVIDE(PROGRAM_STACK_SIZE = 0x8000);   /* 32 KB — application / SYS mode */
PROVIDE(IRQ_STACK_SIZE     = 0x2000);   /*  8 KB */
PROVIDE(FIQ_STACK_SIZE     = 0x2000);   /*  8 KB */
PROVIDE(SVC_STACK_SIZE     = 0x2000);   /*  8 KB */
PROVIDE(ABT_STACK_SIZE     = 0x2000);   /*  8 KB */

/* Top byte address of on-chip SRAM (used to anchor stack sections) */
INTERNAL_RAM_END = 0x20300000;
