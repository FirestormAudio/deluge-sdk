/* The zero-init data: the MMU translation table, then `.bss` (the table first, so at the start of
 * RAM it needs no padding for its 16 KB alignment). build.rs publishes it as
 * zero_init_after_image.x (link.x's default place for it, between __image_end and the heap) or,
 * with the `zero-init-below-image` feature, as zero_init_below_image.x (at the start of RAM, before
 * the vector table), and publishes the other name empty. */

    /* The MMU translation table (rza1_hal::mmu::TTB, 16 KB, 16 KB-aligned), here rather than in
     * .bss. KEEP, because nothing references the input section by name. The reservation stays
     * TTB_SIZE whatever it holds; a table that outgrew it would move the location counter
     * backwards, which is a link error. */
    .ttb_mmu1 (NOLOAD) : ALIGN(0x4000) {
        ttb_mmu1_base = .;
        KEEP(*(.ttb_mmu1 .ttb_mmu1.*))
        . = ttb_mmu1_base + TTB_SIZE;
        . = ALIGN(4);
        ttb_mmu1_end = .;
    } > RAM

    .bss (NOLOAD) : ALIGN(4) {
        __bss_start__ = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4);
        __bss_end__ = .;
    } > RAM
