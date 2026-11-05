//! Deluge CV/gate interface initialisation and control.
//!
//! ## Hardware
//! CV outputs use a **MAX5136** quad 16-bit SPI DAC connected to RSPI0.
//! Gate outputs use direct GPIO V-trig lines (high = gate OFF, low = gate ON).
//!
//! ## SPI wiring (RSPI0)
//! | Port.Pin | Mux  | Signal  | Role                             |
//! |----------|------|---------|----------------------------------|
//! | P6.0     | 3    | RSPCLK0 | SCK                              |
//! | P6.2     | 3    | MOSI0   | MOSI                             |
//! | P6.1     | GPIO | SPI_SSL | active-low CS (software-driven)  |
//!
//! ## MAX5136 frames
//! The DAC's command encoding (write-through frames, the linearity-init sequence) lives in the
//! board-agnostic `max5136` crate. RSPI0 runs in 32-bit mode here, so this module sends its
//! padded `*_u32` words.
//!
//! ## Gate GPIO assignments (V-trig: 0 = ON, 1 = OFF)
//! | Channel | Port | Pin |
//! |---------|------|-----|
//! | 0       | 2    | 7   |
//! | 1       | 2    | 8   |
//! | 2       | 2    | 9   |
//! | 3       | 4    | 0   |

use core::cell::RefCell;

use critical_section::Mutex;
use rza1_hal::gpio::{Level, Output};
#[cfg(target_os = "none")]
use rza1_hal::ostm;

use crate::board::CvGatePins;

// ── Constants ─────────────────────────────────────────────────────────────────

/// MAX5136 linearity-init delay in milliseconds (datasheet §8.3).
const LIN_DELAY_MS: u32 = max5136::LINEARITY_SETTLE_MS;

/// Number of hardware CV channels supported.
pub const NUM_CV_CHANNELS: usize = 2;

/// Number of hardware gate channels supported.
pub const NUM_GATE_CHANNELS: usize = 4;

// ── Pin definitions ───────────────────────────────────────────────────────────

/// The DAC's chip select (active low) and the gate outputs (V-trig: low = gate on), once
/// [`init`] has run. Index matches the gate channel number.
struct Lines {
    cs: Output<'static>,
    gates: [Output<'static>; NUM_GATE_CHANNELS],
}

static LINES: Mutex<RefCell<Option<Lines>>> = Mutex::new(RefCell::new(None));

/// Take the board's chip-select and gate pins: the chip select deselected (high), every gate off
/// (high). Each pin's level is set before it becomes an output, so neither the DAC nor a gate
/// sees a pulse.
fn take_lines(pins: CvGatePins) {
    let lines = Lines {
        cs: Output::new(pins.cs, Level::High),
        gates: [
            Output::new(pins.gate0, Level::High),
            Output::new(pins.gate1, Level::High),
            Output::new(pins.gate2, Level::High),
            Output::new(pins.gate3, Level::High),
        ],
    };
    critical_section::with(|cs| LINES.borrow_ref_mut(cs).replace(lines));
}

/// Drive the DAC's chip select: `true` selects it (low). Does nothing before [`init`].
fn select_dac(select: bool) {
    critical_section::with(|cs| {
        if let Some(l) = LINES.borrow_ref_mut(cs).as_mut() {
            l.cs.set_level((!select).into());
        }
    });
}

// ── MAX5136 command helpers ───────────────────────────────────────────────────

/// Build a 32-bit write-to-DAC word for the MAX5136.
///
/// - `ch`: DAC channel (0–3).
/// - `value`: unsigned 16-bit voltage code (0 = 0 V, 65535 = full-scale).
///   Approximately 6552 counts per volt on the Deluge hardware.
///
/// The frame encoding lives in the board-agnostic `max5136` crate. `None` for a channel the
/// Deluge doesn't wire (`ch >= NUM_CV_CHANNELS`): the write is dropped rather than sent to the
/// DAC as a wrong command byte.
#[inline]
fn dac_word(ch: u8, value: u16) -> Option<u32> {
    ((ch as usize) < NUM_CV_CHANNELS).then(|| max5136::write_through_u32(ch, value))
}

// ── Initialise hardware ───────────────────────────────────────────────────────

/// Initialise the CV DAC + gate GPIO lines.
///
/// **Must be called after** [`crate::bus::init`] (RSPI0) and `rza1_hal::init` (which starts the time driver)
/// (which `delay_ms` needs).
///
/// After this function returns:
/// - All gate outputs are de-asserted (gate OFF).
/// - The MAX5136 has been through its linearity initialisation sequence.
/// - CV outputs are at 0 V.
/// - RSPI0 is ready to accept transfers via [`cv_set`].
///
#[cfg(target_os = "none")]
pub fn init(pins: CvGatePins) {
    {
        log::debug!("cv_gate: init");
        // RSPI0's SCK and MOSI pins are the bus's (`bus::init` muxes them); CS is cv_gate's.
        take_lines(pins);
        log::debug!("cv_gate: gate GPIOs ok");

        // ---- RSPI0 is the bus's: hold it for the whole linearity sequence ------
        let mut bus = crate::bus::try_lock_rspi0().expect(
            "cv_gate::init found RSPI0 busy: bring CV/gate up before the OLED starts drawing",
        );
        bus.enter_32bit();

        // ---- MAX5136 linearity initialisation -----------------------------------
        // Step 1: LIN=1 (linear ramp mode)
        log::debug!("cv_gate: MAX5136 LIN=1");
        select_dac(true);
        bus.send32_blocking(max5136::LINEARITY_ON_U32);
        select_dac(false);

        // Step 2: wait ≥ 10 ms for internal capacitor to charge
        ostm::delay_ms(LIN_DELAY_MS);

        // Step 3: LIN=0 (return to normal mode)
        select_dac(true);
        bus.send32_blocking(max5136::LINEARITY_OFF_U32);
        select_dac(false);
        log::debug!("cv_gate: MAX5136 linearity init done");

        // ---- Zero all CV outputs ------------------------------------------------
        for ch in 0..NUM_CV_CHANNELS as u8 {
            cv_set_blocking(&mut bus, ch, 0);
        }
        log::debug!("cv_gate: CV outputs zeroed");
    }
}

/// Host/QEMU stand-in for [`init`]: no RSPI0/OSTM hardware to bring up (and no
/// `ostm::delay_ms`, which does raw MMIO and would crash off-target), so this takes the pins and
/// logs.
#[cfg(not(target_os = "none"))]
pub fn init(pins: CvGatePins) {
    take_lines(pins);
    log::info!("cv_gate(host): init (no DAC)");
}

// ── CV output ────────────────────────────────────────────────────────────────

/// Write a voltage code to CV channel `ch`, acquiring exclusive RSPI0 access.
///
/// This is the runtime CV-write path. It locks the shared RSPI0 bus (awaiting
/// any in-progress OLED frame), ensures 32-bit frame mode, then clocks out the
/// DAC word.
///
/// - `ch`: 0 or 1 (hardware supports 0–1 on the Deluge). Other channels are ignored.
/// - `value`: 16-bit DAC code.  ~6552 counts ≈ 1 V.
#[cfg(target_os = "none")]
pub async fn cv_set(ch: u8, value: u16) {
    let Some(word) = dac_word(ch, value) else {
        log::warn!("cv_gate: ignoring write to unwired CV channel {ch}");
        return;
    };
    let mut bus = crate::bus::lock_rspi0().await;
    bus.enter_32bit();
    // We hold the RSPI0 bus guard, so no other transfer runs while the DAC is selected.
    select_dac(true);
    bus.send32_blocking(word);
    select_dac(false);
}

/// Write a voltage code to CV channel `ch` on a bus the caller already holds (in 32-bit mode).
#[cfg(target_os = "none")]
fn cv_set_blocking(bus: &mut crate::bus::Rspi0, ch: u8, value: u16) {
    let Some(word) = dac_word(ch, value) else {
        log::warn!("cv_gate: ignoring write to unwired CV channel {ch}");
        return;
    };
    select_dac(true);
    bus.send32_blocking(word);
    select_dac(false);
}

// ── Gate output ───────────────────────────────────────────────────────────────

/// Assert or de-assert gate output `ch`.
///
/// - `ch`: 0–3.
/// - `on`: `true` = gate active (low GPIO), `false` = gate inactive (high GPIO).
///
/// Does nothing before [`init`].
///
/// # Panics
/// If `ch` is 4 or more.
#[inline]
pub fn gate_set(ch: u8, on: bool) {
    critical_section::with(|cs| {
        if let Some(l) = LINES.borrow_ref_mut(cs).as_mut() {
            // V-trig: 0 = active, 1 = inactive → invert `on`.
            l.gates[ch as usize].set_level((!on).into());
        }
    });
}

// ── Address / logic tests (host only) ────────────────────────────────────────

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn dac_word_channel_0_zero_volts() {
        // ch=0, value=0 → (0x30 | 0x01) << 24 | 0 = 0x3100_0000
        assert_eq!(dac_word(0, 0), Some(0x3100_0000));
    }

    #[test]
    fn dac_word_channel_1_full_scale() {
        // ch=1, value=0xFFFF → (0x30 | 0x02) << 24 | (0xFFFF << 8)
        // = 0x32_00_00_00 | 0x00_FF_FF_00
        // = 0x32FF_FF00
        assert_eq!(dac_word(1, 0xFFFF), Some(0x32FF_FF00));
    }

    #[test]
    fn dac_word_channel_1_one_volt() {
        // ~1 V ≈ 6552 = 0x1998
        // ch=1 → 0x30 | 0x02 = 0x32
        // word = 0x32_19_98_00
        let word = dac_word(1, 6552).unwrap();
        assert_eq!(word >> 24, 0x32);
        assert_eq!((word >> 8) & 0xFFFF, 6552);
        assert_eq!(word & 0xFF, 0);
    }

    /// The host stand-in `init` takes the pins without the DAC bring-up (the device version calls
    /// `ostm::delay_ms`, raw MMIO that would crash here); each gate then drives its own pin,
    /// active low, through its port's PSR.
    #[test]
    fn each_gate_drives_its_pin_active_low() {
        use rza1_hal::mmio::test;
        use rza1_hal::peripherals::{P2_7, P2_8, P2_9, P4_0, P6_1};
        let pins = unsafe {
            CvGatePins {
                cs: P6_1::steal(),
                gate0: P2_7::steal(),
                gate1: P2_8::steal(),
                gate2: P2_9::steal(),
                gate3: P4_0::steal(),
            }
        };
        init(pins);
        // PSRn is 0xFCFE_3100 + 4n; the upper half selects the bit, the lower half is its level.
        for (ch, psr, bit) in [
            (0u8, 0xFCFE_3108, 7),
            (1, 0xFCFE_3108, 8),
            (2, 0xFCFE_3108, 9),
            (3, 0xFCFE_3110, 0),
        ] {
            test::reset();
            gate_set(ch, true);
            gate_set(ch, false);
            assert_eq!(
                test::writes(),
                [
                    (psr, 1 << (bit + 16)),
                    (psr, (1 << (bit + 16)) | (1 << bit))
                ],
                "gate {ch}: on = low, off = high"
            );
        }
    }

    #[test]
    fn dac_word_encodes_the_wired_channels() {
        assert_eq!(super::dac_word(0, 0xABCD), Some(0x31AB_CD00));
        assert_eq!(super::dac_word(1, 0x1234), Some(0x3212_3400));
    }

    #[test]
    fn dac_word_refuses_channels_the_board_does_not_wire() {
        // Only CV 0-1 are wired. A bad channel from an app must be dropped, not reach the
        // MAX5136 as a wrong command byte (ch 4-31) or panic in the frame encoder.
        for ch in [2u8, 3, 4, 31, 32, 255] {
            assert_eq!(super::dac_word(ch, 0x8000), None, "ch {ch}");
        }
    }
}
