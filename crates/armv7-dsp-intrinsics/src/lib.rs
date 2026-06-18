//! # ARMv7 DSP Intrinsics
//!
//! This crate provides safe wrappers around ARMv7 DSP instructions for efficient
//! fixed-point arithmetic. These instructions are available on ARMv7-A processors
//! with the DSP extension (like ARM Cortex-A7 used in Deluge).
//!
//! ## Available Instructions
//!
//! ### Multiply Instructions
//! - **SMMUL**: Signed Most Significant Word Multiply (32x32 → high 32 bits)
//! - **SMMULR**: SMMUL with rounding
//! - **SMMLA**: Signed Most Significant Word Multiply Accumulate
//! - **SMMLAR**: SMMLA with rounding
//! - **SMMLSR**: Signed Most Significant Word Multiply Subtract with rounding
//!
//! ### Saturating Arithmetic
//! - **QADD**: Saturating Add
//! - **QSUB**: Saturating Subtract
//! - **QDADD**: Saturating Double and Add
//! - **QDSUB**: Saturating Double and Subtract
//!
//! ### Saturation
//! - **SSAT**: Signed Saturate to N bits
//! - **USAT**: Unsigned Saturate to N bits
//! - **SSAT (LSL)**: Signed Saturate with left shift
//! - **USAT (LSL)**: Unsigned Saturate with left shift
//!
//! ## Platform Support
//!
//! These intrinsics are only available on ARMv7-A targets with DSP extensions.
//! On other platforms, portable fallback implementations are used.
//!
//! ## Naming Conventions
//!
//! Functions are available in two naming styles:
//! - **ARM instruction names** (e.g., `qadd`, `smmul`) - direct mapping to ARM instructions
//! - **Rust-style aliases** (e.g., `saturating_add`, `mul_high`) - idiomatic Rust naming

#![no_std]
// `core::arch::arm` DSP intrinsics (`__qadd`, `__smmul`, …) are gated behind
// both unstable features in current nightlies — the DSP intrinsics were folded
// in under the same gate as the NEON ones. Both are needed for the `nightly`
// (intrinsic) path; the default path uses inline asm and needs neither.
#![cfg_attr(
    feature = "nightly",
    feature(stdarch_arm_dsp, stdarch_arm_neon_intrinsics)
)]

// ============================================================================
// Saturating Arithmetic
// ============================================================================

/// Signed saturating add (QADD instruction)
///
/// Adds two 32-bit signed integers with saturation.
/// Returns i32::MAX on overflow, i32::MIN on underflow.
#[inline]
pub fn qadd(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        #[cfg(feature = "nightly")]
        unsafe {
            core::arch::arm::__qadd(a, b)
        }
        #[cfg(not(feature = "nightly"))]
        {
            // Stable Rust: use inline assembly
            let result: i32;
            unsafe {
                core::arch::asm!(
                    "qadd {result}, {a}, {b}",
                    a = in(reg) a,
                    b = in(reg) b,
                    result = out(reg) result,
                    options(pure, nomem, nostack)
                );
            }
            result
        }
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        // Portable fallback
        a.saturating_add(b)
    }
}

/// Rust-style alias for [`qadd`]
#[inline]
pub fn saturating_add(a: i32, b: i32) -> i32 {
    qadd(a, b)
}

/// Signed saturating subtract (QSUB instruction)
///
/// Subtracts two 32-bit signed integers with saturation.
/// Returns i32::MAX on overflow, i32::MIN on underflow.
#[inline]
pub fn qsub(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        #[cfg(feature = "nightly")]
        unsafe {
            core::arch::arm::__qsub(a, b)
        }
        #[cfg(not(feature = "nightly"))]
        {
            let result: i32;
            unsafe {
                core::arch::asm!(
                    "qsub {result}, {a}, {b}",
                    a = in(reg) a,
                    b = in(reg) b,
                    result = out(reg) result,
                    options(pure, nomem, nostack)
                );
            }
            result
        }
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        a.saturating_sub(b)
    }
}

/// Rust-style alias for [`qsub`]
#[inline]
pub fn saturating_sub(a: i32, b: i32) -> i32 {
    qsub(a, b)
}

/// Rust-style alias for [`qdadd`]
#[inline]
pub fn saturating_double_add(a: i32, b: i32) -> i32 {
    qdadd(a, b)
}

/// Rust-style alias for [`qdsub`]
#[inline]
pub fn saturating_double_sub(a: i32, b: i32) -> i32 {
    qdsub(a, b)
}

// ============================================================================
// Multiply Instructions
// ============================================================================

/// Signed most significant word multiply (SMMUL instruction)
///
/// Multiplies two 32-bit signed integers and returns the high 32 bits of the result.
/// Equivalent to: (a * b) >> 32
///
/// This is ideal for Q31 fixed-point multiplication where you want the result
/// in the same Q31 format.
#[inline]
pub fn smmul(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "smmul {result}, {a}, {b}",
                a = in(reg) a,
                b = in(reg) b,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        // Portable fallback
        ((a as i64 * b as i64) >> 32) as i32
    }
}

/// Signed most significant word multiply with rounding (SMMULR instruction)
///
/// Multiplies two 32-bit signed integers, adds 0x80000000 for rounding,
/// and returns the high 32 bits.
/// Equivalent to: ((a * b) + 0x80000000) >> 32
///
/// This provides Q31 multiplication with rounding to nearest.
#[inline]
pub fn smmulr(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "smmulr {result}, {a}, {b}",
                a = in(reg) a,
                b = in(reg) b,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        // Portable fallback with rounding
        let product = a as i64 * b as i64;
        let rounded = product.wrapping_add(0x80000000);
        (rounded >> 32) as i32
    }
}

/// Rust-style alias for [`smmul`]
#[inline]
pub fn mul_high(a: i32, b: i32) -> i32 {
    smmul(a, b)
}

/// Rust-style alias for [`smmulr`]
#[inline]
pub fn mul_high_round(a: i32, b: i32) -> i32 {
    smmulr(a, b)
}

/// Signed most significant word multiply accumulate (SMMLA instruction)
///
/// Multiplies two 32-bit signed integers, extracts the high 32 bits,
/// and adds them to an accumulator.
/// Equivalent to: acc + ((a * b) >> 32)
#[inline]
pub fn smmla(acc: i32, a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "smmla {result}, {a}, {b}, {acc}",
                acc = in(reg) acc,
                a = in(reg) a,
                b = in(reg) b,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let product_high = ((a as i64 * b as i64) >> 32) as i32;
        acc.wrapping_add(product_high)
    }
}

/// Signed most significant word multiply accumulate with rounding (SMMLAR instruction)
///
/// Multiplies two 32-bit signed integers with rounding, extracts the high 32 bits,
/// and adds them to an accumulator.
/// Equivalent to: acc + (((a * b) + 0x80000000) >> 32)
#[inline]
pub fn smmlar(acc: i32, a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "smmlar {result}, {a}, {b}, {acc}",
                acc = in(reg) acc,
                a = in(reg) a,
                b = in(reg) b,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let product = a as i64 * b as i64;
        let product_high = ((product + 0x80000000) >> 32) as i32;
        acc.wrapping_add(product_high)
    }
}

/// Signed most significant word multiply subtract with rounding (SMMLSR instruction)
///
/// Multiplies two 32-bit signed integers with rounding, extracts the high 32 bits,
/// and subtracts them from an accumulator.
/// Equivalent to: acc - (((a * b) + 0x80000000) >> 32)
#[inline]
pub fn smmlsr(acc: i32, a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "smmlsr {result}, {a}, {b}, {acc}",
                acc = in(reg) acc,
                a = in(reg) a,
                b = in(reg) b,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let product = a as i64 * b as i64;
        let product_high = ((product + 0x80000000) >> 32) as i32;
        acc.wrapping_sub(product_high)
    }
}

/// Rust-style alias for [`smmla`]
#[inline]
pub fn mul_accumulate_high(acc: i32, a: i32, b: i32) -> i32 {
    smmla(acc, a, b)
}

/// Rust-style alias for [`smmlar`]
#[inline]
pub fn mul_accumulate_high_round(acc: i32, a: i32, b: i32) -> i32 {
    smmlar(acc, a, b)
}

/// Rust-style alias for [`smmlsr`]
#[inline]
pub fn mul_subtract_high_round(acc: i32, a: i32, b: i32) -> i32 {
    smmlsr(acc, a, b)
}

// ============================================================================
// Saturation
// ============================================================================

/// Signed saturate to bit position (SSAT instruction)
///
/// Saturates a signed 32-bit value to a signed N-bit value.
/// N must be between 1 and 32 (exclusive of 32).
///
/// # Examples
/// ```
/// use armv7_dsp_intrinsics::ssat;
///
/// // Saturate to 16-bit range
/// assert_eq!(ssat::<16>(100000), 32767);
/// assert_eq!(ssat::<16>(-100000), -32768);
/// ```
#[inline]
pub fn ssat<const BITS: u32>(val: i32) -> i32 {
    debug_assert!(BITS > 0 && BITS < 32, "BITS must be 1..31");

    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "ssat {result}, #{bits}, {val}",
                bits = const BITS,
                val = in(reg) val,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let min = -(1i32 << (BITS - 1));
        let max = (1i32 << (BITS - 1)) - 1;
        val.clamp(min, max)
    }
}

/// Unsigned saturate to bit position (USAT instruction)
///
/// Saturates a signed 32-bit value to an unsigned N-bit value.
/// N must be between 0 and 32 (inclusive).
///
/// # Examples
/// ```
/// use armv7_dsp_intrinsics::usat;
///
/// // Saturate to 8-bit range
/// assert_eq!(usat::<8>(1000), 255);
/// assert_eq!(usat::<8>(-10), 0);
/// ```
#[inline]
pub fn usat<const BITS: u32>(val: i32) -> u32 {
    debug_assert!(BITS <= 32, "BITS must be 0..=32");

    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: u32;
        unsafe {
            core::arch::asm!(
                "usat {result}, #{bits}, {val}",
                bits = const BITS,
                val = in(reg) val,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        if BITS == 32 {
            val.max(0) as u32
        } else {
            let max = (1u32 << BITS) - 1;
            val.clamp(0, max as i32) as u32
        }
    }
}

/// Signed saturate with left shift (SSAT with LSL)
///
/// Shifts val left by SHIFT bits, then saturates to BITS.
///
/// # Examples
/// ```
/// use armv7_dsp_intrinsics::ssat_lsl;
///
/// // Shift left by 4, saturate to 16 bits
/// assert_eq!(ssat_lsl::<4, 16>(1000), 16000);
/// assert_eq!(ssat_lsl::<4, 16>(10000), 32767); // Saturated
/// ```
#[inline]
pub fn ssat_lsl<const SHIFT: u32, const BITS: u32>(val: i32) -> i32 {
    debug_assert!(SHIFT < 32 && BITS > 0 && BITS < 32);

    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "ssat {result}, #{bits}, {val}, lsl #{shift}",
                bits = const BITS,
                shift = const SHIFT,
                val = in(reg) val,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let shifted = (val as i64) << SHIFT;
        let min = -(1i64 << (BITS - 1));
        let max = (1i64 << (BITS - 1)) - 1;
        shifted.clamp(min, max) as i32
    }
}

/// Unsigned saturate with left shift (USAT with LSL)
///
/// Shifts val left by SHIFT bits, then saturates to BITS.
#[inline]
pub fn usat_lsl<const SHIFT: u32, const BITS: u32>(val: u32) -> u32 {
    debug_assert!(SHIFT < 32 && BITS <= 32);

    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: u32;
        unsafe {
            core::arch::asm!(
                "usat {result}, #{bits}, {val}, lsl #{shift}",
                bits = const BITS,
                shift = const SHIFT,
                val = in(reg) val,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let shifted = (val as u64) << SHIFT;
        if BITS == 32 {
            shifted.min(u32::MAX as u64) as u32
        } else {
            let max = (1u64 << BITS) - 1;
            shifted.min(max) as u32
        }
    }
}

/// Rust-style alias for [`ssat`]
#[inline]
pub fn saturate_signed<const BITS: u32>(val: i32) -> i32 {
    ssat::<BITS>(val)
}

/// Rust-style alias for [`usat`]
#[inline]
pub fn saturate_unsigned<const BITS: u32>(val: i32) -> u32 {
    usat::<BITS>(val)
}

/// Rust-style alias for [`ssat_lsl`]
#[inline]
pub fn saturate_signed_shl<const SHIFT: u32, const BITS: u32>(val: i32) -> i32 {
    ssat_lsl::<SHIFT, BITS>(val)
}

/// Rust-style alias for [`usat_lsl`]
#[inline]
pub fn saturate_unsigned_shl<const SHIFT: u32, const BITS: u32>(val: u32) -> u32 {
    usat_lsl::<SHIFT, BITS>(val)
}

/// Saturating left shift to the signed 32-bit range.
///
/// Saturates `val` to the signed `(32 - SHIFT)`-bit range and then shifts left
/// by `SHIFT`, so the shift can never overflow `i32`. This is **not** the same as
/// [`ssat_lsl`]: the hardware `SSAT …, LSL #n` shifts *before* saturating and the
/// shift wraps modulo 2³², which cannot represent an `i32`-saturating left shift.
/// Mirrors the Deluge firmware's `signed_saturate<32 - shift>(v) << shift`.
///
/// `SHIFT` must be in `1..32` (0 is identity, ≥32 collapses to 0).
#[inline]
pub fn lshift_saturate<const SHIFT: u32>(val: i32) -> i32 {
    if SHIFT == 0 {
        return val;
    }
    if SHIFT >= 32 {
        return 0;
    }

    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        // Saturate to (32 - SHIFT) signed bits with SSAT, then a plain LSL.
        let saturated: i32;
        unsafe {
            core::arch::asm!(
                "ssat {r}, #{bits}, {v}",
                bits = const 32 - SHIFT,
                v = in(reg) val,
                r = out(reg) saturated,
                options(pure, nomem, nostack)
            );
        }
        saturated << SHIFT
    }

    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let max = i32::MAX >> SHIFT;
        let min = i32::MIN >> SHIFT;
        val.clamp(min, max) << SHIFT
    }
}

// ============================================================================
// Float to Fixed-Point Conversion (VCVT instructions)
// ============================================================================

/// Convert float to fixed-point with FRAC_BITS fractional bits (VCVT instruction)
///
/// Uses the ARM VCVT.S32.F32 instruction to convert a floating-point value
/// to a 32-bit signed fixed-point value with FRAC_BITS fractional bits.
///
/// # Examples
/// ```
/// use armv7_dsp_intrinsics::vcvt_f32_to_fixed;
///
/// // Convert 0.5 to Q31 format
/// let q31 = vcvt_f32_to_fixed::<31>(0.5);
/// // Result is approximately 0.5 * 2^31 = 1073741824
/// ```
#[inline]
pub fn vcvt_f32_to_fixed<const FRAC_BITS: u32>(value: f32) -> i32 {
    debug_assert!(FRAC_BITS <= 32, "FRAC_BITS must be <= 32");

    #[cfg(all(target_arch = "arm", target_feature = "vfp2"))]
    {
        // Use VCVT.S32.F32 instruction
        // This requires VFP (Vector Floating Point) which includes VCVT
        // The fixed-point form of VCVT operates in place: source and destination
        // must be the *same* S register (`vcvt.s32.f32 Sd, Sd, #n`). Inline-asm
        // `inout` needs one type for that shared operand, so keep it `f32` and
        // reinterpret the result — the S register now holds the i32 bit pattern.
        // The `fbits` immediate is the number of fractional bits.
        let bits: f32;
        unsafe {
            core::arch::asm!(
                "vcvt.s32.f32 {x}, {x}, #{fbits}",
                fbits = const FRAC_BITS,
                x = inout(sreg) value => bits,
                options(pure, nomem, nostack)
            );
        }
        bits.to_bits() as i32
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "vfp2")))]
    {
        // Portable fallback
        let scale = if FRAC_BITS == 31 {
            // Special case for Q31 to avoid overflow
            i32::MAX as f32
        } else {
            (1u32 << FRAC_BITS) as f32
        };

        let scaled = value * scale;

        // Saturate to i32 range
        if scaled > i32::MAX as f32 {
            i32::MAX
        } else if scaled < i32::MIN as f32 {
            i32::MIN
        } else {
            scaled as i32
        }
    }
}

/// Convert fixed-point to float with FRAC_BITS fractional bits (VCVT instruction)
///
/// Uses the ARM VCVT.F32.S32 instruction to convert a 32-bit signed fixed-point
/// value with FRAC_BITS fractional bits to a floating-point value.
///
/// # Examples
/// ```
/// use armv7_dsp_intrinsics::vcvt_fixed_to_f32;
///
/// // Convert Q31 value back to float
/// let q31 = 1073741824; // Approximately 0.5 in Q31
/// let f = vcvt_fixed_to_f32::<31>(q31);
/// // f is approximately 0.5
/// ```
#[inline]
pub fn vcvt_fixed_to_f32<const FRAC_BITS: u32>(value: i32) -> f32 {
    debug_assert!(FRAC_BITS <= 32, "FRAC_BITS must be <= 32");

    #[cfg(all(target_arch = "arm", target_feature = "vfp2"))]
    {
        // Use VCVT.F32.S32 instruction. Like the inverse conversion, the
        // fixed-point form is in place (`vcvt.f32.s32 Sd, Sd, #n`); share one
        // `i32`-typed operand and reinterpret the result — the S register holds
        // the f32 bit pattern once the instruction has run.
        let bits: i32;
        unsafe {
            core::arch::asm!(
                "vcvt.f32.s32 {x}, {x}, #{fbits}",
                fbits = const FRAC_BITS,
                x = inout(sreg) value => bits,
                options(pure, nomem, nostack)
            );
        }
        
        f32::from_bits(bits as u32)
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "vfp2")))]
    {
        // Portable fallback
        let scale = if FRAC_BITS == 31 {
            i32::MAX as f32
        } else {
            (1u32 << FRAC_BITS) as f32
        };

        value as f32 / scale
    }
}

// ============================================================================
// Double and Add/Subtract
// ============================================================================

/// Signed saturating double and add (QDADD instruction)
///
/// Doubles `b`, saturates, then adds to `a` with saturation.
/// Equivalent to: qadd(a, qsaturate(b * 2))
#[inline]
pub fn qdadd(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "qdadd {result}, {a}, {b}",
                a = in(reg) a,
                b = in(reg) b,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let doubled = b.saturating_mul(2);
        a.saturating_add(doubled)
    }
}

/// Signed saturating double and subtract (QDSUB instruction)
///
/// Doubles `b`, saturates, then subtracts from `a` with saturation.
/// Equivalent to: qsub(a, qsaturate(b * 2))
#[inline]
pub fn qdsub(a: i32, b: i32) -> i32 {
    #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
    {
        let result: i32;
        unsafe {
            core::arch::asm!(
                "qdsub {result}, {a}, {b}",
                a = in(reg) a,
                b = in(reg) b,
                result = out(reg) result,
                options(pure, nomem, nostack)
            );
        }
        result
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
    {
        let doubled = b.saturating_mul(2);
        a.saturating_sub(doubled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qadd() {
        assert_eq!(qadd(100, 200), 300);
        assert_eq!(qadd(i32::MAX, 1), i32::MAX); // Saturation
        assert_eq!(qadd(i32::MIN, -1), i32::MIN); // Saturation
    }

    #[test]
    fn test_qsub() {
        assert_eq!(qsub(200, 100), 100);
        assert_eq!(qsub(i32::MIN, 1), i32::MIN); // Saturation
        assert_eq!(qsub(i32::MAX, -1), i32::MAX); // Saturation
    }

    #[test]
    fn test_smmul() {
        // Q31 multiplication: 0.5 * 0.5 = 0.25
        // In Q31: 0.5 = 0x40000000, 0.25 = 0x20000000
        let half_q31 = 0x40000000i32;
        let result = smmul(half_q31, half_q31);
        // SMMUL returns high 32 bits of 64-bit product
        // 0x40000000 * 0x40000000 = 0x1000_0000_0000_0000
        // High 32 bits = 0x10000000
        let expected = 0x10000000i32;
        assert_eq!(result, expected);
    }

    #[test]
    fn test_smmulr() {
        // Test rounding behavior
        let a = i32::MAX / 2;
        let b = i32::MAX / 2;
        let result = smmulr(a, b);
        let unrounded = smmul(a, b);
        // Rounded result should be >= unrounded
        assert!(result >= unrounded);
    }

    #[test]
    fn test_smmla() {
        let acc = 1000;
        let a = i32::MAX / 2;
        let b = i32::MAX / 4;
        let result = smmla(acc, a, b);
        let product_high = smmul(a, b);
        assert_eq!(result, acc.wrapping_add(product_high));
    }

    #[test]
    fn test_smmlar() {
        let acc = 1000;
        let a = 0x40000000i32;
        let b = 0x40000000i32;
        let result = smmlar(acc, a, b);
        let product_high_rounded = smmulr(a, b);
        assert_eq!(result, acc.wrapping_add(product_high_rounded));
    }

    #[test]
    fn test_smmlsr() {
        let acc = 1000;
        let a = 0x40000000i32;
        let b = 0x20000000i32;
        let result = smmlsr(acc, a, b);
        let product_high_rounded = smmulr(a, b);
        assert_eq!(result, acc.wrapping_sub(product_high_rounded));
    }

    #[test]
    fn test_ssat() {
        assert_eq!(ssat::<16>(100), 100);
        assert_eq!(ssat::<16>(100000), 32767);
        assert_eq!(ssat::<16>(-100000), -32768);
        assert_eq!(ssat::<8>(255), 127);
    }

    #[test]
    fn test_usat() {
        assert_eq!(usat::<8>(100), 100);
        assert_eq!(usat::<8>(1000), 255);
        assert_eq!(usat::<8>(-10), 0);
        assert_eq!(usat::<16>(100000), 65535);
    }

    #[test]
    fn test_ssat_lsl() {
        assert_eq!(ssat_lsl::<2, 16>(100), 400);
        assert_eq!(ssat_lsl::<4, 16>(10000), 32767); // Saturated
        assert_eq!(ssat_lsl::<4, 16>(-10000), -32768); // Saturated
    }

    #[test]
    fn test_usat_lsl() {
        assert_eq!(usat_lsl::<2, 16>(100), 400);
        assert_eq!(usat_lsl::<4, 8>(100), 255); // Saturated
    }

    #[test]
    fn test_vcvt_f32_to_fixed() {
        // Test Q31 conversion (0.5 = 0x40000000 in Q31)
        let q31 = vcvt_f32_to_fixed::<31>(0.5);
        assert_eq!(q31, 0x40000000);

        // Test Q16 conversion
        let q16 = vcvt_f32_to_fixed::<16>(42.5);
        let expected = (42.5 * 65536.0) as i32;
        assert!((q16 - expected).abs() <= 1); // Allow for rounding

        // Test saturation
        let max = vcvt_f32_to_fixed::<31>(10.0); // > 1.0 for Q31
        assert_eq!(max, i32::MAX);

        let min = vcvt_f32_to_fixed::<31>(-10.0);
        assert_eq!(min, i32::MIN);
    }

    #[test]
    fn test_vcvt_fixed_to_f32() {
        // Test Q31 conversion
        let f = vcvt_fixed_to_f32::<31>(0x40000000);
        assert!((f - 0.5).abs() < 0.0001);

        // Test Q16 conversion
        let q16 = (42.5 * 65536.0) as i32;
        let f = vcvt_fixed_to_f32::<16>(q16);
        assert!((f - 42.5).abs() < 0.01);

        // Test roundtrip
        let original = 0.75f32;
        let q31 = vcvt_f32_to_fixed::<31>(original);
        let back = vcvt_fixed_to_f32::<31>(q31);
        assert!((back - original).abs() < 0.0001);
    }
}
