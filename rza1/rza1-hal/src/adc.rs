//! RZ/A1L 12-bit A/D converter — minimal single-scan polled driver.
//!
//! Used for low-rate analog reads such as battery-voltage sense. One-shot:
//! [`start`] kicks a single conversion on a channel; [`read`] returns the result
//! once the end flag (`ADCSR.ADF`, bit 15) is set. Mirrors the original C BSP's
//! battery path (write `ADCSR` to start, poll bit 15, read `ADDRn`).
//!
//! Preconditions: the ADC module clock must be ungated (CPG `STBCR`, done by the
//! board clock init). The `AN0..AN7` inputs are dedicated analog pins on this
//! family, so no GPIO pin-mux is required.

use crate::pac::ADC;

/// `ADCSR.ADST` — start a single conversion (bit 13).
const ADCSR_ADST: u16 = 1 << 13;
/// `ADCSR.ADF` — conversion-complete flag (bit 15).
const ADCSR_ADF: u16 = 1 << 15;
/// `ADCSR.CKS` clock/conversion-time select used by the C BSP: `0b011` in bits 7:6.
const ADCSR_CKS: u16 = 0b011 << 6;
/// Channel-select field mask (single mode, AN0..AN7 → bits 2:0).
const CH_MASK: u16 = 0x7;

/// Start a single conversion on `channel` (0..=7).
///
/// Writing `ADCSR` also clears any prior `ADF`, so the start/poll/read cycle
/// needs no explicit flag clear.
///
/// # Safety
/// Writes a memory-mapped ADC register; the ADC module clock must be ungated.
pub unsafe fn start(channel: u8) {
    ADC.csr()
        .write_value(ADCSR_ADST | ADCSR_CKS | (channel as u16 & CH_MASK));
}

/// Read the result of the conversion on `channel` if it has completed.
///
/// Returns `Some(raw)` (the raw 16-bit `ADDRn` contents) once `ADCSR.ADF` is set,
/// else `None`. Does not clear `ADF`; the next [`start`] does.
///
/// # Safety
/// Reads memory-mapped ADC registers.
pub unsafe fn read(channel: u8) -> Option<u16> {
    if ADC.csr().read() & ADCSR_ADF == 0 {
        return None;
    }
    Some(ADC.dr(channel as usize).read())
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    #[test]
    fn the_pac_names_the_same_converter_registers() {
        use crate::pac::ADC;
        for ch in 0..8 {
            assert_eq!(
                (ADC.dr(ch).addr(), ADC.dr(ch).width()),
                (0xE800_5800 + ch * 2, 2)
            );
        }
        assert_eq!((ADC.csr().addr(), ADC.csr().width()), (0xE800_5860, 2));
    }
}
