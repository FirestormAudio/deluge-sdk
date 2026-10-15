//! OS Timer (OSTM) driver for the RZ/A1L.
//!
//! The RZ/A1L has two OSTM channels, each a 32-bit timer clocked by P0φ
//! (see [`crate::clocks`]).
//!
//! Two operating modes (OSTMnCTL bits):
//!   - Interval timer (CTL = 0x00): counts down from CMP, fires interrupt at 0
//!   - Free-running  (CTL = 0x02): counts up from 0, optionally fires at CMP
//!
//! The time driver ([`crate::time_driver`]) owns both channels: OSTM0 free-runs as the clock,
//! OSTM1 is the alarm. Code that needs a busy-wait (with interrupts masked, or before the
//! executor runs) uses [`delay_ms`] or [`Delay`] on OSTM0's counter.

use core::sync::atomic::{AtomicBool, Ordering};

// ------- Peripheral base addresses -------------------------------------------
const OSTM0_BASE: usize = 0xFCFE_C000;
const OSTM1_BASE: usize = 0xFCFE_C400;

// ------- Register offsets (from OSTMn base) ----------------------------------
const CMP: usize = 0x00; // OSTMnCMP — compare register (32-bit)
const CNT: usize = 0x04; // OSTMnCNT — counter, read-only (32-bit)
const TE: usize = 0x10; // OSTMnTE  — timer enable status (8-bit, read-only)
const TS: usize = 0x14; // OSTMnTS  — timer start: write 1 to start (8-bit)
const TT: usize = 0x18; // OSTMnTT  — timer stop:  write 1 to stop  (8-bit)
const CTL: usize = 0x20; // OSTMnCTL — control: bit1=mode, bit0=int_enable (8-bit)

// CTL mode values
const CTL_INTERVAL: u8 = 0x00; // count-down interval timer, optional interrupt
const CTL_FREE_RUN: u8 = 0x02; // free-running counter up, optional interrupt
const CTL_FREE_RUN_INT: u8 = 0x03; // free-running with compare interrupt

// ------- CPG standby control (clock gating) ----------------------------------
//
// CPG base = 0xFCFE_0010. STBCR5 is at offset 0x418 from the base:
//   STBCR5 address = 0xFCFE_0010 + 0x418 = 0xFCFE_0428
//
// STBCR5 layout (from stb.c comment):
//   bit 7 : SCIM0  stop (1 = stopped)
//   bit 6 : SCIM1  stop
//   bit 5 : [reserved, keep 1]
//   bit 4 : [reserved, keep 1]
//   bit 3 : [reserved, keep 1]
//   bit 2 : [reserved, keep 1]
//   bit 1 : OSTM1  stop  ← MSTP51
//   bit 0 : OSTM0  stop  ← MSTP50
//
// stb.c initializes STBCR5 = 0b11111100 — bits 0 and 1 clear → OSTM0+OSTM1 clocked.
// A dummy read after write is required by the hardware spec.
const STBCR5: usize = 0xFCFE_0428;
/// STBCR5 bit 0: MSTP50 — stop OSTM0 clock (1 = stopped, 0 = running).
const STBCR5_MSTP50: u8 = 1 << 0;
/// STBCR5 bit 1: MSTP51 — stop OSTM1 clock (1 = stopped, 0 = running).
const STBCR5_MSTP51: u8 = 1 << 1;

/// Whether OSTM0 is counting: the time driver sets it when it starts the channel.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Record whether OSTM0 is counting. Only the time driver starts or stops it.
pub(crate) fn set_running(running: bool) {
    RUNNING.store(running, Ordering::Release);
}

fn running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Channel control (the time driver's)
// ---------------------------------------------------------------------------

/// Ungate OSTM0 and OSTM1 clocks in the CPG module stop register (STBCR5).
///
/// The Renesas `STB_Init()` normally sets `STBCR5 = 0b11111100` which already
/// has bits 0+1 clear (= OSTM0+OSTM1 enabled). This function clears only
/// those two bits so it is safe to call even if the rest of STBCR5 was set
/// by an earlier boot stage.
///
/// # Safety
/// Writes to memory-mapped CPG registers.
pub(crate) unsafe fn enable_clock() {
    unsafe {
        let val = crate::mmio::read8(STBCR5);
        // Clear MSTP50 (OSTM0) and MSTP51 (OSTM1).
        crate::mmio::write8(STBCR5, val & !(STBCR5_MSTP50 | STBCR5_MSTP51));
        let _ = crate::mmio::read8(STBCR5); // mandatory dummy read per HW spec
    }
}

#[inline(always)]
fn base(n: u8) -> usize {
    if n == 0 { OSTM0_BASE } else { OSTM1_BASE }
}

/// Configure OSTM channel `n` (0 or 1) for free-running mode and start it.
///
/// The counter begins at 0 and increments at P0. No interrupt is generated. If the channel is
/// running it is stopped first so CTL can be written.
///
/// # Safety
/// Writes to memory-mapped OSTM registers. Must only be called after [`enable_clock`].
pub(crate) unsafe fn start_free_running(n: u8) {
    unsafe {
        let b = base(n);
        // Stop the timer in case it is already running (CTL can only be written
        // when the timer is stopped — TE must read 0 first).
        crate::mmio::write8(b + TT, 1);
        // Wait for the hardware to confirm the timer has stopped.
        while crate::mmio::read8(b + TE) != 0 {}
        // Free-running mode, compare interrupt disabled.
        crate::mmio::write8(b + CTL, CTL_FREE_RUN);
        crate::mmio::write8(b + TS, 1);
    }
}

/// Read the current counter value of OSTM channel `n` (0 or 1). Wraps at `u32::MAX`.
///
/// # Safety
/// Reads from memory-mapped OSTM registers.
#[inline]
pub(crate) unsafe fn count(n: u8) -> u32 {
    unsafe { crate::mmio::read32(base(n) + CNT) }
}

/// Stop OSTM channel `n`.
///
/// # Safety
/// Writes to memory-mapped OSTM registers.
#[inline]
pub(crate) unsafe fn stop(n: u8) {
    unsafe { crate::mmio::write8(base(n) + TT, 1) }
}

/// Start OSTM channel `n` in free-running mode with a compare-match interrupt.
///
/// The interrupt fires when CNT == `cmp`. After firing, CNT continues counting
/// (one-shot style; the channel stays running). Re-arm by calling this again.
///
/// Use `cmp = 0` to get an overflow notification (fires when CNT wraps to 0).
///
/// # Safety
/// Writes to memory-mapped OSTM registers. Must only be called after [`enable_clock`].
pub(crate) unsafe fn start_free_running_cmp(n: u8, cmp: u32) {
    unsafe {
        let b = base(n);
        crate::mmio::write8(b + TT, 1); // stop first so CTL is writeable
        crate::mmio::write32(b + CMP, cmp);
        crate::mmio::write8(b + CTL, CTL_FREE_RUN_INT); // free-running with CMP interrupt
        crate::mmio::write8(b + TS, 1);
    }
}

/// Start OSTM channel `n` in interval mode with a single-shot countdown.
///
/// The channel counts down from `delta_ticks` to 0, fires one interrupt
/// (at the end of counting — NOT at the start), then auto-reloads.
/// Call [`stop`] inside the ISR to prevent the auto-reload.
///
/// # Safety
/// Writes to memory-mapped OSTM registers. Must only be called after [`enable_clock`].
pub(crate) unsafe fn start_alarm(n: u8, delta_ticks: u32) {
    unsafe {
        let b = base(n);
        crate::mmio::write8(b + TT, 1); // stop in case already running
        crate::mmio::write32(b + CMP, delta_ticks);
        // Interval mode: the interrupt fires at the end of the count (CNT=0), not at the start.
        crate::mmio::write8(b + CTL, CTL_INTERVAL);
        crate::mmio::write8(b + TS, 1);
    }
}

// ---------------------------------------------------------------------------
// Delays
// ---------------------------------------------------------------------------

/// OSTM ticks in `ms` milliseconds at `p0_hz`.
fn ms_to_ticks(p0_hz: u32, ms: u32) -> u32 {
    ms * (p0_hz / 1000)
}

/// OSTM ticks in `ns` nanoseconds at `p0_hz`, rounded up so a delay is never short.
/// Fits u32 for any `ns`: u32::MAX ns ≈ 4.3 s ≈ 143 M ticks.
fn ns_to_ticks(p0_hz: u32, ns: u32) -> u32 {
    (ns as u64 * p0_hz as u64).div_ceil(1_000_000_000) as u32
}

/// Busy-wait for approximately `ticks` OSTM0 counts (~30 ns each at P0).
///
/// # Safety
/// OSTM0 is running.
unsafe fn delay_ticks(ticks: u32) {
    unsafe {
        let t0 = count(0);
        while count(0).wrapping_sub(t0) < ticks {}
    }
}

/// OSTM ticks in `ms` milliseconds, or `None` if no clock tree is registered yet.
fn ms_ticks_if_clocked(clocks: Option<crate::clocks::ClockConfig>, ms: u32) -> Option<u32> {
    clocks.map(|c| ms_to_ticks(c.p0_hz(), ms))
}

/// Busy-wait for `ms` milliseconds on OSTM0's counter. Works with interrupts masked.
///
/// # Panics
/// If OSTM0 isn't running (the time driver starts it), or [`crate::clocks::init`] has not been
/// called.
pub fn delay_ms(ms: u32) {
    assert!(running(), "ostm::delay_ms: OSTM0 isn't running; the time driver starts it");
    // SAFETY: reads OSTM0's counter, which the time driver owns and never writes after start.
    unsafe { delay_ticks(ms_to_ticks(crate::clocks::get().p0_hz(), ms)) }
}

/// Like [`delay_ms`], but returns `false` without waiting if OSTM0 isn't running or no clock tree
/// is registered, instead of panicking. For code that must not panic, such as a panic handler.
pub fn try_delay_ms(ms: u32) -> bool {
    if !running() {
        return false;
    }
    match ms_ticks_if_clocked(crate::clocks::try_get(), ms) {
        Some(ticks) => {
            // SAFETY: as in `delay_ms`.
            unsafe { delay_ticks(ticks) };
            true
        }
        None => false,
    }
}

/// A `DelayNs` on OSTM0's counter, which the time driver runs.
pub struct Delay;

impl Delay {
    /// Create a new `Delay`.
    #[inline]
    pub fn new() -> Self {
        Delay
    }
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl embedded_hal::delay::DelayNs for Delay {
    /// # Panics
    /// As [`delay_ms`].
    fn delay_ns(&mut self, ns: u32) {
        assert!(running(), "ostm::Delay: OSTM0 isn't running; the time driver starts it");
        let ticks = ns_to_ticks(crate::clocks::get().p0_hz(), ns);
        // SAFETY: as in `delay_ms`.
        unsafe { delay_ticks(ticks) };
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::clocks::TEST_CLOCKS;

    #[test]
    fn ostm_base_addresses() {
        // RZ/A1L HW Manual: OSTM0 @ 0xFCFEC000, OSTM1 @ 0xFCFEC400
        assert_eq!(OSTM0_BASE, 0xFCFE_C000);
        assert_eq!(OSTM1_BASE, 0xFCFE_C400);
    }

    #[test]
    fn base_selects_correct_channel() {
        assert_eq!(base(0), OSTM0_BASE);
        assert_eq!(base(1), OSTM1_BASE);
    }

    /// 1 ms at the Deluge's P0φ = 33_064 ticks, as with the old `OSTM_HZ / 1000`.
    #[test]
    fn one_ms_ticks() {
        assert_eq!(ms_to_ticks(TEST_CLOCKS.p0_hz(), 1), 33_064);
        assert_eq!(ms_to_ticks(TEST_CLOCKS.p0_hz(), 100), 3_306_400);
    }

    /// `delay_ns` rounds up, so a short delay is never shorter than asked.
    #[test]
    fn ns_ticks_round_up() {
        let p0 = TEST_CLOCKS.p0_hz();
        assert_eq!(ns_to_ticks(p0, 0), 0);
        assert_eq!(ns_to_ticks(p0, 1), 1);
        // 1 µs = 33.064 ticks → 34.
        assert_eq!(ns_to_ticks(p0, 1_000), 34);
        // u32::MAX ns ≈ 4.29 s still fits u32.
        assert_eq!(ns_to_ticks(p0, u32::MAX), 142_009_065);
    }

    /// Before `clocks::init` there is no delay to compute: `try_delay_ms` must report that
    /// instead of panicking (a panic handler that panics recurses until the stack overflows).
    #[test]
    fn no_delay_ticks_before_clocks_are_registered() {
        assert_eq!(ms_ticks_if_clocked(None, 100), None);
        assert_eq!(ms_ticks_if_clocked(Some(TEST_CLOCKS), 100), Some(3_306_400));
    }

    #[test]
    fn the_time_drivers_ostm0_setup_is_free_running_with_a_wrap_interrupt() {
        crate::mmio::test::reset();
        unsafe { start_free_running_cmp(0, 0) };
        assert_eq!(
            crate::mmio::test::writes(),
            [
                (OSTM0_BASE + TT, 1),
                (OSTM0_BASE + CMP, 0),
                (OSTM0_BASE + CTL, CTL_FREE_RUN_INT as u32),
                (OSTM0_BASE + TS, 1),
            ]
        );
    }

    #[test]
    fn the_alarm_counts_down_once_from_its_delta() {
        crate::mmio::test::reset();
        unsafe { start_alarm(1, 33_064) };
        assert_eq!(
            crate::mmio::test::writes(),
            [
                (OSTM1_BASE + TT, 1),
                (OSTM1_BASE + CMP, 33_064),
                (OSTM1_BASE + CTL, CTL_INTERVAL as u32),
                (OSTM1_BASE + TS, 1),
            ]
        );
    }

    /// Before the time driver starts OSTM0 its counter stands still: a delay would spin forever.
    #[test]
    fn a_delay_before_ostm0_runs_reports_it_instead_of_spinning() {
        assert!(!try_delay_ms(100));
    }

    #[test]
    #[should_panic(expected = "OSTM0 isn't running")]
    fn delay_ms_before_ostm0_runs_panics() {
        delay_ms(1);
    }
}
