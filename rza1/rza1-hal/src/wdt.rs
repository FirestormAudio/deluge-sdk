//! The watchdog timer (HW manual §12), in watchdog-timer mode with an internal reset.
//!
//! [`Watchdog::start`] runs the 8-bit counter at P0φ divided by `CKS` (§12.3.2: `0b111` is
//! P0φ/16384, a 125 ms overflow at 33.33 MHz, the longest there is). If [`Watchdog::kick`]
//! doesn't come first, the module sets `WRCSR.WOVF` and, with `RSTE`, resets the LSI internally.
//! That reset leaves `WRCSR` as it was (§12.5.5): `WOVF` stays 1, so after the watchdog's own
//! reset [`take_overflow`] tells the boot it happened, and clears it. It must: while `WOVF` is 1
//! a further overflow resets nothing. A `RES#` pin reset clears it.
//!
//! The registers take keyed 16-bit writes and 8-bit reads (§12.3.4): `WTCNT` with `0x5A` in the
//! upper byte, `WTCSR` with `0xA5`; `WRCSR` with `0xA5_00` to clear `WOVF` and `0x5A` + data to
//! set `RSTE`.

use core::marker::PhantomData;

use crate::Peri;

/// Watchdog timer control/status register (§12.3.2).
fn wtcsr() -> usize {
    crate::pac::WDT.wtcsr().as_ptr() as usize
}
/// Watchdog timer counter (§12.3.1).
fn wtcnt() -> usize {
    crate::pac::WDT.wtcnt().as_ptr() as usize
}
/// Watchdog reset control/status register (§12.3.3).
fn wrcsr() -> usize {
    crate::pac::WDT.wrcsr().as_ptr() as usize
}

/// `WTCSR`: watchdog-timer mode (not interval).
const WT: u8 = 1 << 6;
/// `WTCSR`: the timer runs.
const TME: u8 = 1 << 5;
/// `WTCSR` bits 4:3: reserved, written as 1.
const WTCSR_RESERVED: u8 = 0b11 << 3;
/// `WTCSR`: the clock select field, three bits.
const CKS: u8 = 0b111;
/// `WRCSR`: the counter overflowed in watchdog-timer mode.
const WOVF: u8 = 1 << 7;
/// `WRCSR`: reset the LSI on overflow.
const RSTE: u8 = 1 << 6;

/// The watchdog timer singleton.
pub trait Instance: crate::PeripheralType + 'static {}
impl Instance for crate::peripherals::WDT {}

/// The watchdog timer, stopped until [`Watchdog::start`].
pub struct Watchdog<'d> {
    _wdt: PhantomData<&'d mut ()>,
}

impl<'d> Watchdog<'d> {
    /// Take the watchdog. Changes nothing: a watchdog a previous stage started keeps running.
    pub fn new(_wdt: Peri<'d, impl Instance>) -> Self {
        Self { _wdt: PhantomData }
    }

    /// Run in watchdog-timer mode with an internal reset on overflow, the counter from 0 at
    /// P0φ / `cks` (§12.3.2's `CKS[2:0]`; only the low three bits are used). The mode, clock and
    /// reset enable are set with the timer stopped, then it starts (§12.4.2).
    pub fn start(&mut self, cks: u8) {
        let mode = WT | WTCSR_RESERVED | (cks & CKS);
        // SAFETY: the watchdog's own registers, written as §12.3.4 requires; the module is owned.
        unsafe {
            write_wtcsr(mode);
            crate::mmio::write16(wtcnt(), 0x5A00);
            crate::mmio::write16(wrcsr(), 0x5A00 | RSTE as u16);
            write_wtcsr(mode | TME);
        }
    }

    /// Restart the count from 0.
    pub fn kick(&mut self) {
        // SAFETY: as `start`.
        unsafe { crate::mmio::write16(wtcnt(), 0x5A00) }
    }

    /// Stop the count, keeping the mode and clock.
    pub fn stop(&mut self) {
        // SAFETY: as `start`.
        unsafe {
            let kept = crate::mmio::read8(wtcsr()) & (WT | CKS);
            write_wtcsr(kept | WTCSR_RESERVED);
        }
    }
}

/// Whether the watchdog overflowed (in watchdog-timer mode) since this was last asked, clearing
/// the flag if so. Usable before any [`Watchdog`] exists.
pub fn take_overflow() -> bool {
    // SAFETY: reading WRCSR has no side effect; the clearing write touches only WOVF (§12.3.4).
    unsafe {
        if crate::mmio::read8(wrcsr()) & WOVF == 0 {
            return false;
        }
        crate::mmio::write16(wrcsr(), 0xA500);
    }
    true
}

/// Write `WTCSR`'s low byte (the `0xA5` key in the upper byte).
///
/// # Safety
/// The watchdog's register.
unsafe fn write_wtcsr(v: u8) {
    unsafe { crate::mmio::write16(wtcsr(), 0xA500 | v as u16) }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio::test;
    use crate::peripherals::WDT;

    fn wdt() -> Watchdog<'static> {
        Watchdog::new(unsafe { WDT::steal() })
    }

    // RZ/A1L HW manual §12.3, Table 12.2.
    const WTCSR: usize = 0xFCFE_0000;
    const WTCNT: usize = 0xFCFE_0002;
    const WRCSR: usize = 0xFCFE_0004;

    #[test]
    fn the_addresses_are_the_manuals() {
        assert_eq!((wtcsr(), wtcnt(), wrcsr()), (WTCSR, WTCNT, WRCSR));
    }

    #[test]
    fn take_overflow_clears_a_set_flag() {
        test::reset();
        test::poke8(WRCSR, 0x80 | 0x1F);
        assert!(take_overflow());
        assert_eq!(
            test::writes(),
            [(WRCSR, 0xA500)],
            "Figure 12.3's WOVF clear"
        );
        test::reset();
        test::poke8(WRCSR, 0x1F);
        assert!(!take_overflow());
        assert!(test::writes().is_empty());
    }

    #[test]
    fn start_enables_reset_then_the_timer() {
        let mut w = wdt();
        test::reset();
        w.start(0b111);
        assert_eq!(
            test::writes(),
            [
                // Stopped, watchdog mode, bits 4:3 written as 1, P0φ/16384.
                (WTCSR, 0xA500 | 0b0101_1111),
                (WTCNT, 0x5A00),
                (WRCSR, 0x5A00 | 0x40),
                (WTCSR, 0xA500 | 0b0111_1111),
            ]
        );
    }

    #[test]
    fn kick_writes_the_counter_word() {
        let mut w = wdt();
        test::reset();
        w.kick();
        assert_eq!(test::writes(), [(WTCNT, 0x5A00)]);
    }

    #[test]
    fn stop_clears_tme() {
        let mut w = wdt();
        test::reset();
        test::poke8(WTCSR, 0b0111_1111);
        w.stop();
        assert_eq!(test::writes(), [(WTCSR, 0xA500 | 0b0101_1111)]);
    }

    #[test]
    fn cks_takes_three_bits() {
        let mut w = wdt();
        test::reset();
        w.start(0xFF);
        assert_eq!(test::writes()[0], (WTCSR, 0xA500 | 0b0101_1111));
    }
}
