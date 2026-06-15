//! Cross-crate DSP-pipeline integration test (testing plan §5.5).
//!
//! Runs a representative audio-block transform through all three DSP crates
//! together — `fixedpoint` (quantise), `armv7-dsp-intrinsics` (the fixed-point
//! multiply backing the gain), and `deluge-fft` (analysis) — and asserts the
//! result stays within numerical bounds. Runs in the QEMU ARM bucket so the
//! real DSP instructions (SMMUL etc.) are exercised, not just the portable
//! fallback.

use armv7_dsp_intrinsics::smmul;
use deluge_fft::{Complex, Fft};
use fixedpoint::Q31;

const N: usize = 256;
const LANES: usize = 4;
const BIN: usize = 8;
const TWO_PI: f32 = core::f32::consts::PI * 2.0;

/// FFT a real signal and return the index of the largest-magnitude bin in the
/// lower half (bins above N/2 mirror it for a real input).
fn peak_bin(sig: &[f32; N]) -> usize {
    let mut buf = [Complex::ZERO; N];
    for (c, &s) in buf.iter_mut().zip(sig.iter()) {
        c.re = s;
    }
    Fft::<N, LANES>::process(&mut buf);
    buf[..N / 2]
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.norm_sq().partial_cmp(&b.norm_sq()).unwrap())
        .map(|(i, _)| i)
        .unwrap()
}

/// Magnitude of bin `BIN` after FFT of a real signal.
fn bin_magnitude(sig: &[f32; N]) -> f32 {
    let mut buf = [Complex::ZERO; N];
    for (c, &s) in buf.iter_mut().zip(sig.iter()) {
        c.re = s;
    }
    Fft::<N, LANES>::process(&mut buf);
    buf[BIN].abs()
}

#[test]
fn fixed_point_gain_then_fft_preserves_tone_and_scales_magnitude() {
    // A pure tone at BIN, comfortably below full scale.
    let mut sig = [0.0f32; N];
    for (i, s) in sig.iter_mut().enumerate() {
        *s = 0.8 * (TWO_PI * BIN as f32 * i as f32 / N as f32).sin();
    }

    // Reference spectrum of the un-gained signal.
    assert_eq!(peak_bin(&sig), BIN, "input tone must land in bin {BIN}");
    let mag0 = bin_magnitude(&sig);

    // Apply a 0.5 gain in Q31 fixed point. `saturating_mul` is backed by the
    // ARMv7 SMMUL intrinsic, so this exercises fixedpoint -> dsp-intrinsics.
    let gain = Q31::from_float(0.5);
    let mut gained = [0.0f32; N];
    for (g, &s) in gained.iter_mut().zip(sig.iter()) {
        *g = Q31::from_float(s).saturating_mul(gain).to_float();
    }

    assert_eq!(peak_bin(&gained), BIN, "gain must not move the tone");
    let mag1 = bin_magnitude(&gained);

    // Magnitude should halve (within Q31 quantisation + FFT round-off).
    let ratio = mag1 / mag0;
    assert!(
        (ratio - 0.5).abs() < 0.01,
        "gained magnitude ratio {ratio} should be ~0.5"
    );
}

#[test]
fn fixedpoint_mul_agrees_with_raw_smmul_intrinsic() {
    // The fixed-point Q31 multiply and the raw SMMUL intrinsic must agree:
    // smmul(a,b) = (a*b)>>32 is Q30 for two Q31 operands, so <<1 brings it back
    // to Q31 — matching `Q31::saturating_mul` to within the dropped LSB.
    let gain = Q31::from_float(0.5);
    for k in -100i32..=100 {
        let x = k as f32 / 128.0; // spread across [-0.78, 0.78]
        let xq = Q31::from_float(x);

        let via_fixedpoint = xq.saturating_mul(gain).raw();
        let via_intrinsic = smmul(xq.raw(), gain.raw()) << 1;

        assert!(
            (via_fixedpoint - via_intrinsic).abs() <= 2,
            "x={x}: fixedpoint {via_fixedpoint} vs smmul {via_intrinsic}"
        );
    }
}

#[test]
fn quantize_dequantize_round_trip_keeps_the_tone() {
    // Quantising to Q31 and back must not move the spectral peak and must add
    // only tiny (quantisation-floor) error — ties fixedpoint to the FFT.
    let mut sig = [0.0f32; N];
    for (i, s) in sig.iter_mut().enumerate() {
        *s = 0.5 * (TWO_PI * BIN as f32 * i as f32 / N as f32).sin();
    }
    let mut requantized = [0.0f32; N];
    let mut max_err = 0.0f32;
    for (r, &s) in requantized.iter_mut().zip(sig.iter()) {
        *r = Q31::from_float(s).to_float();
        max_err = max_err.max((*r - s).abs());
    }
    // Q31 has ~2^-31 resolution; the round-trip error is essentially zero.
    assert!(max_err < 1e-6, "Q31 round-trip error {max_err} too large");
    assert_eq!(peak_bin(&requantized), BIN);
}
