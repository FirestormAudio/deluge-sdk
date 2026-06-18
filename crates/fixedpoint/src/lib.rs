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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod proptests;

#[cfg(test)]
mod from_float_matches_vcvt;
