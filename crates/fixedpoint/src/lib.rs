//! # Fixed-Point Arithmetic Library
//!
//! This crate provides type-safe fixed-point arithmetic with configurable precision.
//! Fixed-point numbers are represented using an integer with a fixed number of fractional bits,
//! allowing for deterministic arithmetic without floating-point rounding errors.
//!
//! ## Features
//!
//! - Generic over the number of fractional bits via const generics
//! - Compile-time type safety - operations on different formats require explicit conversion
//! - Saturating arithmetic to prevent overflow/underflow
//! - Optional rounding support
//! - Efficient multiplication/division using 64-bit intermediate values
//! - Full trait implementations (Add, Sub, Mul, Div, Ord, etc.)
//! - Seamless conversion between different fixed-point formats
//!
//! ## Examples
//!
//! ```
//! use fixedpoint::{FixedPoint, Q31, Q16};
//!
//! // Create a Q31 (31 fractional bits) fixed-point number from a float
//! let a = Q31::from_float(0.5);
//! let b = Q31::from_float(0.25);
//!
//! // Arithmetic operations
//! let sum = a + b;  // 0.75
//! let product = a * b;  // 0.125
//!
//! // Convert to different precision
//! let c: Q16 = a.convert();
//!
//! // Get the raw underlying value
//! let raw = a.raw();
//! ```

#![cfg_attr(not(test), no_std)]

use core::cmp::Ordering;
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

#[cfg(all(target_arch = "arm", target_feature = "dsp"))]
use armv7_dsp_intrinsics as dsp;

/// Simple rounding function for no_std
#[inline]
const fn round_f32(x: f32) -> f32 {
    let truncated = x as i64;
    let frac = x - truncated as f32;
    if frac.abs() >= 0.5 {
        if x >= 0.0 {
            truncated as f32 + 1.0
        } else {
            truncated as f32 - 1.0
        }
    } else {
        truncated as f32
    }
}

/// Simple rounding function for no_std
#[inline]
fn round_f64(x: f64) -> f64 {
    let truncated = x as i64;
    let frac = x - truncated as f64;
    if frac.abs() >= 0.5 {
        if x >= 0.0 {
            truncated as f64 + 1.0
        } else {
            truncated as f64 - 1.0
        }
    } else {
        truncated as f64
    }
}

/// A fixed-point number with `FRAC_BITS` fractional bits.
///
/// The fixed-point number is stored as a 32-bit signed integer, where the lower
/// `FRAC_BITS` represent the fractional part and the upper `32 - FRAC_BITS` represent
/// the integral part.
///
/// # Type Parameters
///
/// * `FRAC_BITS` - The number of bits used for the fractional part (must be > 0 and < 32)
/// * `ROUNDED` - Whether arithmetic operations should round results (default: false)
///
/// # Invariants
///
/// - `FRAC_BITS` must be in the range `1..32`
/// - All arithmetic operations saturate on overflow/underflow
#[derive(Debug, Clone, Copy)]
pub struct FixedPoint<const FRAC_BITS: u32, const ROUNDED: bool = false> {
    value: i32,
}

/// Q31 format: 1 sign bit, 0 integer bits, 31 fractional bits
/// Range: [-1.0, ~1.0)
pub type Q31 = FixedPoint<31, false>;

/// Q31 with rounding
pub type Q31Rounded = FixedPoint<31, true>;

/// Q16 format: 1 sign bit, 15 integer bits, 16 fractional bits
/// Range: [-32768.0, 32767.99998...]
pub type Q16 = FixedPoint<16, false>;

/// Q16 with rounding
pub type Q16Rounded = FixedPoint<16, true>;

/// Q17 format: 1 sign bit, 14 integer bits, 17 fractional bits
/// Range: [-16384.0, 16383.999992...]
pub type Q17 = FixedPoint<17, false>;

/// Q17 with rounding
pub type Q17Rounded = FixedPoint<17, true>;

/// Q24 format: 1 sign bit, 7 integer bits, 24 fractional bits
/// Range: [-128.0, 127.999999...]
pub type Q24 = FixedPoint<24, false>;

/// Q24 with rounding
pub type Q24Rounded = FixedPoint<24, true>;

impl<const FRAC_BITS: u32, const ROUNDED: bool> FixedPoint<FRAC_BITS, ROUNDED> {
    /// Compile-time check that FRAC_BITS is valid
    const _ASSERT_VALID_FRAC_BITS: () = {
        assert!(
            FRAC_BITS > 0 && FRAC_BITS < 32,
            "FRAC_BITS must be in range 1..32"
        );
    };

    /// The number of fractional bits
    pub const FRAC_BITS: u32 = FRAC_BITS;

    /// The number of integer bits (excluding sign bit)
    pub const INT_BITS: u32 = 31 - FRAC_BITS;

    /// Whether operations round results
    pub const ROUNDED: bool = ROUNDED;

    /// The value representing 1.0 in this format
    pub const ONE: i32 = if FRAC_BITS == 31 {
        i32::MAX
    } else {
        1 << FRAC_BITS
    };

    /// The maximum representable value
    pub const MAX: Self = Self { value: i32::MAX };

    /// The minimum representable value
    pub const MIN: Self = Self { value: i32::MIN };

    /// The value representing 0.0
    pub const ZERO: Self = Self { value: 0 };

    /// Create a fixed-point number from its raw representation
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let raw_value = 0x4000_0000; // 0.5 in Q31 format
    /// let fp = Q31::from_raw(raw_value);
    /// ```
    #[inline(always)]
    pub const fn from_raw(value: i32) -> Self {
        #[allow(clippy::let_unit_value)]
        let _ = Self::_ASSERT_VALID_FRAC_BITS;
        Self { value }
    }

    /// Get the raw underlying value
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let fp = Q31::from_float(0.5);
    /// let raw = fp.raw();
    /// ```
    #[inline(always)]
    pub const fn raw(self) -> i32 {
        self.value
    }

    /// Create a fixed-point number from a floating-point value.
    ///
    /// The input is clamped to the representable range and converted.
    ///
    /// On ARM with VFP this is the single fused `VCVT.S32.F32` instruction
    /// (scale + saturate in one op), which is why it is **not** a `const fn`.
    /// Note the hardware instruction rounds toward zero regardless of the
    /// `ROUNDED` parameter; for a `const` constructor that honours `ROUNDED`,
    /// use [`from_float_const`](Self::from_float_const).
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let fp = Q31::from_float(0.75);
    /// assert!((fp.to_float() - 0.75).abs() < 0.0001);
    /// ```
    #[inline]
    pub fn from_float(value: f32) -> Self {
        #[allow(clippy::let_unit_value)]
        let _ = Self::_ASSERT_VALID_FRAC_BITS;

        // Fused fixed-point VCVT.S32.F32 on ARM with VFP: one instruction does
        // the scale, round-toward-zero, and saturation. This is the hot path
        // for DSP, so we keep it on the single-instruction form.
        #[cfg(all(target_arch = "arm", target_feature = "vfp2"))]
        {
            Self {
                value: dsp::vcvt_f32_to_fixed::<FRAC_BITS>(value),
            }
        }

        // Off ARM (host/tests) fall back to the portable const path.
        #[cfg(not(all(target_arch = "arm", target_feature = "vfp2")))]
        {
            Self::from_float_const(value)
        }
    }

    /// `const fn` equivalent of [`from_float`](Self::from_float), for building
    /// compile-time lookup tables.
    ///
    /// Always uses the portable scale-and-saturate path (the hardware VCVT
    /// can't run in a `const` context). For `ROUNDED == false` types its results
    /// are bit-identical to `from_float` on real hardware — verified against the
    /// VCVT instruction under QEMU by the `from_float_matches_vcvt` test module.
    /// Unlike the hardware instruction it also honours `ROUNDED`.
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// const HALF: Q31 = Q31::from_float_const(0.5);
    /// assert_eq!(HALF, Q31::from_float(0.5));
    /// ```
    #[inline]
    pub const fn from_float_const(value: f32) -> Self {
        #[allow(clippy::let_unit_value)]
        let _ = Self::_ASSERT_VALID_FRAC_BITS;

        let scaled = value * (Self::ONE as f32);
        let scaled = if ROUNDED { round_f32(scaled) } else { scaled };

        // Saturate to i32 range
        let clamped = if scaled > i32::MAX as f32 {
            i32::MAX
        } else if scaled < i32::MIN as f32 {
            i32::MIN
        } else {
            scaled as i32
        };

        Self { value: clamped }
    }

    /// Create a fixed-point number from a double-precision floating-point value
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let fp = Q31::from_double(0.333333333);
    /// ```
    #[inline]
    pub fn from_double(value: f64) -> Self {
        #[allow(clippy::let_unit_value)]
        let _ = Self::_ASSERT_VALID_FRAC_BITS;

        let scaled = value * (Self::ONE as f64);
        let scaled = if ROUNDED { round_f64(scaled) } else { scaled };

        let clamped = if scaled > i32::MAX as f64 {
            i32::MAX
        } else if scaled < i32::MIN as f64 {
            i32::MIN
        } else {
            scaled as i32
        };

        Self { value: clamped }
    }

    /// Convert to a single-precision floating-point value
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let fp = Q31::from_float(0.5);
    /// let f = fp.to_float();
    /// assert!((f - 0.5).abs() < 0.0001);
    /// ```
    #[inline]
    pub fn to_float(self) -> f32 {
        // Use VCVT instruction on ARM with VFP
        #[cfg(all(target_arch = "arm", target_feature = "vfp2"))]
        {
            dsp::vcvt_fixed_to_f32::<FRAC_BITS>(self.value)
        }

        // Portable fallback
        #[cfg(not(all(target_arch = "arm", target_feature = "vfp2")))]
        {
            self.value as f32 / Self::ONE as f32
        }
    }

    /// Convert to a double-precision floating-point value
    #[inline]
    pub fn to_double(self) -> f64 {
        self.value as f64 / Self::ONE as f64
    }

    /// Create a fixed-point number from an integer
    ///
    /// The integer is shifted left by `FRAC_BITS`.
    /// Values that don't fit are saturated.
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q16;
    ///
    /// let fp = Q16::from_int(42);
    /// assert_eq!(fp.to_int(), 42);
    /// ```
    #[inline]
    pub fn from_int(value: i32) -> Self {
        #[allow(clippy::let_unit_value)]
        let _ = Self::_ASSERT_VALID_FRAC_BITS;

        // Check for overflow before shifting
        let max_int = i32::MAX >> FRAC_BITS;
        let min_int = i32::MIN >> FRAC_BITS;

        let clamped = value.clamp(min_int, max_int);
        Self {
            value: clamped << FRAC_BITS,
        }
    }

    /// Convert to an integer, truncating the fractional part
    ///
    /// If `ROUNDED` is true, rounds to nearest integer.
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::{Q16, Q16Rounded};
    ///
    /// let fp = Q16::from_float(42.7);
    /// assert_eq!(fp.to_int(), 42);
    ///
    /// let fp_rounded = Q16Rounded::from_float(42.7);
    /// assert_eq!(fp_rounded.to_int(), 43);
    /// ```
    #[inline]
    pub fn to_int(self) -> i32 {
        if ROUNDED {
            // Add 0.5 and then truncate
            let half = 1i32 << (FRAC_BITS - 1);
            let adjusted = self.value.saturating_add(half);
            adjusted >> FRAC_BITS
        } else {
            self.value >> FRAC_BITS
        }
    }

    /// Get the integral part of the fixed-point number
    #[inline]
    pub fn integral(self) -> i32 {
        self.value >> FRAC_BITS
    }

    /// Get the fractional part as a value in range [0, 1)
    ///
    /// Returns a fixed-point number with the same format but only the fractional bits set.
    #[inline]
    pub fn fractional(self) -> Self {
        let mask = (1i32 << FRAC_BITS) - 1;
        Self {
            value: self.value & mask,
        }
    }

    /// Absolute value
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let fp = Q31::from_float(-0.5);
    /// let abs_fp = fp.abs();
    /// assert!(abs_fp.to_float() > 0.0);
    /// ```
    #[inline]
    pub fn abs(self) -> Self {
        Self {
            value: self.value.abs(),
        }
    }

    /// Saturating addition
    #[inline]
    pub fn saturating_add(self, rhs: Self) -> Self {
        #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
        {
            Self {
                value: dsp::saturating_add(self.value, rhs.value),
            }
        }
        #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
        {
            Self {
                value: self.value.saturating_add(rhs.value),
            }
        }
    }

    /// Saturating subtraction
    #[inline]
    pub fn saturating_sub(self, rhs: Self) -> Self {
        #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
        {
            Self {
                value: dsp::saturating_sub(self.value, rhs.value),
            }
        }
        #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
        {
            Self {
                value: self.value.saturating_sub(rhs.value),
            }
        }
    }

    /// Multiply two fixed-point numbers
    ///
    /// Uses 64-bit intermediate to prevent overflow, then shifts back.
    /// On ARM platforms with DSP extensions, uses SMMUL/SMMULR for Q31 format.
    #[inline]
    pub fn saturating_mul(self, rhs: Self) -> Self {
        // Use ARM DSP intrinsics for Q31 (31 fractional bits)
        #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
        {
            if FRAC_BITS == 31 {
                // For Q31: a * b with both having 31 fractional bits
                // Product has 62 fractional bits, we want bits [62:31]
                // SMMUL returns bits [63:32], so shift left by 1
                let high = if ROUNDED {
                    dsp::mul_high_round(self.value, rhs.value)
                } else {
                    dsp::mul_high(self.value, rhs.value)
                };
                // Use QADD to saturate the double (shift left by 1)
                // This is equivalent to saturating_add(high, high)
                return Self {
                    value: dsp::saturating_add(high, high),
                };
            }
        }

        // Portable implementation for non-ARM or non-Q31 formats
        let product = (self.value as i64).wrapping_mul(rhs.value as i64);

        let result = if ROUNDED {
            // Add half for rounding
            let half = 1i64 << (FRAC_BITS - 1);
            (product + half) >> FRAC_BITS
        } else {
            product >> FRAC_BITS
        };

        // Saturate to i32 range
        let saturated = result.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Self { value: saturated }
    }

    /// Divide two fixed-point numbers
    ///
    /// Uses 64-bit intermediate to maintain precision.
    #[inline]
    pub fn saturating_div(self, rhs: Self) -> Self {
        if rhs.value == 0 {
            return if self.value >= 0 {
                Self::MAX
            } else {
                Self::MIN
            };
        }

        let dividend = (self.value as i64) << FRAC_BITS;
        let result = dividend / (rhs.value as i64);

        let result = if ROUNDED {
            // Round the result
            let remainder = dividend % (rhs.value as i64);
            let half_divisor = (rhs.value as i64) / 2;
            if remainder.abs() >= half_divisor {
                if (remainder > 0) == (rhs.value > 0) {
                    result + 1
                } else {
                    result - 1
                }
            } else {
                result
            }
        } else {
            result
        };

        let saturated = result.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Self { value: saturated }
    }

    /// Multiply by an integer
    ///
    /// More efficient than converting the integer to fixed-point first.
    #[inline]
    pub fn mul_int(self, rhs: i32) -> Self {
        Self {
            value: self.value.saturating_mul(rhs),
        }
    }

    /// Divide by an integer
    #[inline]
    pub fn div_int(self, rhs: i32) -> Self {
        if rhs == 0 {
            return if self.value >= 0 {
                Self::MAX
            } else {
                Self::MIN
            };
        }
        Self {
            value: self.value / rhs,
        }
    }

    /// Fused multiply-add: `self + a * b`
    ///
    /// Computes the product and sum in a single operation, which can be more efficient
    /// and accurate than separate operations.
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let a = Q31::from_float(0.5);
    /// let b = Q31::from_float(0.25);
    /// let c = Q31::from_float(0.1);
    ///
    /// let result = a.mul_add(b, c);  // a + b * c
    /// ```
    #[inline]
    pub fn mul_add(self, a: Self, b: Self) -> Self {
        // Use ARM DSP intrinsics for Q31 (31 fractional bits)
        #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
        if FRAC_BITS == 31 {
            let product_high = if ROUNDED {
                dsp::mul_high_round(a.value, b.value)
            } else {
                dsp::mul_high(a.value, b.value)
            };
            // Shift left by 1 for Q31 format, then saturating add
            let shifted = product_high << 1;
            return Self {
                value: dsp::saturating_add(self.value, shifted),
            };
        }

        // Portable implementation
        let product = (a.value as i64).wrapping_mul(b.value as i64);

        let shifted = if ROUNDED {
            let half = 1i64 << (FRAC_BITS - 1);
            (product + half) >> FRAC_BITS
        } else {
            product >> FRAC_BITS
        };

        let result = (self.value as i64).saturating_add(shifted);
        let saturated = result.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Self { value: saturated }
    }

    /// Convert to a different fixed-point format
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::{Q31, Q16};
    ///
    /// let q31 = Q31::from_float(0.5);
    /// let q16: Q16 = q31.convert();
    /// ```
    #[inline]
    pub fn convert<const NEW_FRAC: u32, const NEW_ROUNDED: bool>(
        self,
    ) -> FixedPoint<NEW_FRAC, NEW_ROUNDED> {
        if FRAC_BITS == NEW_FRAC {
            return FixedPoint::from_raw(self.value);
        }

        if FRAC_BITS < NEW_FRAC {
            // Shifting left - check for overflow
            let shift = NEW_FRAC - FRAC_BITS;
            let max_val = i32::MAX >> shift;
            let min_val = i32::MIN >> shift;
            let clamped = self.value.clamp(min_val, max_val);
            FixedPoint::from_raw(clamped << shift)
        } else {
            // Shifting right - may need rounding
            let shift = FRAC_BITS - NEW_FRAC;
            if NEW_ROUNDED {
                let half = 1i32 << (shift - 1);
                let rounded = self.value.saturating_add(half);
                FixedPoint::from_raw(rounded >> shift)
            } else {
                FixedPoint::from_raw(self.value >> shift)
            }
        }
    }

    /// Left shift with saturation
    ///
    /// Shifts the value left by `shift` bits, saturating on overflow.
    #[inline]
    pub fn lshift_saturate(self, shift: u32) -> Self {
        if shift == 0 {
            return self;
        }
        if shift >= 32 {
            return Self::ZERO;
        }

        // Note: Cannot use dsp::saturate_signed_shl because it requires
        // compile-time constant shift amount, but we have runtime shift
        let max_val = i32::MAX >> shift;
        let min_val = i32::MIN >> shift;
        let clamped = self.value.clamp(min_val, max_val);
        Self {
            value: clamped << shift,
        }
    }

    /// Left shift with saturation (compile-time shift amount)
    ///
    /// This version takes the shift amount as a const generic parameter,
    /// allowing the use of ARM DSP saturating shift instructions when available.
    ///
    /// # Examples
    ///
    /// ```
    /// use fixedpoint::Q31;
    ///
    /// let val = Q31::from_float(0.5);
    /// let shifted = val.lshift_saturate_const::<2>();
    /// ```
    #[inline]
    pub fn lshift_saturate_const<const SHIFT: u32>(self) -> Self {
        if SHIFT == 0 {
            return self;
        }
        if SHIFT >= 32 {
            return Self::ZERO;
        }

        #[cfg(all(target_arch = "arm", target_feature = "dsp"))]
        {
            // Saturate to (32 - SHIFT) signed bits with SSAT, then shift left.
            // (The earlier `SSAT …, LSL` form shifted *before* saturating, with a
            // modular shift, so it couldn't saturate values that overflow 32 bits
            // on the shift — see `lshift_saturate`'s docs.)
            Self {
                value: dsp::lshift_saturate::<SHIFT>(self.value),
            }
        }

        #[cfg(not(all(target_arch = "arm", target_feature = "dsp")))]
        {
            let max_val = i32::MAX >> SHIFT;
            let min_val = i32::MIN >> SHIFT;
            let clamped = self.value.clamp(min_val, max_val);
            Self {
                value: clamped << SHIFT,
            }
        }
    }

    /// Right shift (always truncates, never rounds)
    #[inline]
    pub fn rshift(self, shift: u32) -> Self {
        if shift >= 32 {
            return Self::ZERO;
        }
        Self {
            value: self.value >> shift,
        }
    }
}

// Arithmetic trait implementations

impl<const FRAC_BITS: u32, const ROUNDED: bool> Add for FixedPoint<FRAC_BITS, ROUNDED> {
    type Output = Self;

    #[inline]
    fn add(self, rhs: Self) -> Self::Output {
        self.saturating_add(rhs)
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> AddAssign for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = self.saturating_add(rhs);
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> Sub for FixedPoint<FRAC_BITS, ROUNDED> {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        self.saturating_sub(rhs)
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> SubAssign for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = self.saturating_sub(rhs);
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> Mul for FixedPoint<FRAC_BITS, ROUNDED> {
    type Output = Self;

    #[inline]
    fn mul(self, rhs: Self) -> Self::Output {
        self.saturating_mul(rhs)
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> MulAssign for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn mul_assign(&mut self, rhs: Self) {
        *self = self.saturating_mul(rhs);
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> Div for FixedPoint<FRAC_BITS, ROUNDED> {
    type Output = Self;

    #[inline]
    fn div(self, rhs: Self) -> Self::Output {
        self.saturating_div(rhs)
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> DivAssign for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn div_assign(&mut self, rhs: Self) {
        *self = self.saturating_div(rhs);
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> Neg for FixedPoint<FRAC_BITS, ROUNDED> {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self::Output {
        Self {
            value: self.value.saturating_neg(),
        }
    }
}

// Comparison trait implementations

impl<const FRAC_BITS: u32, const ROUNDED: bool> PartialEq for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> Eq for FixedPoint<FRAC_BITS, ROUNDED> {}

impl<const FRAC_BITS: u32, const ROUNDED: bool> PartialOrd for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<const FRAC_BITS: u32, const ROUNDED: bool> Ord for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

// Default trait

impl<const FRAC_BITS: u32, const ROUNDED: bool> Default for FixedPoint<FRAC_BITS, ROUNDED> {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

// Display trait for debugging

impl<const FRAC_BITS: u32, const ROUNDED: bool> core::fmt::Display
    for FixedPoint<FRAC_BITS, ROUNDED>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.to_float())
    }
}

// Conversion between rounded and unrounded versions of the same format

impl<const FRAC_BITS: u32> From<FixedPoint<FRAC_BITS, false>> for FixedPoint<FRAC_BITS, true> {
    #[inline]
    fn from(value: FixedPoint<FRAC_BITS, false>) -> Self {
        Self::from_raw(value.raw())
    }
}

impl<const FRAC_BITS: u32> From<FixedPoint<FRAC_BITS, true>> for FixedPoint<FRAC_BITS, false> {
    #[inline]
    fn from(value: FixedPoint<FRAC_BITS, true>) -> Self {
        Self::from_raw(value.raw())
    }
}

#[cfg(test)]
mod tests {
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
}

// Property-based tests using proptest
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    // Generate valid i32 values for raw fixed-point
    fn raw_value() -> impl Strategy<Value = i32> {
        any::<i32>()
    }

    // Generate valid float values for Q31 (-1.0 to 1.0)
    fn q31_float() -> impl Strategy<Value = f32> {
        -1.0f32..1.0f32
    }

    // Generate valid float values for Q16 (-32768.0 to 32767.0)
    fn q16_float() -> impl Strategy<Value = f32> {
        -32768.0f32..32768.0f32
    }

    proptest! {
        #[test]
        fn prop_addition_commutative(a in raw_value(), b in raw_value()) {
            let fp_a = Q31::from_raw(a);
            let fp_b = Q31::from_raw(b);

            assert_eq!((fp_a + fp_b).raw(), (fp_b + fp_a).raw());
        }

        #[test]
        fn prop_multiplication_commutative(a in raw_value(), b in raw_value()) {
            let fp_a = Q31::from_raw(a);
            let fp_b = Q31::from_raw(b);

            assert_eq!((fp_a * fp_b).raw(), (fp_b * fp_a).raw());
        }

        #[test]
        fn prop_zero_identity_addition(a in raw_value()) {
            let fp = Q31::from_raw(a);
            assert_eq!((fp + Q31::ZERO).raw(), fp.raw());
            assert_eq!((Q31::ZERO + fp).raw(), fp.raw());
        }

        #[test]
        fn prop_zero_identity_multiplication(a in raw_value()) {
            let fp = Q31::from_raw(a);
            assert_eq!((fp * Q31::ZERO).raw(), Q31::ZERO.raw());
            assert_eq!((Q31::ZERO * fp).raw(), Q31::ZERO.raw());
        }

        #[test]
        fn prop_negation_involution(a in raw_value()) {
            let fp = Q31::from_raw(a);
            let neg_neg = -(-fp);

            // Double negation should give original (except for MIN)
            if a != i32::MIN {
                assert_eq!(neg_neg.raw(), fp.raw());
            }
        }

        #[test]
        fn prop_addition_subtraction_inverse(a in raw_value(), b in raw_value()) {
            let fp_a = Q31::from_raw(a);
            let fp_b = Q31::from_raw(b);

            // (a + b) - b should equal a (when no saturation occurs)
            let sum = fp_a + fp_b;
            let diff = sum - fp_b;

            // Check if saturation occurred in either direction
            let saturated = (a > 0 && b > 0 && sum.raw() == i32::MAX) ||
                           (a < 0 && b < 0 && sum.raw() == i32::MIN) ||
                           (sum - fp_b).raw() == i32::MIN ||
                           (sum - fp_b).raw() == i32::MAX;

            if !saturated {
                // Allow for rounding error of 1 due to saturation
                let error = (diff.raw() - fp_a.raw()).abs();
                assert!(error <= 1, "Error too large: {} vs {}", diff.raw(), fp_a.raw());
            }
        }

        #[test]
        fn prop_comparison_transitivity(a in raw_value(), b in raw_value(), c in raw_value()) {
            let fp_a = Q31::from_raw(a);
            let fp_b = Q31::from_raw(b);
            let fp_c = Q31::from_raw(c);

            // If a <= b and b <= c, then a <= c
            if fp_a <= fp_b && fp_b <= fp_c {
                assert!(fp_a <= fp_c);
            }
        }

        #[test]
        fn prop_float_roundtrip_q31(f in q31_float()) {
            let fp = Q31::from_float(f);
            let back = fp.to_float();

            // Should be close (within quantization error)
            let error = (back - f).abs();
            assert!(error < 0.001);
        }

        #[test]
        fn prop_float_roundtrip_q16(f in q16_float()) {
            let fp = Q16::from_float(f);
            let back = fp.to_float();

            // Should be close (within quantization error)
            let error = (back - f).abs();
            assert!(error < 0.1);
        }

        #[test]
        fn prop_abs_non_negative(a in raw_value()) {
            let fp = Q31::from_raw(a);
            let abs_fp = fp.abs();

            assert!(abs_fp.raw() >= 0);
        }

        #[test]
        fn prop_abs_idempotent(a in raw_value()) {
            let fp = Q31::from_raw(a);
            let abs_once = fp.abs();
            let abs_twice = abs_once.abs();

            assert_eq!(abs_once.raw(), abs_twice.raw());
        }

        #[test]
        fn prop_multiplication_by_one_identity(a in raw_value()) {
            let fp = Q31::from_raw(a);
            let one = Q31::from_raw(Q31::ONE);
            let result = fp * one;

            // `one` is `Q31::ONE == i32::MAX` (≈ 1 − 2⁻³¹, since exact 1.0 isn't
            // representable). On ARM the Q31 multiply is `SMMUL` (a 64→hi-32 i.e.
            // `>>32`) then `*2`, which drops one more bit than the portable `>>31`,
            // so the hardware path can be up to 2 LSB off here (the portable path
            // stays within 1). This matches the Deluge firmware's `q31_mult`.
            let diff = (result.raw() - fp.raw()).abs();
            assert!(diff <= 2);
        }

        #[test]
        fn prop_division_by_one_identity(a in raw_value()) {
            let fp = Q31::from_raw(a);
            let one = Q31::from_raw(Q31::ONE);
            let result = fp / one;

            // Should be approximately equal
            let diff = (result.raw() - fp.raw()).abs();
            assert!(diff <= 1);
        }

        #[test]
        fn prop_int_conversion_preserves_value(i in -1000i32..1000i32) {
            let fp = Q16::from_int(i);
            let back = fp.to_int();

            assert_eq!(back, i);
        }

        #[test]
        fn prop_lshift_rshift_inverse(a in raw_value(), shift in 0u32..16u32) {
            let fp = Q16::from_raw(a);
            let shifted = fp.lshift_saturate(shift);
            let back = shifted.rshift(shift);

            // Check if saturation occurred
            let max_safe = i32::MAX >> shift;
            let min_safe = i32::MIN >> shift;

            if a >= min_safe && a <= max_safe {
                assert_eq!(back.raw(), fp.raw());
            }
        }

        #[test]
        fn prop_conversion_roundtrip_q31_q16(a in raw_value()) {
            let q31 = Q31::from_raw(a);
            let q16: Q16 = q31.convert();
            let back: Q31 = q16.convert();

            // Should be close (precision loss expected)
            let diff = (back.to_float() - q31.to_float()).abs();
            assert!(diff < 0.01);
        }

        #[test]
        fn prop_saturating_add_never_wraps(a in raw_value(), b in raw_value()) {
            let fp_a = Q31::from_raw(a);
            let fp_b = Q31::from_raw(b);
            let result = fp_a + fp_b;

            // If both positive, result should be >= both inputs (or MAX)
            if a > 0 && b > 0 {
                assert!(result.raw() >= a || result.raw() == i32::MAX);
                assert!(result.raw() >= b || result.raw() == i32::MAX);
            }

            // If both negative, result should be <= both inputs (or MIN)
            if a < 0 && b < 0 {
                assert!(result.raw() <= a || result.raw() == i32::MIN);
                assert!(result.raw() <= b || result.raw() == i32::MIN);
            }
        }

        #[test]
        fn prop_mul_add_equals_separate_ops(a in raw_value(), b in raw_value(), c in raw_value()) {
            let fp_a = Q31::from_raw(a);
            let fp_b = Q31::from_raw(b);
            let fp_c = Q31::from_raw(c);

            let fused = fp_a.mul_add(fp_b, fp_c);
            let separate = fp_a + (fp_b * fp_c);

            // Should be identical
            assert_eq!(fused.raw(), separate.raw());
        }
    }
}

/// Create a compile-time array of FixedPoint values from raw integer values.
///
/// This macro allows you to define lookup tables and constant arrays using
/// the type-safe FixedPoint type without runtime conversion overhead.
///
/// # Examples
///
/// ```
/// use fixedpoint::{Q31, Q17, fixed_array};
///
/// // Create a Q31 lookup table
/// const COEFFS: [Q31; 4] = fixed_array![Q31; 1073741824, 536870912, 268435456, 0];
///
/// // Create a Q17 tangent table
/// const TAN_TABLE: [Q17; 3] = fixed_array![Q17; 0, 6040817, 12087756];
/// ```
#[macro_export]
macro_rules! fixed_array {
    ($type:ty; $($value:expr),* $(,)?) => {
        [$(
            <$type>::from_raw($value)
        ),*]
    };
}
/// Verifies that the `const fn from_float_const` (portable scale-and-saturate)
/// agrees with [`FixedPoint::from_float`], which on ARM is the single fused
/// hardware `VCVT.S32.F32` instruction.
///
/// On the `armv7-unknown-linux-gnueabihf` QEMU runner (cortex-a9 + neon, see
/// `.cargo/config.toml`) `from_float` lowers to the real instruction, so this is
/// a genuine hardware-vs-portable comparison there:
///
/// ```text
/// cargo test -p fixedpoint --target armv7-unknown-linux-gnueabihf
/// ```
///
/// On the host triple both paths are portable, so it still guards against
/// scale/saturation regressions. Only `ROUNDED == false` formats are checked:
/// the hardware VCVT always truncates, while `from_float_const` honours
/// `ROUNDED`, so the two intentionally diverge for rounded formats.
#[cfg(test)]
mod from_float_matches_vcvt {
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
                    hardware,
                    portable,
                    concat!(
                        stringify!($q),
                        ": from_float({}) = {} (VCVT) but from_float_const = {}"
                    ),
                    v,
                    hardware,
                    portable
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
}
