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

use crate::mmio;

/// ADC peripheral base (RZ/A1L).
const ADC_BASE: usize = 0xE800_5800;
/// `ADDRA` — first channel result register (offset 0x00); channel `n` is `+2*n`.
const ADDR0: usize = ADC_BASE;
/// `ADCSR` — A/D control/status register (offset 0x60).
const ADCSR: usize = ADC_BASE + 0x60;

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
    unsafe { mmio::write16(ADCSR, ADCSR_ADST | ADCSR_CKS | (channel as u16 & CH_MASK)) };
}

/// Read the result of the conversion on `channel` if it has completed.
///
/// Returns `Some(raw)` (the raw 16-bit `ADDRn` contents) once `ADCSR.ADF` is set,
/// else `None`. Does not clear `ADF`; the next [`start`] does.
///
/// # Safety
/// Reads memory-mapped ADC registers.
pub unsafe fn read(channel: u8) -> Option<u16> {
    if unsafe { mmio::read16(ADCSR) } & ADCSR_ADF == 0 {
        return None;
    }
    Some(unsafe { mmio::read16(ADDR0 + (channel as usize) * 2) })
}
