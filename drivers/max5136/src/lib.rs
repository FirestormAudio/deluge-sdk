//! Platform-agnostic driver for the Maxim **MAX5136** quad 16-bit SPI DAC.
//!
//! Every command is a 24-bit frame, sent MSB first: a command byte, then 16 data bits. The
//! frame helpers ([`write_through`], [`LINEARITY_ON`], …) are usable without the driver, for
//! boards that move frames over their own transport. The `*_u32` forms pad the frame into a
//! 32-bit word (`frame << 8`) for SPI controllers run in 32-bit mode; the part ignores the
//! trailing byte.
//!
//! Power-up needs a linearity calibration: send [`LINEARITY_ON`], wait at least
//! [`LINEARITY_SETTLE_MS`], then send [`LINEARITY_OFF`]. [`Max5136::init`] does exactly that.

#![no_std]

use embedded_hal::delay::DelayNs;
use embedded_hal::spi::SpiDevice;

/// DAC channels on the part.
pub const NUM_CHANNELS: u8 = 4;

/// Linearity calibration on: `0x05` command, data `0x4200`.
pub const LINEARITY_ON: [u8; 3] = [0x05, 0x42, 0x00];
/// Linearity calibration off: `0x05` command, data `0x4000`.
pub const LINEARITY_OFF: [u8; 3] = [0x05, 0x40, 0x00];
/// [`LINEARITY_ON`] as a padded 32-bit word.
pub const LINEARITY_ON_U32: u32 = 0x0542_0000;
/// [`LINEARITY_OFF`] as a padded 32-bit word.
pub const LINEARITY_OFF_U32: u32 = 0x0540_0000;
/// Minimum wait between [`LINEARITY_ON`] and [`LINEARITY_OFF`].
pub const LINEARITY_SETTLE_MS: u32 = 10;

/// Write-through frame: load `value` into channel `ch`'s input and DAC registers at once
/// (command `0x30 | 1 << ch`).
///
/// # Panics
/// If `ch >= NUM_CHANNELS` (at compile time, in a `const`).
pub const fn write_through(ch: u8, value: u16) -> [u8; 3] {
    assert!(ch < NUM_CHANNELS, "max5136: channel must be 0-3");
    let [hi, lo] = value.to_be_bytes();
    [0x30 | (1 << ch), hi, lo]
}

/// [`write_through`] as a padded 32-bit word.
pub const fn write_through_u32(ch: u8, value: u16) -> u32 {
    let [cmd, hi, lo] = write_through(ch, value);
    u32::from_be_bytes([cmd, hi, lo, 0])
}

/// A MAX5136 on an [`SpiDevice`] (which owns the chip select).
pub struct Max5136<SPI> {
    spi: SPI,
}

impl<SPI: SpiDevice<u8>> Max5136<SPI> {
    /// Wrap `spi`. Call [`Max5136::init`] before the first [`Max5136::set`].
    pub fn new(spi: SPI) -> Self {
        Self { spi }
    }

    /// Run the power-up linearity calibration.
    pub fn init(&mut self, delay: &mut impl DelayNs) -> Result<(), SPI::Error> {
        self.spi.write(&LINEARITY_ON)?;
        delay.delay_ms(LINEARITY_SETTLE_MS);
        self.spi.write(&LINEARITY_OFF)
    }

    /// Set channel `ch` to `value` (0 = zero scale, 65535 = full scale).
    ///
    /// # Panics
    /// If `ch >= NUM_CHANNELS`.
    pub fn set(&mut self, ch: u8, value: u16) -> Result<(), SPI::Error> {
        self.spi.write(&write_through(ch, value))
    }

    /// Give the SPI device back.
    pub fn release(self) -> SPI {
        self.spi
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::convert::Infallible;
    use embedded_hal::spi::{ErrorType, Operation};
    use std::vec::Vec;

    /// Records every write, one entry per transaction (one chip-select assertion).
    #[derive(Default)]
    struct FakeSpi {
        frames: Vec<Vec<u8>>,
    }
    impl ErrorType for FakeSpi {
        type Error = Infallible;
    }
    impl SpiDevice<u8> for FakeSpi {
        fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), Infallible> {
            let mut frame = Vec::new();
            for op in ops.iter() {
                if let Operation::Write(bytes) = op {
                    frame.extend_from_slice(bytes);
                }
            }
            self.frames.push(frame);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeDelay {
        total_ns: u64,
    }
    impl DelayNs for FakeDelay {
        fn delay_ns(&mut self, ns: u32) {
            self.total_ns += ns as u64;
        }
    }

    #[test]
    fn write_through_frames_match_the_deluge_words() {
        // The words `deluge_bsp::cv_gate` sent before this crate existed.
        assert_eq!(write_through(0, 0xABCD), [0x31, 0xAB, 0xCD]);
        assert_eq!(write_through_u32(0, 0xABCD), 0x31AB_CD00);
        assert_eq!(write_through_u32(1, 0x1234), 0x3212_3400);
        assert_eq!(write_through_u32(3, 0), 0x3800_0000);
    }

    #[test]
    fn linearity_words_match_their_frames() {
        let as_u32 = |f: [u8; 3]| u32::from_be_bytes([f[0], f[1], f[2], 0]);
        assert_eq!(as_u32(LINEARITY_ON), LINEARITY_ON_U32);
        assert_eq!(as_u32(LINEARITY_OFF), LINEARITY_OFF_U32);
    }

    #[test]
    #[should_panic(expected = "channel must be 0-3")]
    fn channel_four_is_rejected() {
        let _ = write_through(4, 0);
    }

    #[test]
    fn init_sends_on_waits_then_off() {
        let mut dac = Max5136::new(FakeSpi::default());
        let mut delay = FakeDelay::default();
        dac.init(&mut delay).unwrap();
        let spi = dac.release();
        assert_eq!(spi.frames, [LINEARITY_ON.to_vec(), LINEARITY_OFF.to_vec()]);
        assert!(delay.total_ns >= LINEARITY_SETTLE_MS as u64 * 1_000_000);
    }

    #[test]
    fn set_sends_one_write_through_frame() {
        let mut dac = Max5136::new(FakeSpi::default());
        dac.set(2, 0x8000).unwrap();
        assert_eq!(dac.release().frames, [std::vec![0x34, 0x80, 0x00]]);
    }
}
