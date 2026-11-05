OUTPUT_FORMAT("elf32-littlearm", "elf32-bigarm", "elf32-littlearm")
OUTPUT_ARCH(arm)
ENTRY(_start)

/*
 * Linker script for the RZ/A1 with SEGGER RTT enabled.
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
 *
 * Published as link.x when rza1-rt's `rtt` feature is on; otherwise link.x is the RTT-free form.
 */
INCLUDE memory.x

/* One level-1 translation table: 4096 x 4 B, one 1 MB section per entry, TTBR0
 * only (TTBCR.N = 0) and no second-level tables, so nothing else needs room here. */
TTB_SIZE = 0x4000;

/* The fault hook: the image's `#[rza1_rt::fault]` handler (or a C `rza1_cpu_fault_report`), or
 * else rza1-rt's default, which parks the core. */
PROVIDE(rza1_cpu_fault_report = __rza1_rt_default_fault_report);

/* A board's boot header (see `.vector_table` below) is pulled in by name: nothing references it,
 * so without this its archive member would never be linked. The board labels the header
 * `__rza1_boot_header`; an image with no boot header leaves the symbol undefined, which is fine. */
EXTERN(__rza1_boot_header)

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
    /* NOLOAD, so it survives a reset holding whatever the last program left: startup zeroes it
       (__rtt_buffer_start..__rtt_buffer_end) so that a fault before rtt_init finds no ring. */
    .rtt_buffer (NOLOAD) : ALIGN(4) {
        __rtt_buffer_start = .;
        KEEP(*(.rtt_buffer .rtt_buffer.*))
        /* 32-byte cache line: keeps any cached allocation above the buffer from
           sharing a line with it, so a fill/evict through the cached alias cannot
           write back over what the CPU wrote uncached. */
        . = ALIGN(32);
        __rtt_buffer_end = .;
    } > NCACHE_RTT_RAM

    /*
     * The vector table, the first loadable bytes of the image, then the board's boot header, if
     * it has one: a board crate emits the words and signature its first-stage loader checks into
     * `.boot_header`, which therefore lands at `_start + 0x20`. Optional; nothing requires it.
     */
    .vector_table ORIGIN(RAM) : ALIGN(0x20) {
        KEEP(*(.vector_table))
        KEEP(*(.boot_header))
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

    /* The reset handler copies .data from __data_load__ when that differs from __data_start__
     * (an XIP image, link_xip.x); here the two are equal and the copy is skipped. */
    .data : ALIGN(8) {
        __data_start__ = .;
        *(.data .data.*)
        . = ALIGN(8);
        __data_end__ = .;
    } > RAM
    __data_load__ = LOADADDR(.data);

    /* ARM EH unwind tables, kept rather than discarded so C++ exception unwinding works for
     * code built with exceptions. In the image, so a loader copies them. Rust firmwares
     * (panic=abort) emit little or nothing here. */
    .ARM.extab : ALIGN(4) {
        *(.ARM.extab* .gnu.linkonce.armextab.*)
    } > RAM
    .ARM.exidx : ALIGN(4) {
        __exidx_start = .;
        *(.ARM.exidx* .gnu.linkonce.armexidx.*)
        __exidx_end = .;
    } > RAM

    /* The end of the image: every byte a loader must copy. A board that needs a rounded copy length
     * (a first-stage loader's code_end, say) computes it from __image_end in its memory.x. */
    __image_end = .;

    .bss (NOLOAD) : ALIGN(4) {
        __bss_start__ = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4);
        __bss_end__ = .;
    } > RAM

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

    end = .;
    _end = .;

    /* SRAM heap: the free space between the image's zero-init data and the RTT/stack
     * reservation. A firmware with memory it no longer needs after boot (a load copy it has moved
     * elsewhere, say) defines `__sram_heap_base` to start the heap lower. 16-byte aligned, as
     * TLSF pools require. */
    __sram_heap_start = DEFINED(__sram_heap_base) ? __sram_heap_base : ALIGN(end, 16);
    __sram_heap_end   = ORIGIN(RTT_RAM);
    /* The per-mode stacks sit above the RTT reserve. Without this check, stacks that grew into
     * it fail later as an unexplained section overlap. */
    ASSERT(INTERNAL_RAM_END - PROGRAM_STACK_SIZE - ABT_STACK_SIZE - SVC_STACK_SIZE - FIQ_STACK_SIZE
           - IRQ_STACK_SIZE >= ORIGIN(RTT_RAM) + LENGTH(RTT_RAM),
           "the stacks overlap the RTT reserve: with RTT on, the per-mode stacks must fit between RTT_RAM's end and INTERNAL_RAM_END (is a --defsym'd PROGRAM_STACK_SIZE meant for a build without RTT?)")
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
        /* program_stack_start is the stack base, for code that measures stack headroom;
         * startup uses program_stack_end as the initial SP. */
        program_stack_start = .;
        . += PROGRAM_STACK_SIZE;
        program_stack_end = .;
    } > RAM
}
