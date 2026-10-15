OUTPUT_FORMAT("elf32-littlearm", "elf32-bigarm", "elf32-littlearm")
OUTPUT_ARCH(arm)
ENTRY(_start)

/*
 * Linker script for RZ/A1L with SEGGER RTT enabled.
 *
 * The binary crate (or BSP) must provide memory.x on the linker search path.
 * It must define (at minimum):
 *
 *   MEMORY {
 *       RAM            (rwx) : ORIGIN = ..., LENGTH = ...
 *       RTT_RAM        (rw)  : ORIGIN = ..., LENGTH = ...  -- cached alias for RTT page
 *       NCACHE_RTT_RAM (rw)  : ORIGIN = ..., LENGTH = ...  -- uncached alias (same physical page + 0x40000000)
 *       SDRAM          (rwx) : ORIGIN = ..., LENGTH = ...  -- optional
 *   }
 *
 *   PROGRAM_STACK_SIZE = ...;
 *   IRQ_STACK_SIZE     = ...;
 *   FIQ_STACK_SIZE     = ...;
 *   SVC_STACK_SIZE     = ...;
 *   ABT_STACK_SIZE     = ...;
 *
 *   INTERNAL_RAM_END   = ...;   -- top byte address of on-chip SRAM
 *   RZA1_STATUS_LED_PORT = ...; -- GPIO port of the status LED (1-11)
 *   RZA1_STATUS_LED_PIN  = ...; -- its pin (0-15)
 *
 * Published as link.x when rza1-rt's `rtt` feature is on; otherwise link.x is the RTT-free form.
 */
INCLUDE memory.x

/* One level-1 translation table: 4096 x 4 B, one 1 MB section per entry, TTBR0
 * only (TTBCR.N = 0) and no second-level tables, so nothing else needs room here. */
TTB_SIZE = 0x4000;

/* The status LED the reset and fault handlers drive (startup.rs). The port's GPIO registers are
 * 16 bits wide and 4 bytes apart per port (RZ/A1L hardware manual, ch. 54: Pn, PMn, PMCn,
 * PIPCn; matches rza1l_hal::gpio). Port 0 is input-only, so it has no PM/PIPC register. */
ASSERT(DEFINED(RZA1_STATUS_LED_PORT) && DEFINED(RZA1_STATUS_LED_PIN),
       "memory.x must define RZA1_STATUS_LED_PORT and RZA1_STATUS_LED_PIN")
ASSERT(RZA1_STATUS_LED_PORT >= 1 && RZA1_STATUS_LED_PORT <= 11,
       "RZA1_STATUS_LED_PORT must be 1-11 (P1-P11)")
ASSERT(RZA1_STATUS_LED_PIN <= 15, "RZA1_STATUS_LED_PIN must be 0-15")
__rza1_status_led_p    = 0xFCFE3000 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_pm   = 0xFCFE3300 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_pmc  = 0xFCFE3400 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_pipc = 0xFCFE7200 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_mask = 1 << RZA1_STATUS_LED_PIN;

SECTIONS {
    /* _SEGGER_RTT control block in the uncached SRAM mirror so the debug
       probe can always see RTT writes, even after D-cache is enabled.
       RTT_RAM (cached) and NCACHE_RTT_RAM (uncached) are the same physical
       bytes — the mirror is a hardware alias at +0x40000000, not an MMU mapping
       — so reserving the cached view too is what stops BSS or stacks landing on
       top of the buffer.
       LENGTH(RTT_RAM) rather than a literal: the region size and the reserve must
       agree, and the memory map sizes the region to the actual buffer (see
       memory_rtt.x). A literal here would silently under- or over-reserve the
       moment that size changed. */
    .rtt_cached_reserve (NOLOAD) : ALIGN(4) { . += LENGTH(RTT_RAM); } > RTT_RAM
    .rtt_buffer (NOLOAD) : ALIGN(4) {
        KEEP(*(.rtt_buffer .rtt_buffer.*))
        /* 32-byte cache line: keeps any cached allocation above the buffer from
           sharing a line with it, so a fill/evict through the cached alias cannot
           write back over what the CPU wrote uncached. */
        . = ALIGN(32);
    } > NCACHE_RTT_RAM

    /* Match the vendor Deluge layout more closely: reserve low SRAM for the
       MMU translation table and BSS, and place the first loadable code above
       that reserved region. */
    .ttb_mmu1 ORIGIN(RAM) (NOLOAD) : ALIGN(0x4000) {
        ttb_mmu1_base = .;
        /* rza1l_hal::mmu::TTB itself (16 KB, 16 KB-aligned), here rather than in
         * .bss. KEEP, because nothing references the input section by name. The
         * reservation stays TTB_SIZE whatever it holds; a table that outgrew it
         * would move the location counter backwards, which is a link error. */
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

    /*
     * Vector table + bootloader metadata.
     * Must be the first loadable bytes of the binary.
     * The bootloader reads fixed offsets past the 8 vector entries:
     *   +0x20: code_start   (word: address of _start)
     *   +0x24: code_end     (word: address of end)
     *   +0x28: code_execute (word: execution entry = _start)
     *   +0x2C: ".BootLoad_ValidProgramTest." signature
     */
    .vector_table : ALIGN(0x20) {
        KEEP(*(.vector_table))
    } > RAM

    /* Main code */
    .text : ALIGN(4) {
        *(.text .text.*)
        . = ALIGN(4);
    } > RAM

    .rodata : ALIGN(4) {
        *(.rodata .rodata.*)
        . = ALIGN(4);
    } > RAM

    .data : ALIGN(8) {
        *(.data .data.*)
        . = ALIGN(8);
    } > RAM

    /* ARM EH unwind tables — kept (not discarded) so C++ exception unwinding
     * works for consumers built with exceptions (the Deluge app throws/catches
     * deluge::exception). In the image (before `end`). Rust firmwares emit
     * little/none here — harmless. */
    .ARM.extab : ALIGN(4) {
        *(.ARM.extab* .gnu.linkonce.armextab.*)
    } > RAM
    .ARM.exidx : ALIGN(4) {
        __exidx_start = .;
        *(.ARM.exidx* .gnu.linkonce.armexidx.*)
        __exidx_end = .;
    } > RAM

    /* The end of the image: everything the bootloader must copy. */
    __image_end = .;

    /*
     * Round up to next 64 KB boundary so the bootloader knows how much to
     * copy from SPI flash into SRAM (matches original linker script).
     */
    . = ALIGN(0x10000);
    end = .;
    _end = .;

    /* FSB metadata code_end: the FSB copies _start..code_end to SRAM. */
    __metadata_code_end = end;

    /* SRAM heap: free space between the image and the RTT/stack reservation. The
     * Deluge C++ app gets these bounds via libdeluge/memory.h
     * (deluge_memory_region, FAST_INTERNAL → [__sram_heap_start, __sram_heap_end));
     * see src/bsp/rust/src/services.rs.
     *
     * It starts at the image's end, not at `end`: the 64 KB rounding is only the bootloader's copy
     * length, and the bytes it adds are free. A firmware whose image ends in data it no longer
     * needs after boot (a load copy it has moved elsewhere, say) defines `__sram_heap_base` to
     * hand that tail to the heap too. 16-byte aligned, as TLSF pools require. */
    __sram_heap_start = DEFINED(__sram_heap_base) ? __sram_heap_base : ALIGN(__image_end, 16);
    __sram_heap_end   = ORIGIN(RTT_RAM);
    ASSERT(__sram_heap_start <= __sram_heap_end,
           "the image overruns the SRAM heap: it ends past __sram_heap_end")

    /*
     * Exception-mode stacks — NOLOAD, placed just below the program stack.
     * Stacks grow downward; _end symbols are the initial SP values.
     */
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

    /* Program (SYS mode) stack at the very top of SRAM */
    .program_stack (INTERNAL_RAM_END - PROGRAM_STACK_SIZE) (NOLOAD) : {
        /* program_stack_start is the stack base; the C++ app's checkStack() guard
         * measures stack headroom against it. (Startup uses program_stack_end as
         * the initial SP.) The app's internal *heap* bounds now come from
         * libdeluge/memory.h, so this symbol means the program stack, as named. */
        program_stack_start = .;
        . += PROGRAM_STACK_SIZE;
        program_stack_end = .;
    } > RAM
}
