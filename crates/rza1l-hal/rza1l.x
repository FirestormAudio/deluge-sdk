OUTPUT_FORMAT("elf32-littlearm", "elf32-bigarm", "elf32-littlearm")
OUTPUT_ARCH(arm)
ENTRY(_start)

/*
 * Linker script for RZ/A1L without SEGGER RTT.
 *
 * The binary crate (or BSP) must provide memory.x on the linker search path.
 * It must define (at minimum):
 *
 *   MEMORY {
 *       RAM   (rwx) : ORIGIN = ..., LENGTH = ...
 *       SDRAM (rwx) : ORIGIN = ..., LENGTH = ...  -- optional
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
 * For RTT-enabled builds use rza1l_rtt.x instead (requires RTT_RAM and
 * NCACHE_RTT_RAM regions in memory.x).
 */
INCLUDE memory.x

TTB_SIZE = 0x8000;

SECTIONS {
    /*
     * Reserve the low internal SRAM window for the MMU translation table and
     * zero-init data, matching the vendor Deluge layout more closely.  The
     * firmware image itself then starts above this reserved region rather than
     * at 0x20020000 exactly.
     */
    .ttb_mmu1 ORIGIN(RAM) (NOLOAD) : ALIGN(0x4000) {
        ttb_mmu1_base = .;
        . += TTB_SIZE;
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
     * works for consumers built with exceptions (the Synthstrom Deluge app
     * throws/catches deluge::exception). Placed in the image (before `end`) so
     * they're copied to SRAM and accounted for by the heap base. Rust firmwares
     * (panic=abort) emit little/none here — harmless. */
    .ARM.extab : ALIGN(4) {
        *(.ARM.extab* .gnu.linkonce.armextab.*)
    } > RAM
    .ARM.exidx : ALIGN(4) {
        __exidx_start = .;
        *(.ARM.exidx* .gnu.linkonce.armexidx.*)
        __exidx_end = .;
    } > RAM

    /*
     * Round up to next 64 KB boundary so the bootloader knows how much to
     * copy from SPI flash into SRAM (matches original linker script).
     */
    . = ALIGN(0x10000);
    end = .;
    _end = .;

    /* FSB metadata code_end: the FSB copies _start..code_end to SRAM. */
    __metadata_code_end = end;

    /* SRAM heap: free space between the image and the stack reservation */
    __sram_heap_start = end;
    __sram_heap_end   = INTERNAL_RAM_END - PROGRAM_STACK_SIZE - ABT_STACK_SIZE - SVC_STACK_SIZE - FIQ_STACK_SIZE - IRQ_STACK_SIZE;

    /* HACK(DELUGE_APP_HEAP_HACK): the Synthstrom Deluge C++ app's
     * GeneralMemoryAllocator reads &__heap_start as its internal heap BASE. It
     * must be the end of the whole image (== __sram_heap_start), not anything
     * earlier, or the app heap overlaps code/rodata. Point it there. REMOVE once
     * the app sources its heap bounds via libdeluge/memory.h (deluge_memory_*). */
    PROVIDE(__heap_start = __sram_heap_start);
    PROVIDE(__heap_start__ = __sram_heap_start);

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
        /* HACK(DELUGE_APP_HEAP_HACK): the Synthstrom Deluge C++ app's
         * GeneralMemoryAllocator reads &program_stack_start as the TOP of its
         * internal heap (assuming the original single-stack layout). Here the
         * per-mode exception stacks sit between the heap and the program stack,
         * so the real usable heap top is __sram_heap_end — point the symbol there
         * so the app's heap can't overrun the mode stacks. Only the C++ app reads
         * this symbol; startup uses program_stack_end for the SP. REMOVE once the
         * app sources its heap bounds via libdeluge/memory.h (deluge_memory_*). */
        program_stack_start = __sram_heap_end;
        . += PROGRAM_STACK_SIZE;
        program_stack_end = .;
    } > RAM

}
