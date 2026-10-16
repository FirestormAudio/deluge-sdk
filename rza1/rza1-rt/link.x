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
 *   RZA1_STATUS_LED_PORT = ...; -- GPIO port of the status LED (1-11)
 *   RZA1_STATUS_LED_PIN  = ...; -- its pin (0-15)
 *
 * With rza1-rt's `rtt` feature, build.rs publishes link_rtt.x as link.x instead (it requires
 * RTT_RAM and NCACHE_RTT_RAM regions in memory.x).
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
/* Every other use is guarded with DEFINED(), so a missing symbol fails on the ASSERT above
 * rather than as a bare "symbol not found" at each use. */
ASSERT(!DEFINED(RZA1_STATUS_LED_PORT)
       || (RZA1_STATUS_LED_PORT >= 1 && RZA1_STATUS_LED_PORT <= 11),
       "RZA1_STATUS_LED_PORT must be 1-11 (P1-P11)")
ASSERT(!DEFINED(RZA1_STATUS_LED_PIN) || RZA1_STATUS_LED_PIN <= 15,
       "RZA1_STATUS_LED_PIN must be 0-15")
__rza1_status_led_port = DEFINED(RZA1_STATUS_LED_PORT) ? RZA1_STATUS_LED_PORT : 1;
__rza1_status_led_pin  = DEFINED(RZA1_STATUS_LED_PIN) ? RZA1_STATUS_LED_PIN : 0;
__rza1_status_led_p    = 0xFCFE3000 + 4 * __rza1_status_led_port;
__rza1_status_led_pm   = 0xFCFE3300 + 4 * __rza1_status_led_port;
__rza1_status_led_pmc  = 0xFCFE3400 + 4 * __rza1_status_led_port;
__rza1_status_led_pipc = 0xFCFE7200 + 4 * __rza1_status_led_port;
__rza1_status_led_mask = 1 << __rza1_status_led_pin;

SECTIONS {
    /*
     * Reserve the low internal SRAM window for the MMU translation table and
     * zero-init data, matching the vendor Deluge layout more closely.  The
     * firmware image itself then starts above this reserved region rather than
     * at 0x20020000 exactly.
     */
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

    /* SRAM heap: free space between the image and the stack reservation. The
     * Deluge C++ app gets these bounds via libdeluge/memory.h
     * (deluge_memory_region, FAST_INTERNAL → [__sram_heap_start, __sram_heap_end));
     * see src/bsp/rust/src/services.rs.
     *
     * It starts at the image's end, not at `end`: the 64 KB rounding is only the bootloader's copy
     * length, and the bytes it adds are free. A firmware whose image ends in data it no longer
     * needs after boot (a load copy it has moved elsewhere, say) defines `__sram_heap_base` to
     * hand that tail to the heap too. 16-byte aligned, as TLSF pools require. */
    __sram_heap_start = DEFINED(__sram_heap_base) ? __sram_heap_base : ALIGN(__image_end, 16);
    __sram_heap_end   = INTERNAL_RAM_END - PROGRAM_STACK_SIZE - ABT_STACK_SIZE - SVC_STACK_SIZE - FIQ_STACK_SIZE - IRQ_STACK_SIZE;
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
         * libdeluge/memory.h, so this symbol is no longer overloaded as the heap
         * top — it means the program stack, as the name says. */
        program_stack_start = .;
        . += PROGRAM_STACK_SIZE;
        program_stack_end = .;
    } > RAM

}
