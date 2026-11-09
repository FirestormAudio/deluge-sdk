OUTPUT_FORMAT("elf32-littlearm", "elf32-bigarm", "elf32-littlearm")
OUTPUT_ARCH(arm)
ENTRY(_start)

/*
 * Linker script for an RZ/A1 image that executes in place (XIP) from the memory-mapped SPI
 * flash window: code and read-only data stay in flash, .data is copied to RAM by the reset
 * handler (from __data_load__), .bss and the per-mode stacks live in RAM.
 *
 * The binary crate must provide memory_xip.x on the linker search path, defining:
 *
 *   MEMORY {
 *       FLASH (rx)  : ORIGIN = ..., LENGTH = ...   -- where the image executes, e.g. 0x18000000
 *       RAM   (rwx) : ORIGIN = ..., LENGTH = ...
 *   }
 *
 *   PROGRAM_STACK_SIZE, IRQ_STACK_SIZE, FIQ_STACK_SIZE, SVC_STACK_SIZE, ABT_STACK_SIZE
 *   INTERNAL_RAM_END   -- top byte address of the RAM the stacks hang from
 *
 * Unlike link.x there is no MMU translation table, SRAM heap or first-stage-bootloader
 * metadata: nothing that executes in place needs them yet. No RTT variant.
 */
INCLUDE memory_xip.x

PROVIDE(rza1_cpu_fault_report = __rza1_rt_default_fault_report);

/* See link.x: a board's boot header is pulled in by name. */
EXTERN(__rza1_boot_header)

/* One level-1 translation table, as in link.x. */
TTB_SIZE = 0x4000;

SECTIONS {
    .vector_table ORIGIN(FLASH) : ALIGN(0x20) {
        KEEP(*(.vector_table))
        KEEP(*(.boot_header))
    } > FLASH

    .text : ALIGN(4) {
        *(.text .text.*)
        . = ALIGN(4);
    } > FLASH

    .rodata : ALIGN(4) {
        *(.rodata .rodata.*)
        . = ALIGN(4);
    } > FLASH

    .ARM.extab : ALIGN(4) {
        *(.ARM.extab* .gnu.linkonce.armextab.*)
    } > FLASH
    .ARM.exidx : ALIGN(4) {
        __exidx_start = .;
        *(.ARM.exidx* .gnu.linkonce.armexidx.*)
        __exidx_end = .;
    } > FLASH

    /* Runs from RAM, loaded from flash; the reset handler copies it word by word. */
    .data : ALIGN(8) {
        __data_start__ = .;
        *(.data .data.*)
        . = ALIGN(8);
        __data_end__ = .;
    } > RAM AT> FLASH
    /* An empty .data loads from nowhere: its load address is its own (the reset handler then
     * skips the copy), so an image with no .data holds no absolute flash address for it. */
    __data_load__ = SIZEOF(.data) > 0 ? LOADADDR(.data) : __data_start__;

    /* The end of what has to be in flash. */
    __flash_image_end = LOADADDR(.data) + SIZEOF(.data);
    ASSERT(__flash_image_end <= ORIGIN(FLASH) + LENGTH(FLASH),
           "the image overruns its FLASH region")

    .bss (NOLOAD) : ALIGN(4) {
        __bss_start__ = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4);
        __bss_end__ = .;
    } > RAM

    /* The MMU translation table (rza1_hal::mmu::TTB), as link.x reserves it, so an XIP image that
     * turns the MMU on gets its 16 KB-aligned table in RAM rather than wherever an orphan section
     * would land. */
    .ttb_mmu1 (NOLOAD) : ALIGN(0x4000) {
        ttb_mmu1_base = .;
        KEEP(*(.ttb_mmu1 .ttb_mmu1.*))
        . = ttb_mmu1_base + TTB_SIZE;
        ttb_mmu1_end = .;
    } > RAM

    /* Exception-mode stacks, then the program stack at the top, as link.x places them. */
    .irq_stack (INTERNAL_RAM_END - PROGRAM_STACK_SIZE - ABT_STACK_SIZE - SVC_STACK_SIZE - FIQ_STACK_SIZE - IRQ_STACK_SIZE) (NOLOAD) : {
        irq_stack_start = .;
        . += IRQ_STACK_SIZE;
        irq_stack_end = .;
        fiq_stack_start = .;
        . += FIQ_STACK_SIZE;
        fiq_stack_end = .;
        svc_stack_start = .;
        . += SVC_STACK_SIZE;
        svc_stack_end = .;
        abt_stack_start = .;
        . += ABT_STACK_SIZE;
        abt_stack_end = .;
    } > RAM
    ASSERT(__bss_end__ <= irq_stack_start, "BSS runs into the stacks")

    .program_stack (INTERNAL_RAM_END - PROGRAM_STACK_SIZE) (NOLOAD) : {
        program_stack_start = .;
        . += PROGRAM_STACK_SIZE;
        program_stack_end = .;
    } > RAM
}
