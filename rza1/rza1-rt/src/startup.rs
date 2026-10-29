//! Reset vector, exception handlers and the GIC IRQ entry stub for the RZ/A1L, in ARM-mode
//! assembly.
//!
//! The reset handler invalidates caches/TLBs, installs the vector table via
//! VBAR, parks secondary cores, unlocks retention RAM, enables VFP/NEON, sets
//! up the per-mode stacks, zeroes BSS and calls `main` (`#[rza1_rt::entry]`).
//!
//! Every fault vector (undefined instruction, SVC, prefetch abort, data abort, reserved) works out
//! the address of the faulting instruction and, with the `rtt` feature, first dumps the fault
//! registers to RTT channel 0. It then hands over to `rza1_cpu_fault_report` (`#[rza1_rt::fault]`),
//! or, in an image that defines none, parks the core.
//!
//! The vector table is 8 branches and nothing else: a board's boot header (the words and
//! signature its first-stage loader checks) goes in the `.boot_header` section, which link.x
//! places straight after it, at `_start + 0x20`.

use core::arch::global_asm;

global_asm!(
    r#"
    /*
     * Vector table: section .vector_table, the first loadable bytes of the image. Branches
     * rather than `ldr pc, =label`, so there is no literal pool to land between the table and
     * a board's `.boot_header` at +0x20.
     */
    .section .vector_table, "ax"
    .code 32
    .global _start
_start:
    b _reset_handler              /* 0x00  Reset                   */
    b _undef_handler              /* 0x04  Undefined instruction  */
    b _svc_handler                /* 0x08  Supervisor call        */
    b _prefetch_handler           /* 0x0C  Prefetch abort         */
    b _abort_handler              /* 0x10  Data abort             */
    b _reserved_handler           /* 0x14  Reserved               */
    b _irq_handler                /* 0x18  IRQ                    */
    b _fiq_handler                /* 0x1C  FIQ                    */

    /*
     * Reset handler — minimal startup for first-milestone bare-metal boot.
     *
     * Steps:
     *   1. Multi-core guard (only core 0 proceeds)
     *   2. Disable I-cache, D-cache, MMU (clean slate — skip full MMU init
     *      for now; we run entirely from internal SRAM)
     *   3. Enable NEON/VFP coprocessor access
     *   4. Set up per-mode stacks
     *   5. Zero BSS
     *   6. Branch to `main` (`#[rza1_rt::entry]`)
     */
    .section .text._reset_handler, "ax"
    .code 32
    .global _reset_handler
_reset_handler:
    /* Mask IRQ, FIQ and async aborts immediately — old firmware may have
     * left CPSR with interrupts enabled. With VBAR=0 any stray exception
     * would branch into SPI flash (the old bootloader at 0x00000000). */
    cpsid aif

    /*
     * 1. Stop every DMAC channel the previous program left running. A debugger
     * load or a jump from a loader is not a chip reset, so a circular channel
     * (an SSI or SCIF RX ring in link-descriptor mode) keeps writing into
     * memory this image now owns: BSS zeroed below would be overwritten again.
     * Per channel, CLREN then SWRST (as the HAL's dmac::raw::stop does); then
     * release all DMARS request routes, which SWRST does not reset.
     */
    ldr r0, =0xE8200028           /* CHCTRL_0; channels 0-7 every 0x40 */
    mov r2, #8
    bl _dmac_stop_group
    ldr r0, =0xE8200428           /* CHCTRL_8; channels 8-15 every 0x40 */
    mov r2, #8
    bl _dmac_stop_group
    ldr r0, =0xFCFE1000           /* DMARS0..7 */
    mov r1, #0
    mov r2, #8
1:
    str r1, [r0], #4
    subs r2, r2, #1
    bne 1b
    dsb

    /*
     * 1a. Disable MMU, D-cache, I-cache — leave HIVEC alone for now.
     *
     * Do NOT clear HIVEC here. HIVEC must stay set (ROM vectors) until VBAR
     * is written, so any exception in this window still reaches the ROM
     * handlers rather than branching to address 0.
     */
    mrc p15, 0, r0, c1, c0, 0    /* Read SCTLR                   */
    bic r0, r0, #(1 << 12)       /* I-cache off                  */
    bic r0, r0, #(1 << 2)        /* D-cache off                  */
    bic r0, r0, #(1 << 0)        /* MMU off                      */
    mcr p15, 0, r0, c1, c0, 0
    isb
    dsb

    /*
     * 1b. Invalidate TLBs, I-cache, and D-cache.
     *
     * ARM mandates these invalidations before re-enabling caches/MMU because
     * the cache and TLB state is UNDEFINED after reset (or after a soft
     * reset from previous firmware).  Stale "valid" tags would cause the
     * CPU to return garbage on the first cached load.
     *
     * Mirrors R_CACHE_L1Init / reset_handler.S from the Renesas C BSP.
     */
    /* Invalidate entire Unified/Data TLB */
    mov  r0, #0
    mcr  p15, 0, r0, c8, c7, 0   /* TLBIALL                      */

    /* Invalidate I-cache (also flushes branch target cache) */
    mov  r0, #0
    mcr  p15, 0, r0, c7, c5, 0   /* ICIALLU                      */
    isb

    /*
     * Invalidate all D-cache levels by iterating every Set and Way.
     *
     * Faithfully ported from reset_handler.S (both Deluge and Azure RTOS
     * BSPs are identical here — Renesas reference implementation).
     *
     * Register usage (mirrors original):
     *   r0  = CLIDR
     *   r1  = CCSIDR
     *   r2  = line-length shift
     *   r3  = number of cache levels << 1 (from CLIDR LoC field)
     *   r4  = max way index (CCSIDR[12:3])
     *   r5  = CLZ(max_way) — way bit-position in DCISW word
     *   r7  = current set counter (outer loop, counts down to 0)
     *   r9  = current way counter (inner loop, counts down to 0)
     *   r10 = current cache level << 1
     *   r11 = DCISW argument
     *
     * Loop structure (outer = set, inner = way) matches the original exactly.
     * r7 must NOT be inside the way loop or it resets on every way iteration.
     */
    mrc  p15, 1, r0, c0, c0, 1   /* Read CLIDR                   */
    ands r3, r0, #0x07000000      /* Extract LoC field            */
    mov  r3, r3, lsr #23          /* Total cache levels << 1      */
    beq  .Ldcache_inval_done
    mov  r10, #0                  /* Start at level 0             */
.Ldcache_level_loop:
    add  r2, r10, r10, lsr #1     /* r2 = 3 × level               */
    mov  r1, r0, lsr r2           /* Shift CLIDR to this level    */
    and  r1, r1, #7               /* Isolate 3-bit cache type     */
    cmp  r1, #2
    blt  .Ldcache_skip            /* No D-cache at this level     */
    mcr  p15, 2, r10, c0, c0, 0  /* CSSELR = this level          */
    isb                           /* Sync before reading CCSIDR   */
    mrc  p15, 1, r1, c0, c0, 0   /* Read CCSIDR                  */
    and  r2, r1, #7               /* Line-length field            */
    add  r2, r2, #4               /* +4 → log2(bytes per line)    */
    ldr  r4, =0x3FF
    ands r4, r4, r1, lsr #3       /* Max way  = CCSIDR[12:3]      */
    clz  r5, r4                   /* Way bit-position in DCISW    */
    ldr  r7, =0x7FFF
    ands r7, r7, r1, lsr #13      /* Max set  = CCSIDR[27:13]     */
.Ldcache_set_loop:                /* Outer loop: iterate sets     */
    mov  r9, r4                   /* Reset way counter each set   */
.Ldcache_way_loop:                /* Inner loop: iterate ways     */
    orr  r11, r10, r9, lsl r5    /* Encode level | way           */
    orr  r11, r11, r7, lsl r2    /* Encode set                   */
    mcr  p15, 0, r11, c7, c6, 2  /* DCISW — invalidate by S/W    */
    subs r9, r9, #1
    bge  .Ldcache_way_loop        /* Next way                     */
    subs r7, r7, #1
    bge  .Ldcache_set_loop        /* Next set                     */
.Ldcache_skip:
    add  r10, r10, #2
    cmp  r3, r10
    bgt  .Ldcache_level_loop      /* Next cache level             */
.Ldcache_inval_done:
    dsb
    isb

    /* 1d. Set VBAR to our vector table BEFORE clearing HIVEC. */
    ldr r0, =_start
    mcr p15, 0, r0, c12, c0, 0   /* Write VBAR                   */
    isb

    /* 1e. Now safe to clear HIVEC — exceptions will use VBAR = _start.
     *     Also clear TE (bit 30, Thumb Exceptions) so exception vectors
     *     execute in ARM state.  Old firmware (e.g. Deluge) runs in Thumb
     *     mode and sets TE=1; our vector table uses ARM `ldr pc, [pc, #n]`
     *     entries which decode as undefined Thumb-2 when TE=1. */
    mrc p15, 0, r0, c1, c0, 0
    bic r0, r0, #(1 << 13)       /* Clear HIVEC (use low vectors) */
    bic r0, r0, #(1 << 30)       /* Clear TE (ARM exception entry) */
    mcr p15, 0, r0, c1, c0, 0
    isb

    /* 2. Multi-core guard: put secondary cores to sleep.
     * CPU0 must NOT execute WFI here or cold boot can stall forever waiting
     * for an interrupt before the firmware has enabled its own handlers. */
    mrc p15, 0, r0, c0, c0, 5    /* Read MPIDR                   */
    ands r0, r0, #0x3             /* Aff0 = CPU ID in bits[1:0] (Cortex-A9 MPCore) */
    beq .Lprimary_core            /* CPU0 continues immediately   */
.Lsecondary_core_sleep:
    wfine                         /* Secondary core: wait for int */
    b .Lsecondary_core_sleep      /* Never continue on non-zero cores */
.Lprimary_core:

    /*
     * 3. Enable CPU writes to data-retention RAM (0x20000000–0x2001FFFF).
     *
     * CPG.SYSCR3 at 0xFCFE0408 — bit[3:0] enable the four retention RAM
     * banks. Reset default is 0x00 (all banks write-protected). Without
     * this, any CPU store to retention RAM causes a Data Abort.
     *
     * Safe to access now: MMU is disabled, so 0xFCFE0408 is a direct
     * physical access to the CPG peripheral (Strongly-Ordered).
     *
     * Dummy read after write is required by the hardware spec.
     */
    ldr r1, =0xFCFE0408           /* CPG.SYSCR3 address           */
    mov r0, #0x0F                 /* Enable all 4 banks           */
    strb r0, [r1]
    ldrb r0, [r1]                 /* Dummy read                   */

    /*
     * Enable NEON/VFP coprocessor access.
     *
     * CPACR[23:22]=CP11 and CPACR[21:20]=CP10 are set to 0b11 (full access
     * in privileged and unprivileged modes).  Then FPEXC.EN (bit 30) is set
     * to power on the VFP/NEON hardware.
     *
     * This does NOT require STB/CPG clock init — VFP is CPU-internal.
     * It must be done here (before Rust) because stb::init() in rust_main
     * may itself emit VFP instructions (the toolchain is free to use NEON
     * for memset etc. once features are re-enabled).
     */
    mrc p15, 0, r0, c1, c0, 2    /* Read CPACR                   */
    orr r0, r0, #(0xF << 20)     /* Enable CP10 and CP11         */
    mcr p15, 0, r0, c1, c0, 2    /* Write CPACR                  */
    isb
    mov r0, #0x40000000           /* FPEXC.EN = 1                 */
    vmsr FPEXC, r0

    /* 3. Set up per-mode stacks and clear stale banked registers.
     *
     * For each exception mode: set SP, clear LR (= 0, causes PABT at known
     * address rather than jumping to stale previous-firmware code), and set
     * SPSR = SYS-mode ARM CPSR (0x1DF) so any spurious exception return
     * restores SYS mode rather than whatever the previous firmware left.
     *
     * This guards against the case where a FIQ fires before our own interrupt
     * initialisation is complete: `subs pc, lr, #4` in the FIQ handler uses
     * both LR_fiq and SPSR_fiq.  If those contain stale values the return
     * branches into random memory and/or restores a wrong CPSR mode.
     *
     * Safe SPSR value: SYS mode (bits[4:0]=11111), ARM state (T=0),
     * IRQ disabled (I=1), async-abort disabled (A=1) = 0x000001DF.
     * (NMFI means F cannot be set; write is attempted anyway.)
     */
    ldr r0, =0x000001DF           /* safe SPSR value (SYS ARM I+A disabled) */
    mov r1, #0                    /* safe LR  value (PABT at 0 if misused)  */

    msr cpsr_c, #0xD2             /* IRQ mode, I+F masked         */
    ldr sp, =irq_stack_end
    mov lr, r1
    msr spsr_fsxc, r0

    msr cpsr_c, #0xD1             /* FIQ mode                     */
    ldr sp, =fiq_stack_end
    mov lr, r1
    msr spsr_fsxc, r0

    msr cpsr_c, #0xD3             /* SVC mode                     */
    ldr sp, =svc_stack_end
    mov lr, r1
    msr spsr_fsxc, r0

    msr cpsr_c, #0xD7             /* ABT mode                     */
    ldr sp, =abt_stack_end
    mov lr, r1
    msr spsr_fsxc, r0

    /* Note: UNDEF mode is intentionally NOT entered here.
     * Our UNDEF handler is a bare infinite loop with no stack.
     * probe-rs resets SPSR_und and LR_und to safe values before restart.
     * Entering UNDEF mode briefly to init it would create a window where
     * a nested FIQ could return to UNDEF mode unexpectedly. */

    msr cpsr_c, #0xDF             /* SYS mode (application)       */
    ldr sp, =program_stack_end

    /* 5. Zero BSS */
    ldr r0, =__bss_start__
    ldr r1, =__bss_end__
    mov r2, #0
.Lbss_loop:
    cmp r0, r1
    strlt r2, [r0], #4
    blt .Lbss_loop

    /* 6. Branch to Rust entry point (never returns) */
    bl main
    b .                           /* Safety: infinite loop        */

_dmac_stop_group:
    /* Stop r2 DMAC channels whose CHCTRL registers start at r0, 0x40 apart:
     * CLREN (bit 1) ends the transfer, then SWRST (bit 3) clears the channel's
     * status. Clobbers r0-r3. */
    mov r1, #(1 << 1)
    mov r3, #(1 << 3)
2:
    str r1, [r0]
    dsb
    str r3, [r0]
    add r0, r0, #0x40
    subs r2, r2, #1
    bne 2b
    bx lr

    /* Fault vectors. Each one sets up the same two registers and then goes on to its RTT dump
     * (when the `rtt` feature has one) and the shared tail `_fault_report`:
     *   r5 = fault type passed to the reporter (0 undefined, 1 prefetch abort,
     *        2 data abort, 3 reserved, 0xF SVC)
     *   r6 = address of the faulting instruction
     * The LR an exception leaves behind is a fixed distance past that instruction, and for
     * undefined instructions and SVCs the distance depends on the instruction set it was in
     * (ARMv7-A ARM B1.9.6-B1.9.8, B1.9.10): undefined/SVC LR = instruction + 4 (ARM) or + 2
     * (Thumb, SPSR.T set); prefetch abort LR = instruction + 4 and data abort LR = instruction + 8
     * in either state. */
    .global _undef_handler
    .global _svc_handler
    .global _prefetch_handler
    .global _abort_handler
    .global _reserved_handler
    .global _fiq_handler
_undef_handler:
    mov   r5, #0
    mrs   r6, spsr
    tst   r6, #0x20               /* SPSR.T: faulted in Thumb state */
    subne r6, lr, #2
    subeq r6, lr, #4
    b     _undef_rtt
_svc_handler:
    /* Not a fault as such, but nothing makes system calls: report the SVC itself. */
    mov   r5, #0xF
    mrs   r6, spsr
    tst   r6, #0x20
    subne r6, lr, #2
    subeq r6, lr, #4
    b     _fault_report
_prefetch_handler:
    mov   r5, #1
    sub   r6, lr, #4
    b     _pabt_rtt
_abort_handler:
    mov   r5, #2
    sub   r6, lr, #8
    b     _abort_rtt
_reserved_handler:
    /* Never taken on the Cortex-A9 (it is the Hyp trap vector); LR is reported as it stands. */
    mov   r5, #3
    mov   r6, lr
    b     _fault_report
    /* FIQ handler: return from interrupt.
     * On FIQ entry LR_fiq = interrupted PC + 4; subtract 4 to re-run
     * the interrupted instruction.  SUBS restores CPSR from SPSR_fiq.
     * Uses only the FIQ-banked r8–r12/lr so no save/restore needed. */
_fiq_handler:       subs pc, lr, #4
"#
);

// RTT-enabled data-abort body: dumps "DABT\n  PC=XXXXXXXX DFAR=XXXXXXXX DFSR=XXXXXXXX\n"
// (where XXXXXXXX are 8 ASCII hex digits) to RTT channel 0, then halts.
//
// On Data Abort entry:
//   LR_abt = faulting PC + 8  (so faulting PC = LR - 8)
//   SPSR_abt = CPSR of aborted mode
// CP15 c6 c0 0 = DFAR (Data Fault Address Register)
// CP15 c5 c0 0 = DFSR (Data Fault Status Register)
#[cfg(feature = "rtt")]
global_asm!(
    r#"
_abort_rtt:
    /* Save scratch registers (we're in ABT mode with banked LR/SP). */
    push {{r5, r6, r7, r8, r9, r10}}

    /* Compute faulting PC = LR_abt - 8. */
    sub  r5, lr, #8               /* r5 = faulting PC              */

    /* Read DFAR and DFSR from CP15. */
    mrc  p15, 0, r6, c6, c0, 0   /* r6 = DFAR                     */
    mrc  p15, 0, r7, c5, c0, 0   /* r7 = DFSR                     */

    /* rtt-target 0.6 RttControlBlock layout (32-bit ARM, usize = 4 bytes):
     *   _SEGGER_RTT + 0x1C  acUp[0].buffer  (*mut u8)
     *   _SEGGER_RTT + 0x24  acUp[0].write   (AtomicUsize / WrOff)
     */
    ldr  r0, =_SEGGER_RTT
    ldr  r1, [r0, #0x1C]          /* r1 = buffer base              */
    ldr  r2, [r0, #0x24]          /* r2 = WrOff                    */

    /* Write "DABT\n  PC=" (9 bytes) */
    ldr  r3, =0x54424144          /* "DABT"                        */
    str  r3, [r1, r2]
    add  r2, r2, #4
    ldr  r3, =0x43205c6e          /* "\n  C"  (little-endian)      */
    str  r3, [r1, r2]
    add  r2, r2, #4
    ldr  r3, =0x3d435750          /* "PC=C" (we overwrite last C)  */
    /* Actually write "  PC=" as 5 bytes — easier to do nibble-by-nibble below */
    /* Restart: write the whole tag line as 4-byte chunks.
       Prefix header = "DABT\n  PC=" = 10 bytes.
       Use a simpler byte-by-byte approach via a helper macro. */
    /* Back up WrOff to redo — overwrite the partial writes with correct data. */
    ldr  r2, [r0, #0x24]          /* re-read original WrOff        */

    /* Inline helper: write_hex8(r1+r2, r_val) — 8 hex chars, advances r2 by 8.
       Uses r3, r8, r9, r10 as scratch. */
    .macro write_hex8 reg
    mov  r9, \reg
    mov  r10, #28
.Lhex_loop_\@:
    lsr  r8, r9, r10
    and  r8, r8, #0xF
    cmp  r8, #10
    addlt r8, r8, #0x30
    addge r8, r8, #0x37
    strb r8, [r1, r2]
    add  r2, r2, #1
    subs r10, r10, #4
    bge  .Lhex_loop_\@
    .endm

    .macro write_char ch
    mov  r8, \ch
    strb r8, [r1, r2]
    add  r2, r2, #1
    .endm

    /* Write "DABT" */
    write_char #0x44
    write_char #0x41
    write_char #0x42
    write_char #0x54
    write_char #0x0A              /* '\n' */
    /* Write "  PC=" */
    write_char #0x20
    write_char #0x20
    write_char #0x50
    write_char #0x43
    write_char #0x3D
    write_hex8 r5                 /* faulting PC */
    write_char #0x0A
    /* Write "DFAR=" */
    write_char #0x44
    write_char #0x46
    write_char #0x41
    write_char #0x52
    write_char #0x3D
    write_hex8 r6                 /* DFAR */
    write_char #0x0A
    /* Write "DFSR=" */
    write_char #0x44
    write_char #0x46
    write_char #0x53
    write_char #0x52
    write_char #0x3D
    write_hex8 r7                 /* DFSR */
    write_char #0x0A

    str  r2, [r0, #0x24]          /* WrOff = updated value         */

    pop  {{r5, r6, r7, r8, r9, r10}}
    b    _fault_report
"#
);

// No RTT: nothing to dump, straight on to the report.
#[cfg(not(feature = "rtt"))]
global_asm!(
    r#"
_abort_rtt:
_undef_rtt:
_pabt_rtt:
    b    _fault_report
"#
);

// RTT-enabled undefined-instruction handler: dumps "UNDEF\n  PC=XXXXXXXX\n"
#[cfg(feature = "rtt")]
global_asm!(
    r#"
_undef_rtt:
    /* Give Undefined mode a stack before touching one. Boot deliberately never
       initialises SP_und (see the note in the reset sequence), so the push below was
       writing 28 bytes to whatever SP_und happened to hold -- corrupting arbitrary
       memory on the way to reporting the fault, which is how a dump ends up with a
       garbage LR in it. Borrow the abort stack: UND and ABT are both terminal here
       (neither handler ever returns), so they cannot legitimately be live at once, and
       an abort taken while reporting an undefined instruction is unrecoverable anyway.
       Clobbering SP_und is free for the same reason. */
    ldr  sp, =abt_stack_end
    push {{r5, r6, r7, r8, r9, r10, r11}}
    mov  r5, r6                   /* faulting PC, from the vector */
    /* Capture the faulting (SYS/USR) mode SP and LR: USR/SYS share r13/r14, so
       briefly switching to System mode reads the app's stack pointer and its LR —
       the return address of a wild blx, i.e. the call site to symbolize. IRQs
       stay masked across the mode switch. */
    cps  #0x1F                    /* System mode */
    mov  r6, sp                   /* r6 = app SP */
    mov  r7, lr                   /* r7 = app LR (wild-call return address) */
    cps  #0x1B                    /* back to Undefined mode */
    ldr  r0, =_SEGGER_RTT
    ldr  r1, [r0, #0x1C]          /* buffer base */
    ldr  r2, [r0, #0x24]          /* WrOff */
    .macro write_char_u ch
    mov  r8, \ch
    strb r8, [r1, r2]
    add  r2, r2, #1
    .endm
    .macro write_hex8_u reg
    mov  r9, \reg
    mov  r10, #28
.Lhex_loop_u_\@:
    lsr  r8, r9, r10
    and  r8, r8, #0xF
    cmp  r8, #10
    addlt r8, r8, #0x30
    addge r8, r8, #0x37
    strb r8, [r1, r2]
    add  r2, r2, #1
    subs r10, r10, #4
    bge  .Lhex_loop_u_\@
    .endm
    write_char_u #0x55   /* U */
    write_char_u #0x4E   /* N */
    write_char_u #0x44   /* D */
    write_char_u #0x45   /* E */
    write_char_u #0x46   /* F */
    write_char_u #0x0A   /* \n */
    write_char_u #0x20
    write_char_u #0x20
    write_char_u #0x50   /* P */
    write_char_u #0x43   /* C */
    write_char_u #0x3D   /* = */
    write_hex8_u r5
    write_char_u #0x0A
    /* LR= : app return address — symbolize via the .map to find the wild call. */
    write_char_u #0x4C   /* L */
    write_char_u #0x52   /* R */
    write_char_u #0x3D   /* = */
    write_hex8_u r7
    write_char_u #0x0A
    /* SP= : app stack pointer. */
    write_char_u #0x53   /* S */
    write_char_u #0x50   /* P */
    write_char_u #0x3D   /* = */
    write_hex8_u r6
    write_char_u #0x0A
    /* Commit PC/LR/SP now, so they survive even if the stack walk below faults
       (a bad SP would nest a data abort). */
    str  r2, [r0, #0x24]
    /* STK: 16 words from the app stack — scan for code-range return addresses to
       reconstruct the call chain via the .map file. */
    write_char_u #0x53   /* S */
    write_char_u #0x54   /* T */
    write_char_u #0x4B   /* K */
    write_char_u #0x3A   /* : */
    mov  r11, #16
.Lstk_u:
    ldr  r5, [r6]
    add  r6, r6, #4
    write_char_u #0x20   /* space */
    write_hex8_u r5
    subs r11, r11, #1
    bne  .Lstk_u
    write_char_u #0x0A
    str  r2, [r0, #0x24]
    pop  {{r5, r6, r7, r8, r9, r10, r11}}
    b    _fault_report
"#
);

// The shared tail of every fault vector, entered in the exception's mode with interrupts masked
// and r5/r6 set up as described at the vectors.
//
// It calls `rza1_cpu_fault_report(fault_pc, fault_type, sys_lr, sys_sp)`: the image's
// `#[rza1_rt::fault]` handler, or a C function of that name (the Deluge firmware's pad-grid
// crash reporter is one). In an image that defines neither, link.x PROVIDEs the default below,
// which parks the core. `sys_lr`/`sys_sp` are the interrupted code's LR and SP, read by briefly
// switching to System mode, which shares them with User mode: LR is the return address of the
// call that went wrong, and SP is where the reporter walks for the rest of the call chain. The
// reporter runs on the abort stack, never the interrupted one, which may be the very thing that
// overflowed. It does not return.
global_asm!(
    r#"
_fault_report:
    ldr  sp, =abt_stack_end       /* terminal: borrowing the abort stack is safe */
    mrs  r0, cpsr
    cps  #0x1F                    /* System mode, interrupts still masked */
    mov  r2, lr                   /* sys_lr */
    mov  r3, sp                   /* sys_sp */
    msr  cpsr_c, r0               /* back to the exception's mode and stack */
    mov  r0, r6                   /* fault_pc */
    mov  r1, r5                   /* fault_type */
    ldr  r12, =rza1_cpu_fault_report
    bx   r12

    /* The default reporter (link.x PROVIDEs it as rza1_cpu_fault_report): park the core. A
     * PROVIDE rather than a weak symbol, so a reporter in an archive member (a C library's,
     * say) is still pulled in to resolve the reference. */
    .pushsection .text.__rza1_rt_default_fault_report, "ax"
    .code 32
    .global __rza1_rt_default_fault_report
    .type __rza1_rt_default_fault_report, %function
__rza1_rt_default_fault_report:
    wfi
    b    __rza1_rt_default_fault_report
    .popsection
"#
);

// RTT-enabled prefetch-abort handler: dumps "PABT\n  PC=XXXXXXXX IFSR=XXXXXXXX\n"
#[cfg(feature = "rtt")]
global_asm!(
    r#"
_pabt_rtt:
    /* On PABT entry: LR_abt = faulting PC + 4 */
    push {{r5, r6, r7, r8, r9, r10}}
    sub  r5, lr, #4               /* faulting PC */
    mrc  p15, 0, r6, c5, c0, 1   /* IFSR */
    mrc  p15, 0, r7, c6, c0, 2   /* IFAR */
    ldr  r0, =_SEGGER_RTT
    ldr  r1, [r0, #0x1C]
    ldr  r2, [r0, #0x24]
    .macro write_char_p ch
    mov  r8, \ch
    strb r8, [r1, r2]
    add  r2, r2, #1
    .endm
    .macro write_hex8_p reg
    mov  r9, \reg
    mov  r10, #28
.Lhex_loop_p_\@:
    lsr  r8, r9, r10
    and  r8, r8, #0xF
    cmp  r8, #10
    addlt r8, r8, #0x30
    addge r8, r8, #0x37
    strb r8, [r1, r2]
    add  r2, r2, #1
    subs r10, r10, #4
    bge  .Lhex_loop_p_\@
    .endm
    write_char_p #0x50   /* P */
    write_char_p #0x41   /* A */
    write_char_p #0x42   /* B */
    write_char_p #0x54   /* T */
    write_char_p #0x0A
    write_char_p #0x20
    write_char_p #0x20
    write_char_p #0x50   /* P */
    write_char_p #0x43   /* C */
    write_char_p #0x3D   /* = */
    write_hex8_p r5
    write_char_p #0x0A
    write_char_p #0x49   /* I */
    write_char_p #0x46   /* F */
    write_char_p #0x53   /* S */
    write_char_p #0x52   /* R */
    write_char_p #0x3D
    write_hex8_p r6
    write_char_p #0x0A
    write_char_p #0x49   /* I */
    write_char_p #0x46   /* F */
    write_char_p #0x41   /* A */
    write_char_p #0x52   /* R */
    write_char_p #0x3D
    write_hex8_p r7
    write_char_p #0x0A
    str  r2, [r0, #0x24]
    pop  {{r5, r6, r7, r8, r9, r10}}
    b    _fault_report
"#
);

// ---------------------------------------------------------------------------
// IRQ handler — full GIC dispatch with ARM errata workarounds
// ---------------------------------------------------------------------------
//
// This is a faithful port of `irqfiq_handler.S` from the Renesas BSP,
// adapted to call the Rust `gic_dispatch(icciar)` function.
//
// ARM errata handled:
//   801120  — dummy ICCHPIR read before ICCIAR ensures correct data
//   733075  — spurious ID 0 or >=1022: read-modify-write ICDIPR0, re-read ICCIAR
//
// Register usage on entry (IRQ mode, IRQ/FIQ disabled):
//   LR_irq = interrupted PC + 4
//   SPSR_irq = CPSR of interrupted mode
//
// Stack layout built by this handler (IRQ stack → SYS stack):
//   IRQ stack: [LR_irq - 4] [SPSR]
//   SYS stack: r0-r3, r12               (caller-saved AAPCS)
//              d0-d31, FPSCR (8-byte slot)  (interrupted VFP/NEON state)
//              alignment pad (0 or 4)
//              r0(=icciar), r1(=pad), r2, r3, r4, lr_sys   (around bl gic_dispatch)

global_asm!(
    r#"
    .equ GICC_IAR_ADDR,   0xE820200C
    .equ GICC_EOIR_ADDR,  0xE8202010
    .equ GICC_HPPIR_ADDR, 0xE8202018
    .equ GICD_IPR0_ADDR,  0xE8201400
    .equ SYS_MODE, 0x1F
    .equ IRQ_MODE, 0x12

    .section .text._irq_handler, "ax"
    .code 32
    .global _irq_handler
    .type _irq_handler, %function
_irq_handler:
    /* Adjust LR: on IRQ entry LR_irq = interrupted PC + 4; subtract 4
     * so MOVS PC,LR returns to the correct instruction. */
    sub     lr, lr, #4

    /* Push return address and SPSR onto the IRQ-mode stack. */
    push    {{lr}}
    mrs     lr, spsr
    push    {{lr}}

    /* Switch to SYS mode (same stack as application; IRQ/FIQ still masked). */
    cps     #SYS_MODE

    /* Save caller-saved AAPCS registers. */
    push    {{r0-r3, r12}}

    /* Save the interrupted context's VFP/NEON state. Required because the audio
     * interrupt-executor runs the NEON-heavy audio render inside an IRQ handler;
     * without this it would clobber d0-d31/FPSCR of whatever thread-mode code it
     * preempted. r0 is already saved above, so it is free as scratch here.
     * (vpush is word-aligned; the bl below is realigned to 8 bytes separately.) */
    vpush   {{d0-d15}}
    vpush   {{d16-d31}}
    vmrs    r0, fpscr
    sub     sp, sp, #8
    str     r0, [sp]

    /* ---- ARM Errata 801120: dummy HPPIR read before ICCIAR ---- */
    ldr     r2, =GICC_HPPIR_ADDR
    ldr     r2, [r2]

    /* Read Interrupt Acknowledge Register -> r3 = raw icciar */
    ldr     r2, =GICC_IAR_ADDR
    ldr     r3, [r2]

    /* Extract interrupt ID (bits [9:0]) for errata-733075 check. */
    ubfx    r0, r3, #0, #10

    /* ---- ARM Errata 733075: SGI ID 0 or spurious IDs >= 1022 ---- */
    cmp     r0, #0
    beq     .Lirq_errata_733075
    ldr     r1, =1022
    cmp     r0, r1
    bge     .Lirq_errata_733075
    b       .Lirq_post_errata

.Lirq_errata_733075:
    /* Read-modify-write ICDIPR0 to resolve the errata, then re-read ICCIAR. */
    ldr     r2, =GICD_IPR0_ADDR
    ldr     r0, [r2]
    str     r0, [r2]
    dsb
    ldr     r2, =GICC_HPPIR_ADDR
    ldr     r2, [r2]
    ldr     r2, =GICC_IAR_ADDR
    ldr     r3, [r2]

.Lirq_post_errata:
    /* r0 = full icciar (used for both gic_dispatch arg and ICCEOIR write). */
    mov     r0, r3

    /* Align stack to 8 bytes (AAPCS requirement for BL). */
    mov     r1, sp
    and     r1, r1, #4
    sub     sp, sp, r1

    /* Push: r0(icciar), r1(pad amount), r2, r3, r4(padding reg), lr_sys. */
    push    {{r0-r4, lr}}

    /* Call Rust dispatch: gic_dispatch(icciar) -- r0 already set. */
    bl      gic_dispatch

    /* Restore registers; r0 restored to icciar for the EOI write below. */
    pop     {{r0-r4, lr}}

    /* Undo stack alignment. */
    add     sp, sp, r1

    /* Disable IRQ, memory + instruction barriers, then write EOI.
     * TRM §7.8.3: do NOT write ICCEOIR for spurious IDs 1022/1023.
     * Extract int_id bits [9:0] from r0; skip EOI if id >= 1022. */
    cpsid   i
    dsb
    isb
    ubfx    r3, r0, #0, #10
    ldr     r2, =1022
    cmp     r3, r2
    bge     .Lirq_skip_eoi
    ldr     r2, =GICC_EOIR_ADDR
    str     r0, [r2]
.Lirq_skip_eoi:

    /* Restore the interrupted context's VFP/NEON state (reverse of the save).
     * SP points at the FPSCR slot here: the alignment pad and the gic_dispatch
     * call frame have already been unwound (pop {{r0-r4,lr}} + add sp,sp,r1).
     * r0 holds the (now-consumed) icciar and is restored by the pop below. */
    ldr     r0, [sp]
    add     sp, sp, #8
    vmsr    fpscr, r0
    vpop    {{d16-d31}}
    vpop    {{d0-d15}}

    /* Restore caller-saved registers. */
    pop     {{r0-r3, r12}}

    /* Return to interrupted mode: switch to IRQ mode, restore SPSR + LR. */
    cps     #IRQ_MODE
    pop     {{lr}}
    msr     spsr_cxsf, lr
    pop     {{lr}}
    movs    pc, lr          /* return from IRQ exception */

    .size _irq_handler, . - _irq_handler
"#
);
