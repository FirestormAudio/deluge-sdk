//! Generic Interrupt Controller (GIC) driver for the Cortex-A9 / RZ/A1L.
//!
//! The RZ/A1L uses the ARM PrimeCell GIC (PL390, GICv1) split into:
//!   - GIC Distributor (GICD) at 0xE820_1000  (the Renesas "INTC" peripheral)
//!   - GIC CPU Interface (GICC) at 0xE820_2000
//!
//! The RZ/A1L has 538 interrupt sources (IDs 0–537), the RZ/A1H 587 (IDs 0–586): [`INT_ID_TOTAL`].
//!
//! ## Initialization sequence
//! Call [`init`] once before enabling global IRQ (`cpsie i`).
//! Drivers install handlers through [`crate::interrupt::bind`], and enable individual sources via
//! [`enable`].
//!
//! ## IRQ dispatch
//! The assembly IRQ handler in `startup.rs` calls `gic_dispatch` with the raw
//! ICCIAR register value. This function looks up and calls the registered Rust
//! handler (with IRQs re-enabled for nesting), then returns so the assembly
//! can write ICCEOIR.

use core::sync::atomic::{AtomicUsize, Ordering};

// ------- Registers, from rza1-pac ------------------------------------------------
// The distributor at 0xE820_1000 and the CPU interface at 0xE820_2000 are `INTC`; so are the
// RZ/A1L's external-interrupt registers ICR1/IRQRR at 0xFCFE_F802/0xFCFE_F804.
use crate::pac::INTC;
use crate::pac::intc::regs::{Iccbpr, Iccicr, Iccpmr, Icddcr, Icr1, Irqrr};

// ---------------------------------------------------------------------------
// Priority field constants
// ---------------------------------------------------------------------------

/// GIC priority field is bits [7:3] of each IPRIORITYR byte; shift a 5-bit
/// priority value left by this amount to position it correctly.
const GIC_PRIORITY_SHIFT: u32 = 3;
/// An IPRIORITYR word holding [`Priority::default`](crate::interrupt::Priority) in every byte
/// lane. [`init`] writes it to every word, so an interrupt nobody gave a priority still reaches
/// the CPU once enabled.
const GIC_PRIORITY_DEFAULT_WORD: u32 =
    ((crate::interrupt::Priority::DEFAULT as u32) << GIC_PRIORITY_SHIFT) * 0x0101_0101;
/// GICC_CTLR (ICCICR) bit 0: EnableS — enables forwarding of Group 0 (secure) interrupts.
/// BSP: INTC_ICCICR_EnableS = 0x01
const ICCICR_ENABLES: u32 = 0x1;
/// GICC_CTLR (ICCICR) bit 1: EnableNS — enables forwarding of Group 1 (non-secure) interrupts.
/// BSP: INTC_ICCICR_EnableNS = 0x02
const ICCICR_ENABLENS: u32 = 0x2;
/// ICCIAR bits [9:0]: ACKINTID — Interrupt Acknowledge ID field.
/// BSP: INTC_ICCIAR_ACKINTID = 0x3FF
const ICCIAR_ACKINTID: u32 = 0x3FF;
/// ICCIAR spurious interrupt ID lower bound (IDs ≥ 0x3FE are spurious).
/// 0x3FE = 1022 (spurious group IRQ), 0x3FF = 1023 (spurious FIQ).
const ICCIAR_SPURIOUS: u32 = 0x3FE;

// ---------------------------------------------------------------------------

/// Total number of interrupt sources on the RZ/A1L: IDs 0–537 (hardware manual Table 7.3).
#[cfg(feature = "rza1l")]
pub const INT_ID_TOTAL: usize = 538;
/// Total number of interrupt sources on the RZ/A1H: IDs 0–586.
#[cfg(feature = "rza1h")]
pub const INT_ID_TOTAL: usize = 587;

/// Interrupt handler type. The handler is called with IRQ re-enabled
/// (nested interrupts supported), already in SYS mode.
pub type HandlerFn = fn();

/// One dispatch-table slot: the handler's address, 0 when empty. Atomic, so a handler can be
/// installed while interrupts run (a word store; `gic_dispatch` reads it with one load).
struct HandlerCell(core::sync::atomic::AtomicUsize);

impl HandlerCell {
    const fn new() -> Self {
        HandlerCell(core::sync::atomic::AtomicUsize::new(0))
    }
    fn set(&self, h: HandlerFn) {
        self.0.store(h as usize, core::sync::atomic::Ordering::Release);
    }
    fn get(&self) -> Option<HandlerFn> {
        match self.0.load(core::sync::atomic::Ordering::Acquire) {
            0 => None,
            // SAFETY: every non-zero value was stored by `set` from a `HandlerFn`.
            addr => Some(unsafe { core::mem::transmute::<usize, HandlerFn>(addr) }),
        }
    }
}

/// CS2 + CS3, the SDRAM-capable external areas (64 MB each): the default SDRAM code window.
const SDRAM_CODE_AREAS: core::ops::Range<usize> = 0x0800_0000..0x1000_0000;

/// The part of [`SDRAM_CODE_AREAS`] that can hold code on this board; see
/// [`set_sdram_code_window`].
static SDRAM_CODE_START: AtomicUsize = AtomicUsize::new(SDRAM_CODE_AREAS.start);
static SDRAM_CODE_END: AtomicUsize = AtomicUsize::new(SDRAM_CODE_AREAS.end);

/// Tell the IRQ dispatcher where this board's SDRAM is, so a corrupted handler slot pointing into
/// an unpopulated part of CS2/CS3 is caught instead of branched to. Optional: by default all of
/// CS2/CS3 counts as code. Call once at boot, before enabling interrupts.
///
/// # Panics
/// If `window` is empty or not inside CS2/CS3 (`0x0800_0000..0x1000_0000`).
pub fn set_sdram_code_window(window: core::ops::Range<usize>) {
    assert!(
        window.start < window.end
            && window.start >= SDRAM_CODE_AREAS.start
            && window.end <= SDRAM_CODE_AREAS.end,
        "gic::set_sdram_code_window: {:#x}..{:#x} is empty or outside CS2/CS3",
        window.start,
        window.end
    );
    SDRAM_CODE_START.store(window.start, Ordering::Relaxed);
    SDRAM_CODE_END.store(window.end, Ordering::Relaxed);
}

/// Could `addr` be the entry point of a function in this image?
///
/// Deliberately coarse — the point is to reject a corrupted slot before branching to it, not to
/// validate a symbol. Code on this part lives in on-chip SRAM (`0x2000_0000`, 3 MB) or in SDRAM on
/// CS2/CS3 (all of `0x0800_0000`–`0x0FFF_FFFF` unless the board narrows it with
/// [`set_sdram_code_window`]), and the range check is what actually catches corruption.
///
/// # Thumb pointers
///
/// A Thumb function pointer carries the interworking bit in bit 0 and its entry point is only
/// 2-byte aligned, so the check strips bit 0 and requires only 2-byte alignment, which both ISAs
/// satisfy. Requiring 4-byte alignment would reject every handler in a Thumb build. Bit 0 is not
/// treated as evidence either way; the range test is what rejects a corrupted slot.
fn is_plausible_code_address(addr: usize) -> bool {
    let sdram = SDRAM_CODE_START.load(Ordering::Relaxed)..SDRAM_CODE_END.load(Ordering::Relaxed);
    is_plausible_in(addr, sdram)
}

/// [`is_plausible_code_address`] against an explicit SDRAM window.
fn is_plausible_in(addr: usize, sdram: core::ops::Range<usize>) -> bool {
    const SRAM: core::ops::Range<usize> = 0x2000_0000..0x2030_0000;
    let target = addr & !1; // drop the Thumb interworking bit
    target % 2 == 0 && (SRAM.contains(&target) || sdram.contains(&target))
}

/// IRQ handler dispatch table — one slot per interrupt ID.
/// Initialized to `None` (unhandled = silently EOI'd).
///
/// # Safety contract
/// Handlers are installed ([`set_handler`]) before global IRQ is enabled.
/// [`gic_dispatch`] is only called from the IRQ handler with IRQ masked,
/// so no data race is possible on single-core.
static HANDLERS: [HandlerCell; INT_ID_TOTAL] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const NONE: HandlerCell = HandlerCell::new();
    [NONE; INT_ID_TOTAL]
};

/// Initial ICDICFR edge/level configuration values (34 registers × 32 bits, the RZ/A1L's).
/// Taken verbatim from the Renesas `intc_icdicfrn_table[]` in `intc.c`, less its last three
/// words (IDs 544–586, which only the RZ/A1H has).
/// Each 2-bit field: bit[1] = 1 → edge-triggered, 0 → level-sensitive (GIC architecture);
/// bit[0] is implementation-defined.
static ICDICFR_INIT: [u32; 34] = [
    0xAAAAAAAA, /* ICDICFR0  :  15 to   0 */
    0x00000055, /* ICDICFR1  :  19 to  16 */
    0xFFFD5555, /* ICDICFR2  :  47 to  32 */
    0x555FFFFF, /* ICDICFR3  :  63 to  48 */
    0x55555555, /* ICDICFR4  :  79 to  64 */
    0x55555555, /* ICDICFR5  :  95 to  80 */
    0x55555555, /* ICDICFR6  : 111 to  96 */
    0x55555555, /* ICDICFR7  : 127 to 112 */
    0x5555F555, /* ICDICFR8  : 143 to 128 */
    0x55555555, /* ICDICFR9  : 159 to 144 */
    0x55555555, /* ICDICFR10 : 175 to 160 */
    0xF5555555, /* ICDICFR11 : 191 to 176 */
    0xF555F555, /* ICDICFR12 : 207 to 192 */
    0x5555F555, /* ICDICFR13 : 223 to 208 */
    0x55555555, /* ICDICFR14 : 239 to 224 */
    0x55555555, /* ICDICFR15 : 255 to 240 */
    0x55555555, /* ICDICFR16 : 271 to 256 */
    0xFD555555, /* ICDICFR17 : 287 to 272 */
    0x55555557, /* ICDICFR18 : 303 to 288 */
    0x55555555, /* ICDICFR19 : 319 to 304 */
    0xFFD55555, /* ICDICFR20 : 335 to 320 */
    0x5F55557F, /* ICDICFR21 : 351 to 336 */
    0xFD55555F, /* ICDICFR22 : 367 to 352 */
    0x55555557, /* ICDICFR23 : 383 to 368 */
    0x55555555, /* ICDICFR24 : 399 to 384 */
    0x55555555, /* ICDICFR25 : 415 to 400 */
    0x55555555, /* ICDICFR26 : 431 to 416 */
    0x55555555, /* ICDICFR27 : 447 to 432 */
    0x55555555, /* ICDICFR28 : 463 to 448 */
    0x55555555, /* ICDICFR29 : 479 to 464 */
    0x55555555, /* ICDICFR30 : 495 to 480 */
    0x55555555, /* ICDICFR31 : 511 to 496 */
    0x55555555, /* ICDICFR32 : 527 to 512 */
    0x55555555, /* ICDICFR33 : 543 to 528 */
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the GIC distributor and CPU interface.
///
/// Mirrors `R_INTC_Init()` from `intc.c`. Must be called once, with global
/// IRQ disabled, before any handler is bound or [`enable`] is called.
///
/// ## Priority filter (PMR)
///
/// This function sets `GICC_PMR = 0xF8` (priority threshold = 31).  The GIC
/// forwards an interrupt to the CPU **only if its programmed priority is
/// strictly less than PMR**.  Because this function sets every interrupt's
/// priority to 31, an interrupt whose priority has *not* been explicitly
/// lowered via [`set_priority`] will **never fire** even after [`enable`] is
/// called.  Always call `set_priority(id, n)` with `n < 31` before `enable`.
///
/// # Safety
/// Writes to memory-mapped GIC registers. Must run exactly once before
/// `cpsie i`.
pub unsafe fn init() {
    unsafe {
        debug!("gic: init ({} IRQ sources)", INT_ID_TOTAL);
        let n_isr = (INT_ID_TOTAL / 32) + 1; // 17 — ICDISR count
        let _n_icfr = ICDICFR_INIT.len(); // 34 — ICDICFR count
        let n_ipr = (INT_ID_TOTAL / 4) + 1; // 135 — ICDIPR count
        let n_iptr = n_ipr; // 135 — ICDIPTR count
        let n_icer = n_isr; // 17  — ICDICER count

        // 0. End what a debugger's reset left active. A probe's halt-only reset restarts the core
        //    but not the GIC, so an interrupt acknowledged before it stays active and holds the CPU
        //    interface's running priority at its level: every interrupt at that priority or below
        //    is then never signalled (the time driver's OSTM among them, so no timer ever fires).
        //    Read before step 3 rewrites the priorities.
        end_stale_active(n_isr);
        for i in 0..n_isr {
            INTC.icdicpr(i).write_value(0xFFFF_FFFF);
        }

        // 1. Mark all interrupts as secure (Group 0), matching the C driver.
        for i in 0..n_isr {
            INTC.icdisr(i).write_value(0);
        }

        // 2. Edge/level configuration — Renesas-specific table from intc.c.
        //    ICDICFR0 (index 0) is read-only per TRM (SGIs are always edge); skip it.
        for (i, &val) in ICDICFR_INIT.iter().enumerate().skip(1) {
            INTC.icdicfr(i).write_value(val);
        }

        // 3. Start every interrupt at the default priority (P16): below PMR (31), so an enabled
        //    interrupt reaches the CPU even if nobody set its priority. Field [7:3] per byte.
        for i in 0..n_ipr {
            INTC.icdipr(i).write_value(GIC_PRIORITY_DEFAULT_WORD);
        }

        // 4. Route all SPI interrupts (IDs 32+) to CPU 0 (target byte = 0x01).
        //    ICDIPTR0–ICDIPTR7 (IDs 0–31) are CPU-local and must not be touched.
        for i in 8..n_iptr {
            INTC.icdiptr(i).write_value(0x0101_0101);
        }

        // 5. Disable all interrupts (ICDICER — write-1-to-clear).
        for i in 0..n_icer {
            INTC.icdicer(i).write_value(0xFFFF_FFFF);
        }

        // 6. CPU interface: priority mask — accept all priorities 0–30.
        //    ICCPMR bit field [7:3] is valid; 31 << 3 = 0xF8.
        INTC.iccpmr()
            .write_value(Iccpmr(31u32 << GIC_PRIORITY_SHIFT));

        // 7. Binary point: group priority [7:3], sub-priority [2:0] unused.
        INTC.iccbpr().write_value(Iccbpr(2));

        // 8. Enable CPU interface — forward both Group 0 (secure) and Group 1
        //    (non-secure) interrupts.
        INTC.iccicr()
            .write_value(Iccicr(ICCICR_ENABLES | ICCICR_ENABLENS));

        // 9. Enable GIC distributor.
        INTC.icddcr().write_value(Icddcr(1));
        debug!("gic: distributor + CPU interface enabled");
    }
}

/// The most urgent active interrupt in `active` (one bit per interrupt ID, 32 IDs a word, as the
/// active bit registers hold them), by `priority` (a lower value is more urgent; the lower ID on a
/// tie), or `None` when none is active. Bits past the last interrupt ID (`INT_ID_TOTAL`) name no
/// interrupt and are ignored: the last active word covers IDs the chip doesn't have.
fn most_urgent_active(active: &[u32], priority: impl Fn(usize) -> u8) -> Option<usize> {
    let mut best: Option<(u8, usize)> = None;
    for (word, &bits) in active.iter().enumerate() {
        let mut rest = bits;
        while rest != 0 {
            let id = word * 32 + rest.trailing_zeros() as usize;
            rest &= rest - 1;
            if id >= INT_ID_TOTAL {
                continue;
            }
            let p = priority(id);
            if best.is_none_or(|(q, _)| p < q) {
                best = Some((p, id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// End every interrupt the GIC still holds active, most urgent first: the reverse of the order a
/// nest of them was acknowledged in, as ICCEOIR expects. GICv1's active bits are read-only, so
/// ICCEOIR is the only way to end one.
///
/// # Safety
/// Writes to memory-mapped GIC registers; for [`init`] only, with IRQs masked.
#[cfg(target_os = "none")]
unsafe fn end_stale_active(n_words: usize) {
    /// Nested acknowledgements are bounded by the 32 priority levels; more passes than that means
    /// an EOI is not taking effect, and the loop stops rather than spin.
    const MAX_ENDS: usize = 32;
    let mut active = [0u32; INT_ID_TOTAL / 32 + 1];
    let words = &mut active[..n_words];
    let mut ended = 0;
    while ended < MAX_ENDS {
        for (i, word) in words.iter_mut().enumerate() {
            // SAFETY: ICDABR0..n_words are readable GIC distributor registers.
            *word = INTC.icdabr(i).read();
        }
        // `most_urgent_active` asks only for IDs below INT_ID_TOTAL, each with a priority byte in
        // ICDIPR0–134. The RZ/A1L gives the priority registers 32-bit access only (manual Table 46.1): read the
        // word and take the ID's byte.
        let priority = |id: usize| (INTC.icdipr(id / 4).read() >> ((id % 4) * 8)) as u8;
        let Some(id) = most_urgent_active(words, priority) else {
            break;
        };
        warn!(
            "gic: interrupt {} was left active by a reset; ending it",
            id
        );
        // SAFETY: ICCEOIR takes the ID of an active interrupt; this core is CPU 0, the source an
        // SGI's EOI names.
        INTC.icceoir()
            .write_value(crate::pac::intc::regs::Icceoir(id as u32));
        ended += 1;
    }
}

/// Host stand-in for [`end_stale_active`]: there is no GIC to read.
#[cfg(not(target_os = "none"))]
unsafe fn end_stale_active(_n_words: usize) {}

/// Install `handler` for interrupt `id`. Out-of-range IDs are ignored.
pub(crate) fn set_handler(id: u16, handler: HandlerFn) {
    trace!("gic: register IRQ {}", id);
    if (id as usize) < INT_ID_TOTAL {
        HANDLERS[id as usize].set(handler);
    }
}

/// The handler installed for `id`, if any.
pub(crate) fn handler(id: u16) -> Option<HandlerFn> {
    HANDLERS.get(id as usize).and_then(HandlerCell::get)
}

/// Enable (unmask) interrupt `id` in the GIC distributor.
///
/// **Requires prior `set_priority` call:** `GICC_PMR` is set to priority 31
/// by [`init`].  An interrupt left at the initial priority of 31 will not
/// reach the CPU even after being enabled here.  Always call
/// [`set_priority`]`(id, n)` with `n < 31` before this function.
///
/// # Safety
/// Writes to memory-mapped peripheral registers.
pub(crate) unsafe fn enable(id: u16) {
    if (id as usize) >= INT_ID_TOTAL {
        return;
    }
    INTC.icdiser((id >> 5) as usize)
        .write_value(1u32 << (id & 31));
}

/// Disable (mask) interrupt `id` in the GIC distributor.
///
/// # Safety
/// Writes to memory-mapped peripheral registers.
pub(crate) unsafe fn disable(id: u16) {
    if (id as usize) >= INT_ID_TOTAL {
        return;
    }
    INTC.icdicer((id >> 5) as usize)
        .write_value(1u32 << (id & 31));
}

/// Shut the GIC down entirely: turn off the distributor and the CPU
/// interface so no interrupt is forwarded to the core.
///
/// Intended for use before handing control to another program that will
/// re-initialise the interrupt controller from scratch (e.g. an application
/// loaded by the second-stage bootloader).
///
/// # Safety
/// Writes the GIC distributor and CPU-interface control registers.
pub unsafe fn shutdown() {
    INTC.iccicr().write_value(Iccicr(0)); // CPU interface off
    INTC.icddcr().write_value(Icddcr(0)); // distributor off
}

/// Set priority for interrupt `id` (0 = highest, 31 = lowest).
///
/// # Safety
/// Writes to memory-mapped peripheral registers.
pub(crate) unsafe fn set_priority(id: u16, priority: u8) {
    if (id as usize) >= INT_ID_TOTAL || priority >= 32 {
        return;
    }
    let ipr = INTC.icdipr((id / 4) as usize);
    let shift = ((id % 4) * 8) as u32;
    let mut val = ipr.read();
    val &= !(0xFFu32 << shift);
    val |= ((priority as u32) << GIC_PRIORITY_SHIFT) << shift; // bits [7:3] are the valid field
    ipr.write_value(val);
}

/// Priority of interrupt `id` (0 = highest, 31 = lowest): the inverse of [`set_priority`].
pub(crate) fn priority(id: u16) -> u8 {
    if (id as usize) >= INT_ID_TOTAL {
        return 31;
    }
    let shift = ((id % 4) * 8) as u32;
    ((INTC.icdipr((id / 4) as usize).read() >> shift) as u8) >> GIC_PRIORITY_SHIFT
}

/// Whether interrupt `id` is enabled in the distributor.
pub(crate) fn is_enabled(id: u16) -> bool {
    (id as usize) < INT_ID_TOTAL
        && INTC.icdiser((id >> 5) as usize).read() & (1u32 << (id & 31)) != 0
}

/// Whether interrupt `id` is pending in the distributor.
pub(crate) fn is_pending(id: u16) -> bool {
    (id as usize) < INT_ID_TOTAL
        && INTC.icdispr((id >> 5) as usize).read() & (1u32 << (id & 31)) != 0
}

/// GICD_SGIR TargetListFilter 0b10: send the SGI to the requesting CPU only.
const SGIR_TARGET_SELF: u32 = 0b10 << 24;

/// Make interrupt `id` pending. A software-generated interrupt (ID 0–15) can only be raised
/// through GICD_SGIR (its ISPENDR bits are read-only on the PL390); every other ID through its
/// set-pending bit.
pub(crate) fn pend(id: u16) {
    if id < 16 {
        INTC.icdsgir()
            .write_value(crate::pac::intc::regs::Icdsgir(SGIR_TARGET_SELF | id as u32));
    } else if (id as usize) < INT_ID_TOTAL {
        INTC.icdispr((id >> 5) as usize)
            .write_value(1u32 << (id & 31));
    }
}

/// Clear interrupt `id`'s pending state. SGIs' clear-pending bits are read-only on the PL390, so
/// for them this does nothing: an SGI stays pending until it is taken.
pub(crate) fn unpend(id: u16) {
    if (16..INT_ID_TOTAL as u16).contains(&id) {
        INTC.icdicpr((id >> 5) as usize)
            .write_value(1u32 << (id & 31));
    }
}

// ---------------------------------------------------------------------------
// RZ/A1L external-interrupt (IRQ0–7) helpers
// ---------------------------------------------------------------------------

/// Configure external interrupt `irq_num` (0–7) for **both-edge** detection.
///
/// Programs ICR1 at `0xFCFEF802`.  Per the TRM the mode bits must be cleared
/// to `0b00` (level-low) before writing the desired `0b11` (both-edges) value.
///
/// # Safety
/// Writes to MMIO.  Call before enabling the interrupt in the GIC.
pub unsafe fn set_irq_both_edges(irq_num: u8) {
    let shift = (irq_num * 2) as u32;
    let mask = 0b11u16 << shift;
    let icr1 = INTC.icr1();
    let cleared = icr1.read().0 & !mask; // clear to level-low (00) first
    icr1.write_value(Icr1(cleared));
    icr1.write_value(Icr1(cleared | (0b11u16 << shift))); // then both-edges (11)
}

/// Configure external interrupt `irq_num` (0–7) for **falling-edge** detection.
///
/// Programs ICR1 at `0xFCFEF802` (mode bits `01`).  Per the TRM the mode bits
/// must be cleared to `0b00` (level-low) before writing the desired value.
///
/// # Safety
/// Writes to MMIO.  Call before enabling the interrupt in the GIC.
pub unsafe fn set_irq_falling_edge(irq_num: u8) {
    let shift = (irq_num * 2) as u32;
    let mask = 0b11u16 << shift;
    let icr1 = INTC.icr1();
    let cleared = icr1.read().0 & !mask;
    icr1.write_value(Icr1(cleared));
    icr1.write_value(Icr1(cleared | (0b01u16 << shift)));
}

/// Clear the pending flag for external interrupt `irq_num` (0–7) in IRQRR.
///
/// Must be called inside the IRQ handler body before returning; otherwise the
/// GIC will re-assert the interrupt immediately after EOI.
///
/// # Safety
/// Writes to MMIO register at `0xFCFEF804`.
pub unsafe fn clear_irq_pending(irq_num: u8) {
    let irqrr = INTC.irqrr();
    irqrr.write_value(Irqrr(irqrr.read().0 & !(1u16 << irq_num)));
}

// ---------------------------------------------------------------------------
// Dispatch — called from assembly IRQ handler
// ---------------------------------------------------------------------------

/// Called from the IRQ handler assembly stub with the raw ICCIAR value.
///
/// Looks up and calls the registered handler for the interrupt. The handler
/// runs with IRQ re-enabled (Cortex-A9 nested interrupt support).
/// The assembly stub is responsible for writing ICCEOIR after this returns.
///
/// # Safety
/// Must only be called from the IRQ handler assembly, with the CPU in
/// SYS mode (as set up by the IRQ handler prologue).
#[cfg(target_os = "none")]
#[cfg_attr(feature = "rt", unsafe(no_mangle))]
pub unsafe extern "C" fn gic_dispatch(icciar: u32) {
    unsafe {
        let int_id = (icciar & ICCIAR_ACKINTID) as u16;

        // IDs 1022 (0x3FE) and 1023 (0x3FF) are spurious — no EOI needed for them
        // but we just return; the assembly will still write ICCEOIR with the value
        // we received, which is harmless for 0x3FE/0x3FF.
        if int_id >= ICCIAR_SPURIOUS as u16 {
            warn!("GIC: spurious IRQ {}", int_id);
            return;
        }

        if (int_id as usize) < INT_ID_TOTAL {
            if let Some(f) = HANDLERS[int_id as usize].get() {
                // Refuse to branch to an address that cannot be code. A corrupted slot
                // would otherwise be a wild `blx` ending in an undefined-instruction abort
                // with only `LR` as a clue; this logs the interrupt whose slot was
                // clobbered and keeps the machine running.
                if !is_plausible_code_address(f as usize) {
                    // Count the other damaged slots too: one bad slot points at a stray
                    // write or a bad index, many at a bulk overrun through the table.
                    let damaged = HANDLERS
                        .iter()
                        .filter(|c| {
                            c.get()
                                .is_some_and(|h| !is_plausible_code_address(h as usize))
                        })
                        .count();
                    error!(
                        "GIC: id={} handler pointer {:#010x} is not a code address - REFUSING to call \
                         it. {} of {} slots damaged. The dispatch table has been corrupted at runtime.",
                        int_id,
                        f as usize,
                        damaged,
                        INT_ID_TOTAL,
                    );
                } else {
                    // Re-enable IRQ to allow higher-priority interrupts to preempt.
                    cortex_ar::interrupt::enable();
                    f();
                    cortex_ar::interrupt::disable();
                }
            } else {
                warn!("GIC: id={} no handler — EOI only", int_id);
            }
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::{
        ICDICFR_INIT, INT_ID_TOTAL, SDRAM_CODE_AREAS, init, is_plausible_code_address,
        is_plausible_in, most_urgent_active, set_irq_both_edges, set_sdram_code_window,
    };

    #[test]
    fn no_active_interrupt_ends_nothing() {
        assert_eq!(most_urgent_active(&[0; 19], |_| 0), None);
    }

    #[test]
    fn the_most_urgent_active_interrupt_ends_first() {
        // OSTM1 (135) active at priority 14, a DMAINT (51) at 13 nested inside it, SGI 8 at 20.
        let mut active = [0u32; 19];
        active[0] = 1 << 8;
        active[1] = 1 << (51 - 32);
        active[4] = 1 << (135 - 128);
        let priority = |id: usize| match id {
            8 => 20 << 3,
            51 => 13 << 3,
            135 => 14 << 3,
            _ => 31 << 3,
        };
        assert_eq!(most_urgent_active(&active, priority), Some(51));
        active[1] = 0;
        assert_eq!(most_urgent_active(&active, priority), Some(135));
        active[4] = 0;
        assert_eq!(most_urgent_active(&active, priority), Some(8));
    }

    #[test]
    fn a_tie_ends_the_lower_id_first() {
        assert_eq!(most_urgent_active(&[0b1010_0000, 1], |_| 0x70), Some(5));
    }

    /// First and last ICDICFR entries from the Renesas intc.c table.
    #[test]
    fn icdicfr_spot_values() {
        assert_eq!(ICDICFR_INIT[0], 0xAAAAAAAA); // ICDICFR0 : IDs 15–0
        assert_eq!(ICDICFR_INIT[1], 0x00000055); // ICDICFR1 : IDs 19–16
        assert_eq!(ICDICFR_INIT[33], 0x55555555); // ICDICFR33: IDs 543–528, the RZ/A1L's last
    }

    #[test]
    fn plausible_code_addresses_cover_sram_and_the_cs2_cs3_sdram_window() {
        // On-chip SRAM, ARM and Thumb entry points.
        assert!(is_plausible_code_address(0x2002_0000));
        assert!(is_plausible_code_address(0x2002_0001));
        assert!(!is_plausible_code_address(0x2030_0000));
        // CS2 and CS3 (SDRAM-capable areas), whatever size the board fits.
        assert!(is_plausible_code_address(0x0800_0000));
        assert!(is_plausible_code_address(0x0C00_0000));
        assert!(is_plausible_code_address(0x0FFF_FFFE));
        assert!(!is_plausible_code_address(0x1000_0000));
        // Null and CS0/CS1.
        assert!(!is_plausible_code_address(0x0000_0000));
        assert!(!is_plausible_code_address(0x07FF_FFFC));
    }

    #[test]
    fn a_board_window_narrows_the_sdram_check() {
        // A board with SDRAM only on CS3: CS2 is unpopulated there, so a slot pointing into it is
        // corruption.
        let cs3_only = 0x0C00_0000..0x1000_0000;
        assert!(is_plausible_in(0x0C00_0000, cs3_only.clone()));
        assert!(is_plausible_in(0x0FFF_FFFE, cs3_only.clone()));
        assert!(!is_plausible_in(0x0800_0000, cs3_only.clone()));
        assert!(!is_plausible_in(0x0BFF_FFFC, cs3_only.clone()));
        // SRAM stays plausible whatever the SDRAM window.
        assert!(is_plausible_in(0x2002_0000, cs3_only));
    }

    #[test]
    fn the_default_window_is_all_of_cs2_and_cs3() {
        assert_eq!(SDRAM_CODE_AREAS, 0x0800_0000..0x1000_0000);
    }

    #[test]
    #[should_panic(expected = "outside CS2/CS3")]
    fn a_window_outside_cs2_cs3_is_rejected() {
        // Validated before anything is stored, so this doesn't disturb the global window.
        set_sdram_code_window(0x2000_0000..0x2030_0000);
    }

    #[test]
    fn the_pac_names_the_same_gic_registers() {
        use crate::pac::INTC;
        assert_eq!(INTC.icddcr().addr(), 0xE820_1000);
        assert_eq!(INTC.icdisr(0).addr(), 0xE820_1080);
        assert_eq!(INTC.icdiser(0).addr(), 0xE820_1100);
        assert_eq!(INTC.icdicer(0).addr(), 0xE820_1180);
        assert_eq!(INTC.icdicpr(0).addr(), 0xE820_1280);
        assert_eq!(INTC.icdabr(0).addr(), 0xE820_1300);
        assert_eq!(
            (INTC.icdipr(0).addr(), INTC.icdipr(0).width()),
            (0xE820_1400, 4)
        );
        assert_eq!(INTC.icdipr(134).addr(), 0xE820_1618);
        assert_eq!(INTC.icdiptr(0).addr(), 0xE820_1800);
        assert_eq!(INTC.icdicfr(0).addr(), 0xE820_1C00);
        assert_eq!(INTC.iccicr().addr(), 0xE820_2000);
        assert_eq!(INTC.iccpmr().addr(), 0xE820_2004);
        assert_eq!(INTC.iccbpr().addr(), 0xE820_2008);
        assert_eq!((INTC.icr1().addr(), INTC.icr1().width()), (0xFCFE_F802, 2));
        assert_eq!(
            (INTC.irqrr().addr(), INTC.irqrr().width()),
            (0xFCFE_F804, 2)
        );
    }

    #[test]
    fn an_active_bit_past_the_last_interrupt_id_is_ignored() {
        // ICDABR16 covers IDs 512–543, but the RZ/A1L stops at 537: bit 28 (ID 540) names no
        // interrupt and has no priority register to look up.
        let mut active = [0u32; 17];
        active[16] = (1 << 28) | (1 << 25); // IDs 540 and 537
        let priority = |id: usize| {
            assert!(id < INT_ID_TOTAL, "looked up the priority of ID {id}");
            0
        };
        assert_eq!(most_urgent_active(&active, priority), Some(537));
    }

    /// rza1-rt's IRQ shim (`src/startup.rs`, `.equ GICC_*_ADDR` and `GICD_IPR0_ADDR`) acknowledges
    /// and ends every interrupt at these addresses; `global_asm!` can't take them from the PAC, so
    /// pin them here.
    #[test]
    fn the_irq_shims_gicc_addresses_are_the_pacs() {
        use crate::pac::INTC;
        assert_eq!(INTC.icciar().addr(), 0xE820_200C);
        assert_eq!(INTC.icceoir().addr(), 0xE820_2010);
        assert_eq!(INTC.icchpir().addr(), 0xE820_2018);
        // `.equ GICD_IPR0_ADDR`: the errata-733075 path's read-modify-write of priority word 0.
        assert_eq!(INTC.icdipr(0).addr(), 0xE820_1400);
    }

    /// The RZ/A1L's distributor: interrupt IDs 0–537 (manual Table 7.3), so 17 words of enable
    /// bits, 135 of priorities and 34 of configuration (Table 46.1).
    #[test]
    fn the_gic_is_sized_for_the_rza1l() {
        assert_eq!(INT_ID_TOTAL, 538);
        assert_eq!(ICDICFR_INIT.len(), 34);
        assert!(std::panic::catch_unwind(|| crate::pac::INTC.icdiser(17)).is_err());
        assert!(std::panic::catch_unwind(|| crate::pac::INTC.icdipr(135)).is_err());
    }

    #[test]
    fn init_masks_everything_then_enables_the_interfaces_last() {
        crate::mmio::test::reset();
        unsafe { init() };
        let w = crate::mmio::test::writes();
        // Every ICDICER word written all-ones (interrupts off), and none past the RZ/A1L's 17.
        for i in 0..17 {
            assert!(
                w.contains(&(0xE820_1180 + 4 * i, 0xFFFF_FFFF)),
                "ICDICER{i}"
            );
        }
        assert!(
            !w.iter().any(|&(a, _)| a == 0xE820_1180 + 4 * 17),
            "no ICDICER17 on the RZ/A1L"
        );
        // SPIs (words 8..) routed to CPU 0; the CPU-local words untouched.
        assert!(w.contains(&(0xE820_1800 + 4 * 8, 0x0101_0101)));
        assert!(
            !w.iter()
                .any(|&(a, _)| (0xE820_1800..0xE820_1820).contains(&a))
        );
        // Priority mask 31, then the CPU interface, then the distributor, last.
        let n = w.len();
        assert_eq!(
            &w[n - 4..],
            [
                (0xE820_2004, 31 << 3),
                (0xE820_2008, 2),
                (0xE820_2000, 3),
                (0xE820_1000, 1)
            ]
        );
    }

    #[test]
    fn an_irq_pin_is_set_to_both_edges_through_level_low() {
        crate::mmio::test::reset();
        crate::mmio::test::poke16(0xFCFE_F802, 0b01 << 6);
        unsafe { set_irq_both_edges(3) };
        assert_eq!(
            crate::mmio::test::writes(),
            [(0xFCFE_F802, 0), (0xFCFE_F802, 0b11 << 6)]
        );
    }
}
