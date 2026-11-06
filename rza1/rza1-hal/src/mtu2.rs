//! Multi-Function Timer Pulse Unit 2 (MTU2) driver for the RZ/A1L.
//!
//! The RZ/A1L MTU2 has five 16-bit timer channels (0–4), all clocked by P0φ
//! (see [`crate::clocks::ClockConfig::p0_hz`]). Each is driven in one of two modes:
//! - [`OneShot`]: a one-shot compare-match timer.
//! - [`PulseTrain`]: a free-running period (compare A) with a pulse end (compare B) inside it.
//!
//! ## Register layout peculiarity
//! The MTU2 register block at `0xFCFF_0000` is non-contiguous: channels 2, 3,
//! and 4 share one sub-block (starting at +0x000), while channels 0 and 1
//! occupy separate sub-blocks starting at +0x300 and +0x380.  All addresses
//! are derived from the `st_mtu2` struct in `mtu2_iodefine.h`.
//!
//! ## TSTR (Timer Start Register)
//! One shared TSTR register at `0xFCFF_0280` controls which channels run.
//! Channels 3 and 4 have a write gate, `TRWER`: it is open at reset, and [`OneShot::new`] opens
//! it again in case earlier code closed it. TSTR changes run in a critical section, so they are
//! atomic with respect to other channels' timers, including ones started or stopped from an
//! interrupt handler.
//!
//! ## Usage (one-shot)
//! ```ignore
//! struct GateHandler;
//! impl Handler<typelevel::TGI2A> for GateHandler {
//!     unsafe fn on_interrupt() { /* timer.clear_flag(); act; timer.disarm(); */ }
//! }
//! rza1_hal::bind_interrupts!(struct Irqs { TGI2A => GateHandler; });
//!
//! let timer = mtu2::OneShot::new(p.MTU2_2, 64, Irqs); // P0φ/64
//! timer.arm(520); // fires after 520 ticks ≈ 1 ms
//! ```
//!
//! ## Usage (period and pulse width)
//! ```ignore
//! struct PeriodHandler;
//! impl Handler<typelevel::TGI4A> for PeriodHandler {
//!     unsafe fn on_interrupt() { /* train.clear_a(); the period began: raise the output */ }
//! }
//! struct WidthHandler;
//! impl Handler<typelevel::TGI4B> for WidthHandler {
//!     unsafe fn on_interrupt() { /* train.clear_b(); the pulse ended: lower the output */ }
//! }
//! rza1_hal::bind_interrupts!(struct Irqs { TGI4A => PeriodHandler; TGI4B => WidthHandler; });
//!
//! let train = mtu2::PulseTrain::new(p.MTU2_4, Irqs);
//! train.start(1024, 64_578, 163); // P0φ/1024: a period of 64578 ticks, a pulse of 163
//! // From the compare-A handler, right after a boundary:
//! train.retime(1024, 64_000, 163);
//! ```

// ---------------------------------------------------------------------------
// Registers, from rza1-pac. The SVD names MTU2's per-channel registers by channel
// (`tcr_2`, `tgra_3`, …); these tables index them by channel number.
// ---------------------------------------------------------------------------

use crate::pac::MTU2;
use crate::pac::common::{RW, Reg};
use crate::pac::mtu2::regs::Trwer;

// Per-channel TCR (Timer Control Register) — 8-bit
//   bits [2:0] = TPSC (prescaler select)
//   bits [7:5] = CCLR (counter-clear source; 0b001 = clear on TGRA match)
const TCR: [Reg<u8, RW>; 5] = [
    MTU2.tcr_0(),
    MTU2.tcr_1(),
    MTU2.tcr_2(),
    MTU2.tcr_3(),
    MTU2.tcr_4(),
];

// Per-channel TIER (Timer Interrupt Enable Register) — 8-bit
//   bit 0 = TGIEA: enable compare-match-A interrupt
//   bit 1 = TGIEB: enable compare-match-B interrupt
const TIER: [Reg<u8, RW>; 5] = [
    MTU2.tier_0(),
    MTU2.tier_1(),
    MTU2.tier_2(),
    MTU2.tier_3(),
    MTU2.tier_4(),
];

// Per-channel TSR (Timer Status Register) — 8-bit
//   bit 0 = TGFA: compare-match-A flag (write 0 to clear)
//   bit 1 = TGFB: compare-match-B flag (write 0 to clear)
const TSR: [Reg<u8, RW>; 5] = [
    MTU2.tsr_0(),
    MTU2.tsr_1(),
    MTU2.tsr_2(),
    MTU2.tsr_3(),
    MTU2.tsr_4(),
];

// Per-channel TCNT (Timer Counter) — 16-bit, read/write
const TCNT: [Reg<u16, RW>; 5] = [
    MTU2.tcnt_0(),
    MTU2.tcnt_1(),
    MTU2.tcnt_2(),
    MTU2.tcnt_3(),
    MTU2.tcnt_4(),
];

// Per-channel TGRA (Timer General Register A) — 16-bit compare value
const TGRA: [Reg<u16, RW>; 5] = [
    MTU2.tgra_0(),
    MTU2.tgra_1(),
    MTU2.tgra_2(),
    MTU2.tgra_3(),
    MTU2.tgra_4(),
];

// Per-channel TGRB (Timer General Register B) — 16-bit compare value
const TGRB: [Reg<u16, RW>; 5] = [
    MTU2.tgrb_0(),
    MTU2.tgrb_1(),
    MTU2.tgrb_2(),
    MTU2.tgrb_3(),
    MTU2.tgrb_4(),
];

// Shared registers: MTU2.tstr() (Timer Start) and MTU2.trwer() (Timer Read/Write Enable).

// TSTR bit for each channel (CST0..CST4).
// Channels 0–2 use bits 0–2; channels 3–4 use bits 6–7.
const CST: [u8; 5] = [0x01, 0x02, 0x04, 0x40, 0x80];

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// GIC interrupt IDs for the compare-match-A (TGInA) interrupt on channels 0–4.
/// Source: `devdrv_intc.h`.
pub(crate) const TGIA_IRQ: [u16; 5] = [139, 146, 150, 154, 159];

/// GIC interrupt IDs for the compare-match-B (TGInB) interrupt on channels 0–4.
pub(crate) const TGIB_IRQ: [u16; 5] = [140, 147, 151, 155, 160];

const TGFA: u8 = 0x01;
const TGFB: u8 = 0x02;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Clear channel `ch`'s CST bit. TSTR is shared by all five channels, so the read-modify-write
/// runs in a critical section: an interrupt on another channel cannot lose its own start or stop.
fn stop_counter(ch: usize) {
    critical_section::with(|_| MTU2.tstr().modify(|r| r.0 &= !CST[ch]));
}

/// Set channel `ch`'s CST bit, atomically with respect to the other channels (see
/// [`stop_counter`]).
fn start_counter(ch: usize) {
    critical_section::with(|_| MTU2.tstr().modify(|r| r.0 |= CST[ch]));
}
// ---------------------------------------------------------------------------

/// Encode the TPSC field in TCR for the given channel and prescaler.
///
/// Valid prescalers:
/// - All channels: 1, 4, 16, 64
/// - Ch1, 3, 4 only: 256
/// - Ch2, 3, 4 only: 1024
///
/// Returns `None` for a prescaler the channel lacks.
fn tpsc(ch: u8, prescaler: u16) -> Option<u8> {
    match prescaler {
        1 => Some(0b000),
        4 => Some(0b001),
        16 => Some(0b010),
        64 => Some(0b011),
        256 => match ch {
            1 => Some(0b110),
            3 | 4 => Some(0b100),
            _ => None,
        },
        1024 => match ch {
            2 => Some(0b111),
            3 | 4 => Some(0b101),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Unlock MTU2 register write access.
///
/// The write gate (`TRWER`) guards channels 3 and 4's control registers and
/// counters; channels 0–2 have none. It is open at reset (RZ/A1L hardware manual
/// §10.3.15), but earlier code may have closed it, so call this once during
/// startup before configuring either channel. The MTU2 clock must already be
/// running (STBCR3 bit 3, MSTP33, = 0, §42.2.3: [`ModuleClock::Mtu2`](crate::ModuleClock::Mtu2)
/// in [`Config::module_clocks`](crate::Config::module_clocks)).
///
/// # Safety
/// Writes to the memory-mapped TRWER register.
unsafe fn enable_write() {
    MTU2.trwer().write_value(Trwer(1));
}

/// Stop channel `ch` and configure it for one-shot compare-match use: TCR = `tpsc` with
/// CCLR = TGRA (the counter clears on the match) and TIER = 0. The GIC source stays masked until
/// [`arm`].
///
/// # Safety
/// Writes MTU2 registers; `ch` is owned.
unsafe fn setup_one_shot(ch: u8, tpsc: u8) {
    stop_counter(ch as usize);
    TCR[ch as usize].write_value(tpsc | (1 << 5));
    TIER[ch as usize].write_value(0);
}

/// Arm channel `ch` for a one-shot event `tgra` ticks from now.
///
/// Resets the counter to 0, writes `tgra` as the compare value, clears any
/// stale flag, enables the GIC interrupt, and starts the counter.  The
/// registered handler is called when `TCNT` reaches `tgra`; after that the
/// counter resets to 0 (because TCR.CCLR = TGRA) and would fire again unless
/// [`disarm`] is called from the handler.
///
/// Typically called from audio-render context to schedule a MIDI gate edge.
///
/// # Safety
/// Writes to memory-mapped MTU2 registers and GIC ISENABLER.
unsafe fn arm(ch: u8, tgra: u16) {
    unsafe {
        // Stop while reconfiguring.
        stop_counter(ch as usize);

        // Write compare value then reset counter.
        TGRA[ch as usize].write_value(tgra);
        TCNT[ch as usize].write_value(0);

        // Clear any pending compare-match flag (write 0 to TGFA bit).
        clear_flag(ch);

        // Enable the GIC source so the handler fires.
        crate::gic::enable(TGIA_IRQ[ch as usize]);

        // Enable the channel's own interrupt output.
        TIER[ch as usize].write_value(1); // TGIEA = 1

        // Start.
        start_counter(ch as usize);
    }
}

/// Clear the compare-match-A flag (TGFA) in channel `ch`'s TSR.
///
/// Must be called from the interrupt handler before returning, or the GIC
/// will immediately re-trigger.  Matches the loop logic in the C BSP
/// (`timerClearCompareMatchTGRA`): reads TSR until TGFA is clear.
///
/// # Safety
/// Reads and writes to memory-mapped MTU2 registers.
unsafe fn clear_flag(ch: u8) {
    clear_flags(ch, TGFA);
}

/// Clear the TSR flags in `mask` on channel `ch`, re-reading until they read back clear (bounded
/// to 64 iterations so a continuously re-firing timer cannot trap the CPU here indefinitely).
fn clear_flags(ch: u8, mask: u8) {
    let tsr = TSR[ch as usize];
    for _ in 0..64u8 {
        let val = tsr.read();
        if val & mask == 0 {
            break;
        }
        tsr.write_value(val & !mask);
    }
}

/// Disarm channel `ch`: stop the counter and disable the GIC interrupt.
///
/// Safe to call from within the interrupt handler (after [`clear_flag`]) or
/// from task context after a cancelled shot.
///
/// # Safety
/// Writes to memory-mapped MTU2 registers and GIC ICENABLER.
unsafe fn disarm(ch: u8) {
    unsafe {
        // Disable the channel's interrupt output first.
        TIER[ch as usize].write_value(0);

        // Stop the counter.
        stop_counter(ch as usize);

        // Mask the GIC source.
        crate::gic::disable(TGIA_IRQ[ch as usize]);
    }
}

// ---------------------------------------------------------------------------
// Owned one-shot
// ---------------------------------------------------------------------------

use core::marker::PhantomData;

use crate::Peri;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// An MTU2 channel singleton (`peripherals::MTU2_0`–`MTU2_4`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–4.
    const CH: u8;
    /// Its compare-match-A interrupt, `TGI<CH>A`.
    type Interrupt: Interrupt;
    /// Its compare-match-B interrupt, `TGI<CH>B`.
    type InterruptB: Interrupt;
}

macro_rules! instances {
    ($($t:ident = $n:literal, $irq:ident, $irq_b:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$t {}
        impl Instance for crate::peripherals::$t {
            const CH: u8 = $n;
            type Interrupt = crate::interrupt::typelevel::$irq;
            type InterruptB = crate::interrupt::typelevel::$irq_b;
        }
    )*};
}

instances! {
    MTU2_0 = 0, TGI0A, TGI0B;
    MTU2_1 = 1, TGI1A, TGI1B;
    MTU2_2 = 2, TGI2A, TGI2B;
    MTU2_3 = 3, TGI3A, TGI3B;
    MTU2_4 = 4, TGI4A, TGI4B;
}

/// An MTU2 channel as a one-shot timer: [`arm`](Self::arm) it for a number of ticks and its
/// compare-match handler runs once they have elapsed. The counter clears on the match, so a shot
/// left armed fires again every period, which makes a periodic tick too.
///
/// Its methods take `&self`, so an interrupt handler can reach it through a static.
pub struct OneShot<'d> {
    ch: u8,
    _timer: PhantomData<&'d mut ()>,
}

impl<'d> OneShot<'d> {
    /// Configure channel `T` at P0φ / `prescaler`, with `H` handling its compare match. Stopped
    /// until [`arm`](Self::arm). The compare-match interrupt's priority is the application's
    /// (`T::Interrupt::set_priority`).
    ///
    /// # Panics
    /// If channel `T` has no /`prescaler`: every channel has /1, /4, /16 and /64; channels 1, 3
    /// and 4 have /256; channels 2, 3 and 4 have /1024.
    pub fn new<T: Instance, H: Handler<T::Interrupt>>(
        _timer: Peri<'d, T>,
        prescaler: u16,
        irq: impl Binding<T::Interrupt, H> + 'd,
    ) -> Self {
        let code = tpsc(T::CH, prescaler)
            .unwrap_or_else(|| panic!("MTU2 channel {} has no /{} prescaler", T::CH, prescaler));
        interrupt::bind(irq);
        // SAFETY: channel T is owned. TRWER opens channels 3 and 4's registers and is harmless
        // to write again.
        unsafe {
            enable_write();
            setup_one_shot(T::CH, code);
        }
        Self {
            ch: T::CH,
            _timer: PhantomData,
        }
    }

    /// Fire once `ticks` from now: the counter restarts from 0, a stale match flag is cleared, and
    /// the interrupt is unmasked. Re-arming before it fires replaces the pending shot.
    pub fn arm(&self, ticks: u16) {
        // SAFETY: the owned channel.
        unsafe { arm(self.ch, ticks) }
    }

    /// Stop the counter and mask the interrupt. Safe to call from the handler.
    pub fn disarm(&self) {
        // SAFETY: the owned channel.
        unsafe { disarm(self.ch) }
    }

    /// Clear the compare-match flag. The handler must do this before it returns, or the GIC
    /// raises the interrupt again at once.
    pub fn clear_flag(&self) {
        // SAFETY: the owned channel.
        unsafe { clear_flag(self.ch) }
    }
}

/// The prescalers channel `ch` offers, as [`PulseTrain::start`] takes them: `[1, 4, 16, 64]`, then
/// 256 and 1024 where the channel has them.
pub fn prescalers(ch: u8) -> &'static [u16] {
    match (tpsc(ch, 256).is_some(), tpsc(ch, 1024).is_some()) {
        (true, true) => &[1, 4, 16, 64, 256, 1024],
        (true, false) => &[1, 4, 16, 64, 256],
        (false, true) => &[1, 4, 16, 64, 1024],
        (false, false) => &[1, 4, 16, 64],
    }
}

/// An MTU2 channel as a pulse generator: the counter clears on a compare-A match, which starts
/// each period, and a compare-B match inside the period ends the pulse. Both interrupts run, so
/// the handlers drive the output: raise it on A, lower it on B.
///
/// Its methods take `&self`, so an interrupt handler can reach it through a static.
pub struct PulseTrain<'d> {
    ch: u8,
    _timer: PhantomData<&'d mut ()>,
}

impl<'d> PulseTrain<'d> {
    /// Configure channel `T` with `HA` handling compare A and `HB` compare B. Stopped, both
    /// interrupts bound but masked, until [`start`](Self::start). The interrupts' priorities are
    /// the application's.
    pub fn new<T: Instance, HA: Handler<T::Interrupt>, HB: Handler<T::InterruptB>>(
        _timer: Peri<'d, T>,
        irq: impl Binding<T::Interrupt, HA> + Binding<T::InterruptB, HB> + 'd,
    ) -> Self {
        interrupt::bind_ref::<T::Interrupt, HA>(&irq);
        interrupt::bind_ref::<T::InterruptB, HB>(&irq);
        // SAFETY: channel T is owned. TRWER opens channels 3 and 4's registers and is harmless
        // to write again.
        unsafe {
            enable_write();
            setup_one_shot(T::CH, 0b000);
        }
        Self {
            ch: T::CH,
            _timer: PhantomData,
        }
    }

    /// Run at P0φ / `prescaler` with a period of `period` ticks and a pulse of `width` ticks: the
    /// counter restarts from 0, stale flags are cleared, both interrupts are unmasked and the
    /// counter starts.
    ///
    /// # Panics
    /// If the channel has no /`prescaler` (see [`prescalers`]).
    pub fn start(&self, prescaler: u16, period: u16, width: u16) {
        let ch = self.ch as usize;
        let code = self.code(prescaler);
        stop_counter(ch as usize);
        TCR[ch].write_value(code | (1 << 5));
        TCNT[ch].write_value(0);
        TGRA[ch].write_value(period);
        TGRB[ch].write_value(width);
        clear_flags(self.ch, TGFA | TGFB);
        // SAFETY: the owned channel's own GIC lines.
        unsafe {
            crate::gic::enable(TGIA_IRQ[ch]);
            crate::gic::enable(TGIB_IRQ[ch]);
        }
        TIER[ch].write_value(TGFA | TGFB); // TGIEA | TGIEB
        start_counter(ch as usize);
    }

    /// Change the period and pulse width. Call it from the compare-A handler, right after a
    /// boundary. At the running prescaler only the compares are written; at a new one the
    /// counter stops, takes the new prescaler, restarts from 0 and runs again.
    ///
    /// # Panics
    /// If the channel has no /`prescaler` (see [`prescalers`]).
    pub fn retime(&self, prescaler: u16, period: u16, width: u16) {
        let ch = self.ch as usize;
        let code = self.code(prescaler);
        if TCR[ch].read() & 0b111 == code {
            TGRA[ch].write_value(period);
            TGRB[ch].write_value(width);
        } else {
            stop_counter(ch as usize);
            TCR[ch].write_value(code | (1 << 5));
            TCNT[ch].write_value(0);
            TGRA[ch].write_value(period);
            TGRB[ch].write_value(width);
            start_counter(ch as usize);
        }
    }

    /// Stop the counter, clear both flags and mask both interrupts. Safe to call from a handler.
    pub fn stop(&self) {
        let ch = self.ch as usize;
        stop_counter(ch as usize);
        TIER[ch].write_value(0);
        clear_flags(self.ch, TGFA | TGFB);
        // SAFETY: the owned channel's own GIC lines.
        unsafe {
            crate::gic::disable(TGIA_IRQ[ch]);
            crate::gic::disable(TGIB_IRQ[ch]);
        }
    }

    /// Clear the compare-A flag. The compare-A handler must do this before it returns.
    pub fn clear_a(&self) {
        clear_flags(self.ch, TGFA);
    }

    /// Clear the compare-B flag. The compare-B handler must do this before it returns.
    pub fn clear_b(&self) {
        clear_flags(self.ch, TGFB);
    }

    fn code(&self, prescaler: u16) -> u8 {
        tpsc(self.ch, prescaler)
            .unwrap_or_else(|| panic!("MTU2 channel {} has no /{} prescaler", self.ch, prescaler))
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn tpsc_common_prescalers() {
        // 1/4/16/64 are valid on every channel and encode identically.
        for ch in 0..5u8 {
            assert_eq!(tpsc(ch, 1), Some(0b000));
            assert_eq!(tpsc(ch, 4), Some(0b001));
            assert_eq!(tpsc(ch, 16), Some(0b010));
            assert_eq!(tpsc(ch, 64), Some(0b011));
        }
    }

    #[test]
    fn tpsc_channel_specific_prescalers() {
        // /256: ch1 -> 0b110; ch3,4 -> 0b100; others invalid.
        assert_eq!(tpsc(1, 256), Some(0b110));
        assert_eq!(tpsc(3, 256), Some(0b100));
        assert_eq!(tpsc(4, 256), Some(0b100));
        // /1024: ch2 -> 0b111; ch3,4 -> 0b101.
        assert_eq!(tpsc(2, 1024), Some(0b111));
        assert_eq!(tpsc(3, 1024), Some(0b101));
        assert_eq!(tpsc(4, 1024), Some(0b101));
    }

    #[test]
    fn tpsc_refuses_prescalers_the_channel_lacks() {
        // /256 not available on ch0/ch2; /1024 not on ch0/ch1; bogus values never.
        assert_eq!(tpsc(0, 256), None);
        assert_eq!(tpsc(2, 256), None);
        assert_eq!(tpsc(0, 1024), None);
        assert_eq!(tpsc(1, 1024), None);
        assert_eq!(tpsc(0, 3), None);
    }

    #[test]
    fn channel_start_bits_are_unique() {
        // TSTR CST bits must be one-hot and distinct per channel.
        let mut seen = 0u8;
        for &b in CST.iter() {
            assert_eq!(b.count_ones(), 1, "each CST bit is one-hot");
            assert_eq!(seen & b, 0, "CST bits must not overlap");
            seen |= b;
        }
    }

    #[test]
    fn tgia_irq_table_covers_all_five_channels() {
        assert_eq!(TGIA_IRQ.len(), 5);
    }

    // ── register-effect tests via the MMIO seam ─────────────────────────────
    use crate::mmio;

    use crate::interrupt::{Handler, typelevel};
    use crate::peripherals::{MTU2_0, MTU2_2};

    struct Shot;
    impl Handler<typelevel::TGI2A> for Shot {
        unsafe fn on_interrupt() {}
    }
    struct Shot0;
    impl Handler<typelevel::TGI0A> for Shot0 {
        unsafe fn on_interrupt() {}
    }

    crate::bind_interrupts!(struct Irqs {
        TGI0A => Shot0;
        TGI2A => Shot;
    });

    #[test]
    fn a_one_shot_is_bound_and_stopped_but_not_enabled() {
        mmio::test::reset();
        let _timer = OneShot::new(unsafe { MTU2_2::steal() }, 64, Irqs);
        assert!(crate::gic::handler(150).is_some(), "TGI2A bound");
        assert_eq!(
            mmio::test::writes(),
            [
                (0xFCFF_0284, 1),
                (0xFCFF_0280, 0),                    // channel 2 stopped
                (0xFCFF_0000, 0b011 | (0b001 << 5)), // P0φ/64, counter cleared on a TGRA match
                (0xFCFF_0004, 0),
                // No GIC priority write: the application sets TGI2A's priority.
            ]
        );
    }

    #[test]
    #[should_panic(expected = "MTU2 channel 0 has no /256 prescaler")]
    fn a_prescaler_the_channel_lacks_is_refused() {
        let _ = OneShot::new(unsafe { MTU2_0::steal() }, 256, Irqs);
    }

    #[test]
    fn arming_starts_a_shot_from_zero_and_unmasks_it() {
        let timer = OneShot::new(unsafe { MTU2_2::steal() }, 64, Irqs);
        mmio::test::reset();
        timer.arm(520);
        assert_eq!(
            mmio::test::writes(),
            [
                (0xFCFF_0280, 0),
                (0xFCFF_0008, 520),
                (0xFCFF_0006, 0),
                (0xE820_1110, 1 << 22), // GICD_ISENABLER4, bit 150 - 128
                (0xFCFF_0004, 1),
                (0xFCFF_0280, CST[2] as u32),
            ]
        );
    }

    #[test]
    fn disarming_stops_the_shot_and_masks_it() {
        let timer = OneShot::new(unsafe { MTU2_2::steal() }, 64, Irqs);
        mmio::test::reset();
        mmio::test::poke8(0xFCFF_0280, CST[2] | CST[3]);
        timer.disarm();
        assert_eq!(
            mmio::test::writes(),
            [
                (0xFCFF_0004, 0),
                (0xFCFF_0280, CST[3] as u32), // channel 3 keeps running
                (0xE820_1190, 1 << 22),       // GICD_ICENABLER4, bit 150 - 128
            ]
        );
    }

    // ── PulseTrain (channel 4) ──────────────────────────────────────────────
    use crate::peripherals::MTU2_4;

    use core::sync::atomic::{AtomicBool, Ordering};
    static PERIOD_RAN: AtomicBool = AtomicBool::new(false);
    static WIDTH_RAN: AtomicBool = AtomicBool::new(false);

    struct Period;
    impl Handler<typelevel::TGI4A> for Period {
        unsafe fn on_interrupt() {
            PERIOD_RAN.store(true, Ordering::SeqCst);
        }
    }
    struct Width;
    impl Handler<typelevel::TGI4B> for Width {
        unsafe fn on_interrupt() {
            WIDTH_RAN.store(true, Ordering::SeqCst);
        }
    }

    crate::bind_interrupts!(struct TrainIrqs {
        TGI4A => Period;
        TGI4B => Width;
    });

    const TSTR: usize = 0xFCFF_0280;
    const TCR4: usize = 0xFCFF_0201;
    const TIER4: usize = 0xFCFF_0209;
    const TSR4: usize = 0xFCFF_022D;
    const TCNT4: usize = 0xFCFF_0212;
    const TGRA4: usize = 0xFCFF_021C;
    const TGRB4: usize = 0xFCFF_021E;

    #[test]
    fn a_pulse_train_is_bound_stopped_and_masked() {
        mmio::test::reset();
        let _train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        // The handler table is process-global: run what is bound and check it is this module's.
        PERIOD_RAN.store(false, Ordering::SeqCst);
        WIDTH_RAN.store(false, Ordering::SeqCst);
        crate::gic::handler(159).expect("TGI4A bound")();
        crate::gic::handler(160).expect("TGI4B bound")();
        assert!(PERIOD_RAN.load(Ordering::SeqCst), "TGI4A is Period");
        assert!(WIDTH_RAN.load(Ordering::SeqCst), "TGI4B is Width");
        assert_eq!(
            mmio::test::writes(),
            [
                (0xFCFF_0284, 1),
                (TSTR, 0),
                (TCR4, 0b000 | (0b001 << 5)), // P0φ/1, counter cleared on a TGRA match
                (TIER4, 0),
            ]
        );
    }

    #[test]
    fn starting_programs_both_compares_then_runs() {
        let train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        mmio::test::reset();
        mmio::test::poke8(TSR4, 0b11);
        train.start(1024, 64_578, 163);
        assert_eq!(
            mmio::test::writes(),
            [
                (TSTR, 0),
                (TCR4, 0b101 | (0b001 << 5)),
                (TCNT4, 0),
                (TGRA4, 64_578),
                (TGRB4, 163),
                (TSR4, 0),              // both flags cleared
                (0xE820_1110, 1 << 31), // GICD_ISENABLER4, bit 159 - 128
                (0xE820_1114, 1 << 0),  // GICD_ISENABLER5, bit 160 - 160
                (TIER4, 0b11),
                (TSTR, CST[4] as u32),
            ]
        );
    }

    #[test]
    #[should_panic(expected = "MTU2 channel 4 has no /8 prescaler")]
    fn starting_at_a_prescaler_the_channel_lacks_is_refused() {
        let train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        train.start(8, 100, 10);
    }

    #[test]
    fn retiming_at_the_same_prescaler_writes_only_the_compares() {
        let train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        train.start(1024, 64_578, 163);
        mmio::test::reset();
        mmio::test::poke8(TCR4, 0b101 | (0b001 << 5)); // reset wiped the running configuration
        train.retime(1024, 60_000, 150);
        assert_eq!(mmio::test::writes(), [(TGRA4, 60_000), (TGRB4, 150)]);
    }

    #[test]
    fn retiming_to_a_new_prescaler_restarts_from_zero() {
        let train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        train.start(1024, 64_578, 163);
        mmio::test::reset();
        mmio::test::poke8(TCR4, 0b101 | (0b001 << 5));
        train.retime(256, 50_000, 120);
        assert_eq!(
            mmio::test::writes(),
            [
                (TSTR, 0),
                (TCR4, 0b100 | (0b001 << 5)),
                (TCNT4, 0),
                (TGRA4, 50_000),
                (TGRB4, 120),
                (TSTR, CST[4] as u32),
            ]
        );
    }

    #[test]
    fn stopping_clears_both_flags_and_masks_both() {
        let train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        train.start(1024, 64_578, 163);
        mmio::test::reset();
        mmio::test::poke8(TSR4, 0b11);
        train.stop();
        assert_eq!(
            mmio::test::writes(),
            [
                (TSTR, 0),
                (TIER4, 0),
                (TSR4, 0),
                (0xE820_1190, 1 << 31), // GICD_ICENABLER4, bit 159
                (0xE820_1194, 1 << 0),  // GICD_ICENABLER5, bit 160
            ]
        );
    }

    #[test]
    fn each_compare_flag_clears_on_its_own() {
        let train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        mmio::test::reset();
        mmio::test::poke8(TSR4, 0b11);
        train.clear_a();
        assert_eq!(mmio::test::writes(), [(TSR4, 0b10)]);
        train.clear_b();
        assert_eq!(mmio::test::writes(), [(TSR4, 0b10), (TSR4, 0)]);
    }

    #[test]
    fn other_channels_keep_their_start_bits() {
        let train = PulseTrain::new(unsafe { MTU2_4::steal() }, TrainIrqs);
        let shot = OneShot::new(unsafe { MTU2_2::steal() }, 64, Irqs);
        mmio::test::reset();
        mmio::test::poke8(TSTR, CST[3]);
        train.start(1024, 64_578, 163);
        assert_eq!(mmio::test::peek8(TSTR), CST[3] | CST[4]);
        train.stop();
        assert_eq!(mmio::test::peek8(TSTR), CST[3]);
        shot.arm(520);
        assert_eq!(mmio::test::peek8(TSTR), CST[3] | CST[2]);
        shot.disarm();
        assert_eq!(mmio::test::peek8(TSTR), CST[3]);
    }

    #[test]
    fn channel_prescalers_match_tpsc() {
        for ch in 0..5u8 {
            for &p in prescalers(ch) {
                assert!(tpsc(ch, p).is_some(), "ch {ch} /{p}");
            }
            for p in [1u16, 4, 16, 64, 256, 1024] {
                assert_eq!(
                    prescalers(ch).contains(&p),
                    tpsc(ch, p).is_some(),
                    "ch {ch} /{p}"
                );
            }
        }
        assert_eq!(prescalers(4), [1, 4, 16, 64, 256, 1024]);
    }

    #[test]
    fn the_pac_names_the_tgrb_registers() {
        let tgrb = [
            MTU2.tgrb_0().addr(),
            MTU2.tgrb_1().addr(),
            MTU2.tgrb_2().addr(),
            MTU2.tgrb_3().addr(),
            MTU2.tgrb_4().addr(),
        ];
        assert_eq!(tgrb[4], TGRB4);
        assert_eq!(MTU2.tgrb_4().width(), 2);
    }

    #[test]
    fn the_pac_names_the_same_channel_registers() {
        use crate::pac::MTU2;
        let tcr = [
            MTU2.tcr_0().addr(),
            MTU2.tcr_1().addr(),
            MTU2.tcr_2().addr(),
            MTU2.tcr_3().addr(),
            MTU2.tcr_4().addr(),
        ];
        assert_eq!(
            tcr,
            [
                0xFCFF_0300,
                0xFCFF_0380,
                0xFCFF_0000,
                0xFCFF_0200,
                0xFCFF_0201
            ]
        );
        let tier = [
            MTU2.tier_0().addr(),
            MTU2.tier_1().addr(),
            MTU2.tier_2().addr(),
            MTU2.tier_3().addr(),
            MTU2.tier_4().addr(),
        ];
        assert_eq!(
            tier,
            [
                0xFCFF_0304,
                0xFCFF_0384,
                0xFCFF_0004,
                0xFCFF_0208,
                0xFCFF_0209
            ]
        );
        let tsr = [
            MTU2.tsr_0().addr(),
            MTU2.tsr_1().addr(),
            MTU2.tsr_2().addr(),
            MTU2.tsr_3().addr(),
            MTU2.tsr_4().addr(),
        ];
        assert_eq!(
            tsr,
            [
                0xFCFF_0305,
                0xFCFF_0385,
                0xFCFF_0005,
                0xFCFF_022C,
                0xFCFF_022D
            ]
        );
        let tcnt = [
            MTU2.tcnt_0().addr(),
            MTU2.tcnt_1().addr(),
            MTU2.tcnt_2().addr(),
            MTU2.tcnt_3().addr(),
            MTU2.tcnt_4().addr(),
        ];
        assert_eq!(
            tcnt,
            [
                0xFCFF_0306,
                0xFCFF_0386,
                0xFCFF_0006,
                0xFCFF_0210,
                0xFCFF_0212
            ]
        );
        let tgra = [
            MTU2.tgra_0().addr(),
            MTU2.tgra_1().addr(),
            MTU2.tgra_2().addr(),
            MTU2.tgra_3().addr(),
            MTU2.tgra_4().addr(),
        ];
        assert_eq!(
            tgra,
            [
                0xFCFF_0308,
                0xFCFF_0388,
                0xFCFF_0008,
                0xFCFF_0218,
                0xFCFF_021C
            ]
        );
        assert_eq!((MTU2.tstr().addr(), MTU2.tstr().width()), (0xFCFF_0280, 1));
        assert_eq!(
            (MTU2.trwer().addr(), MTU2.trwer().width()),
            (0xFCFF_0284, 1)
        );
        // Widths: TCR/TIER/TSR are bytes, TCNT/TGRA halfwords.
        assert_eq!(
            (
                MTU2.tcr_3().width(),
                MTU2.tier_3().width(),
                MTU2.tsr_3().width()
            ),
            (1, 1, 1)
        );
        assert_eq!((MTU2.tcnt_3().width(), MTU2.tgra_3().width()), (2, 2));
    }
}
