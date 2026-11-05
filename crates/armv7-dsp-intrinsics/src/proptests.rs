//! Property tests: every op must match an independent wide-integer reference.
//!
//! These run in all three build configurations the crate ships
//! (wired up in `tools/test.sh`):
//!   * host x86_64        — portable fallback
//!   * QEMU ARM           — raw `asm!` path (the path firmware ships)
//!   * QEMU ARM + nightly — `core::arch::arm` intrinsics
//!
//! The references below are deliberately *not* the crate's own fallback
//! expressions — they are recomputed from the documented semantics in plain
//! `i64`/`u64` math, so a divergence in any single path (a wrong asm operand, a
//! copy-paste bug in the fallback, an intrinsic mismatch) fails here on the
//! target where that path is compiled.

use super::*;
use proptest::prelude::*;

fn clamp_i32(x: i64) -> i32 {
    x.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

fn ref_qadd(a: i32, b: i32) -> i32 {
    clamp_i32(a as i64 + b as i64)
}
fn ref_qsub(a: i32, b: i32) -> i32 {
    clamp_i32(a as i64 - b as i64)
}
/// Saturating double, i.e. `qadd(b, b)`.
fn ref_double(b: i32) -> i32 {
    clamp_i32(2 * b as i64)
}
/// SMMUL: high 32 bits of the 64-bit signed product.
fn ref_smmul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 32) as i32
}
/// SMMULR: round-to-nearest variant (add 2^31 before taking the high word).
fn ref_smmulr(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64 + 0x8000_0000) >> 32) as i32
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn qadd_matches_ref(a: i32, b: i32) {
        prop_assert_eq!(qadd(a, b), ref_qadd(a, b));
        prop_assert_eq!(saturating_add(a, b), ref_qadd(a, b));
    }

    #[test]
    fn qsub_matches_ref(a: i32, b: i32) {
        prop_assert_eq!(qsub(a, b), ref_qsub(a, b));
        prop_assert_eq!(saturating_sub(a, b), ref_qsub(a, b));
    }

    #[test]
    fn qdadd_matches_ref(a: i32, b: i32) {
        let expected = ref_qadd(a, ref_double(b));
        prop_assert_eq!(qdadd(a, b), expected);
        prop_assert_eq!(saturating_double_add(a, b), expected);
    }

    #[test]
    fn qdsub_matches_ref(a: i32, b: i32) {
        let expected = ref_qsub(a, ref_double(b));
        prop_assert_eq!(qdsub(a, b), expected);
        prop_assert_eq!(saturating_double_sub(a, b), expected);
    }

    #[test]
    fn smmul_matches_ref(a: i32, b: i32) {
        prop_assert_eq!(smmul(a, b), ref_smmul(a, b));
        prop_assert_eq!(mul_high(a, b), ref_smmul(a, b));
    }

    #[test]
    fn smmulr_matches_ref(a: i32, b: i32) {
        prop_assert_eq!(smmulr(a, b), ref_smmulr(a, b));
        prop_assert_eq!(mul_high_round(a, b), ref_smmulr(a, b));
    }

    #[test]
    fn smmla_matches_ref(acc: i32, a: i32, b: i32) {
        let expected = acc.wrapping_add(ref_smmul(a, b));
        prop_assert_eq!(smmla(acc, a, b), expected);
        prop_assert_eq!(mul_accumulate_high(acc, a, b), expected);
    }

    #[test]
    fn smmlar_matches_ref(acc: i32, a: i32, b: i32) {
        let expected = acc.wrapping_add(ref_smmulr(a, b));
        prop_assert_eq!(smmlar(acc, a, b), expected);
        prop_assert_eq!(mul_accumulate_high_round(acc, a, b), expected);
    }

    #[test]
    fn smmlsr_matches_ref(acc: i32, a: i32, b: i32) {
        let expected = acc.wrapping_sub(ref_smmulr(a, b));
        prop_assert_eq!(smmlsr(acc, a, b), expected);
        prop_assert_eq!(mul_subtract_high_round(acc, a, b), expected);
    }

    // Saturation ops are const-generic over the bit width, so a representative
    // set of widths is instantiated and the input value is fuzzed across each.
    #[test]
    fn ssat_matches_ref(v: i32) {
        macro_rules! check {
            ($bits:expr) => {{
                let min = -(1i32 << ($bits - 1));
                let max = (1i32 << ($bits - 1)) - 1;
                prop_assert_eq!(ssat::<$bits>(v), v.clamp(min, max));
                prop_assert_eq!(saturate_signed::<$bits>(v), v.clamp(min, max));
            }};
        }
        check!(8);
        check!(12);
        check!(16);
        check!(24);
        check!(31);
    }

    #[test]
    fn usat_matches_ref(v: i32) {
        macro_rules! check {
            ($bits:expr) => {{
                let max = ((1u64 << $bits) - 1) as i32;
                prop_assert_eq!(usat::<$bits>(v), v.clamp(0, max) as u32);
                prop_assert_eq!(saturate_unsigned::<$bits>(v), v.clamp(0, max) as u32);
            }};
        }
        check!(8);
        check!(12);
        check!(16);
        check!(24);
        check!(31);
    }

    // The shift is a 32-bit barrel-shift (bits past bit 31 are lost) and only
    // then is the value saturated — the reference recomputes that in i32, so the
    // full input range (including values whose shift overflows 32 bits) is
    // exercised.
    #[test]
    fn ssat_lsl_matches_ref(v: i32) {
        macro_rules! check {
            ($shift:expr, $bits:expr) => {{
                let shifted = v.wrapping_shl($shift);
                let min = -(1i32 << ($bits - 1));
                let max = (1i32 << ($bits - 1)) - 1;
                prop_assert_eq!(ssat_lsl::<$shift, $bits>(v), shifted.clamp(min, max));
            }};
        }
        check!(1, 16);
        check!(4, 16);
        check!(8, 24);
    }

    #[test]
    fn usat_lsl_matches_ref(v: u32) {
        macro_rules! check {
            ($shift:expr, $bits:expr) => {{
                // USAT reads the shifted 32-bit value as signed, so a 1 landing
                // in bit 31 saturates to 0.
                let shifted = v.wrapping_shl($shift) as i32;
                let expected = if shifted < 0 {
                    0u32
                } else {
                    (shifted as u32).min((1u32 << $bits) - 1)
                };
                prop_assert_eq!(usat_lsl::<$shift, $bits>(v), expected);
            }};
        }
        check!(1, 16);
        check!(4, 16);
        check!(8, 24);
    }
}
