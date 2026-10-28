//! Multi-Function Timer Pulse Unit 2 (MTU2) driver for the RZ/A1L.
//!
//! The RZ/A1L MTU2 has five 16-bit timer channels (0–4), all clocked by P0φ
//! (see [`crate::clocks::ClockConfig::p0_hz`]). Each is driven as a one-shot compare-match timer,
//! [`OneShot`].
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
//! it again in case earlier code closed it.
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
const TIER: [Reg<u8, RW>; 5] = [
    MTU2.tier_0(),
    MTU2.tier_1(),
    MTU2.tier_2(),
    MTU2.tier_3(),
    MTU2.tier_4(),
];

// Per-channel TSR (Timer Status Register) — 8-bit
//   bit 0 = TGFA: compare-match-A flag (write 0 to clear)
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

// ---------------------------------------------------------------------------
// Internal helpers
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
/// running (STBCR3 bit 3, MSTP33, = 0, §42.2.3; normally set through
/// [`crate::stb::init`]).
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
    MTU2.tstr().modify(|r| r.0 &= !CST[ch as usize]);
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
        MTU2.tstr().modify(|r| r.0 &= !CST[ch as usize]);

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
        MTU2.tstr().modify(|r| r.0 |= CST[ch as usize]);
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
    let tsr = TSR[ch as usize];
    // Loop until the flag is clear (normally 1–2 reads; bounded to 64 iterations
    // so a continuously re-firing timer cannot trap the CPU here indefinitely).
    for _ in 0..64u8 {
        let val = tsr.read();
        if val & 0x01 == 0 {
            break;
        }
        tsr.write_value(val & !0x01);
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
        MTU2.tstr().modify(|r| r.0 &= !CST[ch as usize]);

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
    /// Its compare-match-A interrupt, TGI<CH>A.
    type Interrupt: Interrupt;
}

macro_rules! instances {
    ($($t:ident = $n:literal, $irq:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$t {}
        impl Instance for crate::peripherals::$t {
            const CH: u8 = $n;
            type Interrupt = crate::interrupt::typelevel::$irq;
        }
    )*};
}

instances! {
    MTU2_0 = 0, TGI0A;
    MTU2_1 = 1, TGI1A;
    MTU2_2 = 2, TGI2A;
    MTU2_3 = 3, TGI3A;
    MTU2_4 = 4, TGI4A;
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
        let code = tpsc(T::CH, prescaler).unwrap_or_else(|| {
            panic!("MTU2 channel {} has no /{} prescaler", T::CH, prescaler)
        });
        interrupt::bind(irq);
        // SAFETY: channel T is owned. TRWER opens channels 3 and 4's registers and is harmless
        // to write again.
        unsafe {
            enable_write();
            setup_one_shot(T::CH, code);
        }
        Self { ch: T::CH, _timer: PhantomData }
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
