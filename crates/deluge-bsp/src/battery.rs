//! Battery-voltage sense via the RZ/A1L ADC.
//!
//! The battery rides ADC channel 5 (the board's voltage-sense input, P1_13 as AN5): [`init`]
//! muxes the pin once, then each [`start_conversion`] kicks a conversion; [`read_raw`] returns the raw 16-bit
//! ADC result once it has completed. Mirrors the C BSP's battery path; the
//! application converts the raw reading to a voltage/level.

use rza1_hal::Peri;
use rza1_hal::adc;
use rza1_hal::gpio::{Alternate, Flex};
use rza1_hal::peripherals::P1_13;

/// ADC channel wired to the battery voltage divider on this board.
const VOLT_SENSE_CHANNEL: u8 = 5;

/// Hand P1_13 to the ADC as AN5 (alternative 1). The pin stays muxed; no handle is kept.
pub fn init(pin: Peri<'static, P1_13>) {
    Flex::new(pin).set_as_alternate(Alternate::Alt1);
}

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

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// P1_13 is the ADC's AN5 on alternative 1: port mode off (PMC1.13 set) and PFC/PFCE/PFCAE
    /// clear. They start all-ones here, so a mux that left any of them alone would show.
    #[test]
    fn battery_init_muxes_volt_sense_to_an5() {
        use rza1_hal::mmio::test;
        // PMCn, PFCn, PFCEn, PFCAEn for port 1: 0xFCFE_3400/3500/3600/3A00 + 4.
        const PMC1: usize = 0xFCFE_3404;
        const PFC1: usize = 0xFCFE_3504;
        const PFCE1: usize = 0xFCFE_3604;
        const PFCAE1: usize = 0xFCFE_3A04;
        test::reset();
        for r in [PFC1, PFCE1, PFCAE1] {
            test::poke16(r, 0xFFFF);
        }
        init(unsafe { rza1_hal::peripherals::P1_13::steal() });
        assert_ne!(test::peek16(PMC1) & (1 << 13), 0, "P1_13 in alternative mode");
        for r in [PFC1, PFCE1, PFCAE1] {
            assert_eq!(test::peek16(r) & (1 << 13), 0, "alternative 1 at {r:#x}");
        }
    }
}
