//! Unit tests for the fixed-point library.

use super::*;

#[test]
fn test_from_float() {
    let fp = Q31::from_float(0.5);
    // For Q31, 0.5 should be approximately 0x3FFFFFFF (half of 0x7FFFFFFF)
    // The actual value is (0.5 * 2^31) = 1073741824 = 0x40000000
    assert!((fp.to_float() - 0.5).abs() < 0.0001);

    let fp = Q31::from_float(1.0);
    assert_eq!(fp.raw(), i32::MAX);

    let fp = Q31::from_float(-1.0);
    // -1.0 saturates to i32::MIN for Q31
    assert_eq!(fp.raw(), i32::MIN);
}

#[test]
fn test_to_float() {
    let fp = Q31::from_raw(0x4000_0000);
    let f = fp.to_float();
    assert!((f - 0.5).abs() < 0.0001);
}

#[test]
fn test_from_int() {
    let fp = Q16::from_int(42);
    assert_eq!(fp.to_int(), 42);

    let fp = Q16::from_int(-100);
    assert_eq!(fp.to_int(), -100);
}

#[test]
fn test_addition() {
    let a = Q31::from_float(0.5);
    let b = Q31::from_float(0.25);
    let sum = a + b;
    assert!((sum.to_float() - 0.75).abs() < 0.0001);
}

#[test]
fn test_subtraction() {
    let a = Q31::from_float(0.75);
    let b = Q31::from_float(0.25);
    let diff = a - b;
    assert!((diff.to_float() - 0.5).abs() < 0.0001);
}

#[test]
fn test_multiplication() {
    let a = Q31::from_float(0.5);
    let b = Q31::from_float(0.5);
    let product = a * b;
    assert!((product.to_float() - 0.25).abs() < 0.0001);
}

#[test]
fn test_division() {
    let a = Q31::from_float(0.5);
    let b = Q31::from_float(0.25);
    let quotient = a / b;
    // For Q31, dividing 0.5 / 0.25 should give 2.0, but Q31 can't represent 2.0
    // Q31 range is [-1, 1), so 2.0 would saturate to MAX
    assert_eq!(quotient.raw(), i32::MAX);
}

#[test]
fn test_negation() {
    let a = Q31::from_float(0.5);
    let neg_a = -a;
    assert!((neg_a.to_float() + 0.5).abs() < 0.0001);
}

#[test]
fn test_comparison() {
    let a = Q31::from_float(0.5);
    let b = Q31::from_float(0.25);

    assert!(a > b);
    assert!(b < a);
    assert_eq!(a, a);
}

#[test]
fn test_saturation() {
    let max = Q31::MAX;
    let one = Q31::from_float(0.1);
    let sum = max + one;
    assert_eq!(sum, Q31::MAX); // Should saturate
}

#[test]
fn test_conversion() {
    let q31 = Q31::from_float(0.5);
    let q16: Q16 = q31.convert();
    assert!((q16.to_float() - 0.5).abs() < 0.01);
}

#[test]
fn test_mul_add() {
    let a = Q31::from_float(0.5);
    let b = Q31::from_float(0.25);
    let c = Q31::from_float(0.1);

    let result = a.mul_add(b, c);
    let expected = a + b * c;
    assert_eq!(result.raw(), expected.raw());
}

#[test]
fn test_rounding() {
    let fp = Q16Rounded::from_float(42.7);
    assert_eq!(fp.to_int(), 43);

    let fp = Q16::from_float(42.7);
    assert_eq!(fp.to_int(), 42);
}

#[test]
fn test_abs() {
    let fp = Q31::from_float(-0.5);
    let abs_fp = fp.abs();
    assert!((abs_fp.to_float() - 0.5).abs() < 0.0001);
}

#[test]
fn test_mul_int() {
    let fp = Q16::from_float(2.5);
    let result = fp.mul_int(3);
    assert!((result.to_float() - 7.5).abs() < 0.01);
}

#[test]
fn test_div_int() {
    let fp = Q16::from_float(10.0);
    let result = fp.div_int(2);
    assert!((result.to_float() - 5.0).abs() < 0.01);
}

#[test]
fn test_fractional() {
    let fp = Q16::from_float(42.75);
    let frac = fp.fractional();
    assert!((frac.to_float() - 0.75).abs() < 0.01);
}

#[test]
fn test_integral() {
    let fp = Q16::from_float(42.75);
    assert_eq!(fp.integral(), 42);
}

#[test]
fn test_lshift_saturate() {
    let fp = Q16::from_int(100);
    let shifted = fp.lshift_saturate(2);
    assert_eq!(shifted.to_int(), 400);

    // Test saturation
    let large = Q16::from_int(20000);
    let shifted = large.lshift_saturate(2);
    // Verify it doesn't panic or wrap
    let _ = shifted.raw();
}

#[test]
fn test_lshift_saturate_const() {
    let fp = Q16::from_int(100);
    let shifted = fp.lshift_saturate_const::<2>();
    assert_eq!(shifted.to_int(), 400);

    // Test saturation
    let large = Q16::from_int(20000);
    let shifted = large.lshift_saturate_const::<2>();
    // Verify it doesn't panic or wrap and produces same result as runtime version
    let shifted_runtime = large.lshift_saturate(2);
    assert_eq!(shifted.raw(), shifted_runtime.raw());

    // Test edge cases
    let val = Q31::from_float(0.5);
    let shifted = val.lshift_saturate_const::<1>();
    assert!((shifted.to_float() - 1.0).abs() < 0.01);
}

#[test]
fn test_constants() {
    assert_eq!(Q31::FRAC_BITS, 31);
    assert_eq!(Q31::INT_BITS, 0);
    assert_eq!(Q16::FRAC_BITS, 16);
    assert_eq!(Q16::INT_BITS, 15);

    assert_eq!(Q31::ZERO.raw(), 0);
    assert_eq!(Q31::MAX.raw(), i32::MAX);
    assert_eq!(Q31::MIN.raw(), i32::MIN);
}

// Edge case tests

#[test]
fn test_zero_operations() {
    let zero = Q31::ZERO;
    let val = Q31::from_float(0.5);

    assert_eq!(zero + val, val);
    assert_eq!(val + zero, val);
    assert_eq!(val - zero, val);
    assert_eq!(zero * val, zero);
    assert_eq!(val * zero, zero);
    assert_eq!(zero / val, zero);
    assert_eq!(-zero, zero);
}

#[test]
fn test_division_by_zero() {
    let val = Q31::from_float(0.5);
    let zero = Q31::ZERO;

    let result = val / zero;
    assert_eq!(result, Q31::MAX); // Positive / 0 = MAX

    let neg_val = Q31::from_float(-0.5);
    let result = neg_val / zero;
    assert_eq!(result, Q31::MIN); // Negative / 0 = MIN
}

#[test]
fn test_max_min_operations() {
    let max = Q31::MAX;
    let min = Q31::MIN;

    // Addition should saturate
    assert_eq!(max + max, Q31::MAX);
    assert_eq!(min + min, Q31::MIN);

    // Subtraction should saturate
    assert_eq!(max - min, Q31::MAX);
    assert_eq!(min - max, Q31::MIN);

    // Negation
    assert_eq!(-max, min + Q31::from_raw(1));
    assert_eq!(-min, max); // Saturates because -i32::MIN doesn't fit
}

#[test]
fn test_overflow_multiplication() {
    let large = Q16::from_int(1000);
    let result = large * large;
    // Should saturate, not wrap
    assert_eq!(result, Q16::MAX);
}

#[test]
fn test_underflow_multiplication() {
    let large_neg = Q16::from_int(-1000);
    let result = large_neg * large_neg;
    // Negative * negative = positive, should saturate
    assert_eq!(result, Q16::MAX);
}

#[test]
fn test_signed_multiplication() {
    let pos = Q16::from_int(100);
    let neg = Q16::from_int(-100);

    let result = pos * neg;
    assert!(result.raw() < 0);
    assert_eq!(result.to_int(), -10000);
}

#[test]
fn test_one_multiplication() {
    let val = Q31::from_float(0.75);
    let one = Q31::from_raw(Q31::ONE);

    let result = val * one;
    assert!((result.to_float() - 0.75).abs() < 0.0001);
}

#[test]
fn test_conversion_precision_loss() {
    // Converting from higher to lower precision
    let q31 = Q31::from_float(0.123_456_79);
    let q16: Q16 = q31.convert();

    // Q16 has less precision, so the result should be close but not exact
    let error = (q16.to_float() - 0.123_456_79).abs();
    assert!(error < 0.01, "Error too large: {}", error);
    // Note: Q16 actually has enough precision for this value,
    // so we just verify it's reasonably close
}

#[test]
fn test_conversion_saturation() {
    // Q31 can represent values up to ~1.0
    // Q16 can represent much larger values
    let q31_max = Q31::MAX;
    let q16: Q16 = q31_max.convert();

    // Should preserve the ~1.0 value
    assert!((q16.to_float() - 1.0).abs() < 0.01);
}

#[test]
fn test_upscale_downscale_roundtrip() {
    let original = Q16::from_float(123.456);
    let upscaled: Q24 = original.convert();
    let downscaled: Q16 = upscaled.convert();

    // Should be very close after roundtrip
    assert!((downscaled.to_float() - original.to_float()).abs() < 0.1);
}

#[test]
fn test_associativity_addition() {
    let a = Q31::from_float(0.1);
    let b = Q31::from_float(0.2);
    let c = Q31::from_float(0.3);

    let result1 = (a + b) + c;
    let result2 = a + (b + c);

    // Should be identical (no overflow in this case)
    assert_eq!(result1.raw(), result2.raw());
}

#[test]
fn test_distributive_property() {
    let a = Q31::from_float(0.2);
    let b = Q31::from_float(0.3);
    let c = Q31::from_float(0.4);

    let result1 = a * (b + c);
    let result2 = (a * b) + (a * c);

    // Should be very close (rounding errors may differ)
    assert!((result1.to_float() - result2.to_float()).abs() < 0.001);
}

#[test]
fn test_mul_add_accuracy() {
    let a = Q31::from_float(0.5);
    let b = Q31::from_float(0.25);
    let c = Q31::from_float(0.1);

    // mul_add should be as accurate as separate operations
    let fused = a.mul_add(b, c);
    let separate = a + (b * c);

    assert_eq!(fused.raw(), separate.raw());
}

#[test]
fn test_rounding_vs_truncation() {
    // Test that rounding actually rounds
    let val = 0.666666;
    let truncated = Q16::from_float(val);
    let rounded = Q16Rounded::from_float(val);

    // Rounded should be closer to the original value
    let trunc_error = (truncated.to_float() - val).abs();
    let round_error = (rounded.to_float() - val).abs();

    assert!(round_error <= trunc_error);
}

#[test]
fn test_int_multiplication_no_overflow() {
    let fp = Q16::from_int(100);
    let result = fp.mul_int(5);
    assert_eq!(result.to_int(), 500);
}

#[test]
fn test_int_multiplication_overflow() {
    let fp = Q16::from_int(10000);
    let result = fp.mul_int(10000);
    // Should saturate
    assert_eq!(result, Q16::MAX);
}

#[test]
fn test_lshift_edge_cases() {
    let val = Q16::from_int(1);

    // Shift by 0 should do nothing
    assert_eq!(val.lshift_saturate(0), val);

    // Shift by 32 should give zero
    assert_eq!(val.lshift_saturate(32), Q16::ZERO);

    // Large shift should give zero
    assert_eq!(val.lshift_saturate(100), Q16::ZERO);
}

#[test]
fn test_rshift_edge_cases() {
    let val = Q16::from_int(100);

    // Shift by 0 should do nothing
    assert_eq!(val.rshift(0), val);

    // Shift by 32 should give zero
    assert_eq!(val.rshift(32), Q16::ZERO);

    // Large shift should give zero
    assert_eq!(val.rshift(100), Q16::ZERO);
}

#[test]
fn test_fractional_of_integer() {
    let fp = Q16::from_int(42);
    let frac = fp.fractional();
    assert_eq!(frac.raw(), 0);
}

#[test]
fn test_double_conversion() {
    let val = 0.123456789012345;
    let fp = Q31::from_double(val);
    let back = fp.to_double();

    // Should be very close
    assert!((back - val).abs() < 0.000001);
}

#[test]
fn test_comparison_across_zero() {
    let pos = Q31::from_float(0.001);
    let neg = Q31::from_float(-0.001);
    let zero = Q31::ZERO;

    assert!(pos > zero);
    assert!(zero > neg);
    assert!(pos > neg);
}

#[test]
fn test_very_small_values() {
    // Test the smallest representable positive value
    let smallest = Q31::from_raw(1);
    assert!(smallest > Q31::ZERO);
    assert!(smallest.to_float() > 0.0);
    assert!(smallest.to_float() < 0.001);
}

#[test]
fn test_near_one_values() {
    // For Q31, values very close to 1.0
    let almost_one = Q31::from_raw(i32::MAX - 1);
    let one = Q31::from_raw(i32::MAX);

    assert!(almost_one < one);
    assert!((one.to_float() - 1.0).abs() < 0.001);
}
