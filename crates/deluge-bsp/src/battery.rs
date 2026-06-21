//! Battery-voltage sense via the RZ/A1L ADC.
//!
//! The battery rides ADC channel 5 (the board's voltage-sense input). One-shot:
//! [`start_conversion`] kicks a conversion; [`read_raw`] returns the raw 16-bit
//! ADC result once it has completed. Mirrors the C BSP's battery path; the
//! application converts the raw reading to a voltage/level.

use rza1l_hal::adc;

/// ADC channel wired to the battery voltage divider on this board.
const VOLT_SENSE_CHANNEL: u8 = 5;

/// Kick a single battery-voltage conversion.
pub fn start_conversion() {
    // SAFETY: the ADC module clock is ungated by the board clock init; single
    // conversion on a dedicated analog input.
    unsafe { adc::start(VOLT_SENSE_CHANNEL) };
}

/// The raw 16-bit ADC reading for the battery channel if a conversion has
/// completed since the last [`start_conversion`]; `None` otherwise.
pub fn read_raw() -> Option<u16> {
    // SAFETY: reads the ADC status/result registers.
    unsafe { adc::read(VOLT_SENSE_CHANNEL) }
}
