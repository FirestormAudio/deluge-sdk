//! Verifies that the `const fn from_float_const` (portable scale-and-saturate)
//! agrees with [`FixedPoint::from_float`], which on ARM is the single fused
//! hardware `VCVT.S32.F32` instruction.
//!
//! On the `armv7-unknown-linux-gnueabihf` QEMU runner (cortex-a9 + neon, see
//! `.cargo/config.toml`) `from_float` lowers to the real instruction, so this is
//! a genuine hardware-vs-portable comparison there:
//!
//! ```text
//! cargo test -p fixedpoint --target armv7-unknown-linux-gnueabihf
//! ```
//!
//! On the host triple both paths are portable, so it still guards against
//! scale/saturation regressions. Only `ROUNDED == false` formats are checked:
//! the hardware VCVT always truncates, while `from_float_const` honours
//! `ROUNDED`, so the two intentionally diverge for rounded formats.
use super::*;

/// Representative values: fractions, boundaries, and out-of-range inputs
/// that must saturate, plus a fine sweep across [-2.0, 2.0].
fn sweep() -> impl Iterator<Item = f32> {
    const POINTS: &[f32] = &[
        0.0,
        -0.0,
        1e-9,
        -1e-9,
        0.25,
        -0.25,
        0.5,
        -0.5,
        0.75,
        -0.75,
        0.999_999,
        -0.999_999,
        1.0,
        -1.0,
        2.0,
        -2.0,
        100.0,
        -100.0,
        f32::MAX,
        f32::MIN,
    ];
    POINTS
        .iter()
        .copied()
        .chain((-2000..=2000).map(|i| i as f32 / 1000.0))
}

macro_rules! assert_matches_vcvt {
    ($q:ty) => {{
        for v in sweep() {
            let hardware = <$q>::from_float(v).raw();
            let portable = <$q>::from_float_const(v).raw();
            assert_eq!(
                hardware, portable,
                concat!(
                    stringify!($q),
                    ": from_float({}) = {} (VCVT) but from_float_const = {}"
                ),
                v, hardware, portable
            );
        }
    }};
}

#[test]
fn q16() {
    assert_matches_vcvt!(Q16);
}

#[test]
fn q17() {
    assert_matches_vcvt!(Q17);
}

#[test]
fn q24() {
    assert_matches_vcvt!(Q24);
}

#[test]
fn q31() {
    assert_matches_vcvt!(Q31);
}
