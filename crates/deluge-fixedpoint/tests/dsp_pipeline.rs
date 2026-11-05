//! Cross-crate DSP-pipeline integration test.
//!
//! Runs a representative audio-block transform through two DSP crates
//! together — `fixedpoint` (quantise, gain) and `armv7-dsp-intrinsics` (the
//! fixed-point multiply backing the gain) — and checks the result against a
//! scalar `f32` reference. Run it under the QEMU ARM runner to exercise the
//! real DSP instructions (SMMUL etc.), not just the portable fallback.

use armv7_dsp_intrinsics::smmul;
use fixedpoint::Q31;

const N: usize = 256;
const CYCLES: usize = 8;
const TWO_PI: f32 = core::f32::consts::PI * 2.0;

/// A sine of `amp` completing `CYCLES` periods over `N` samples.
fn tone(amp: f32) -> [f32; N] {
    let mut sig = [0.0f32; N];
    for (i, s) in sig.iter_mut().enumerate() {
        *s = amp * (TWO_PI * CYCLES as f32 * i as f32 / N as f32).sin();
    }
    sig
}

#[test]
fn fixed_point_gain_matches_scalar_reference() {
    let sig = tone(0.8);

    // Apply a 0.5 gain in Q31 fixed point. `saturating_mul` is backed by the
    // ARMv7 SMMUL intrinsic, so this exercises fixedpoint -> dsp-intrinsics.
    let gain = Q31::from_float(0.5);
    let mut max_err = 0.0f32;
    for &s in &sig {
        let gained = Q31::from_float(s).saturating_mul(gain).to_float();
        max_err = max_err.max((gained - 0.5 * s).abs());
    }

    // Within Q31 quantisation plus the multiply's dropped LSB.
    assert!(
        max_err < 1e-6,
        "gained signal error {max_err} vs scalar reference"
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
    // Quantising to Q31 and back must add only quantisation-floor error.
    let sig = tone(0.5);
    let mut max_err = 0.0f32;
    for &s in &sig {
        max_err = max_err.max((Q31::from_float(s).to_float() - s).abs());
    }
    // Q31 has ~2^-31 resolution; the round-trip error is essentially zero.
    assert!(max_err < 1e-6, "Q31 round-trip error {max_err} too large");
}
