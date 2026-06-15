//! Pure audio sample-format conversion + dither, shared by the (bare-metal-only)
//! [`crate::audio_block`] ring driver.
//!
//! The DMA ring access in `audio_block` is hardware, so that module is
//! `#[cfg(target_os = "none")]`. These conversions are pure scalar math, so they
//! live here (non-gated) and unit-test on the host.

/// i32 MSB-aligned (full 32-bit range) → `[-1.0, 1.0)`.
#[inline]
pub fn i32_to_f32(s: i32) -> f32 {
    s as f32 * (1.0 / 2_147_483_648.0)
}

/// `f32` → i32 MSB-aligned, clamped. Clamp-before-cast avoids the sign-flip a
/// raw cast would give past full scale; `2^31 - 1` avoids overflow at `+1.0`.
#[inline]
pub fn f32_to_i32(x: f32) -> i32 {
    (x.clamp(-1.0, 1.0) * 2_147_483_647.0) as i32
}

/// ±16-LSB (of 24-bit) LFSR dither, mixed into every output sample so sustained
/// silence doesn't trip the codec's ~8192-identical-sample auto-mute. Ported
/// verbatim from the firmware's `dither_sample`. Advances `lfsr` in place.
#[inline]
pub fn dither_sample(lfsr: &mut u32) -> i32 {
    let bit = *lfsr & 1;
    *lfsr >>= 1;
    if bit != 0 {
        *lfsr ^= 0xB400;
    }
    ((*lfsr & 0x1F) as i32 - 0x10) << 8
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn i32_to_f32_endpoints() {
        assert_eq!(i32_to_f32(0), 0.0);
        assert_eq!(i32_to_f32(i32::MIN), -1.0); // -2^31 / 2^31 = -1.0 exactly
        // +full-scale: i32::MAX rounds up to 2^31 in f32, so this lands at ~1.0.
        assert!((i32_to_f32(i32::MAX) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn f32_to_i32_clamps_out_of_range() {
        assert_eq!(f32_to_i32(0.0), 0);
        // Past full scale clamps to ±1.0 first, so it never wraps/sign-flips.
        // (The scaled f32 rounds to ±2^31, then the saturating cast lands at the
        // i32 extremes — the point is no overflow UB / sign flip.)
        assert_eq!(f32_to_i32(2.0), i32::MAX);
        assert_eq!(f32_to_i32(1.0), i32::MAX);
        assert_eq!(f32_to_i32(-2.0), i32::MIN);
        assert!(f32_to_i32(-1.0) <= -2_147_483_647);
    }

    #[test]
    fn round_trip_is_within_one_lsb() {
        for &x in &[-0.75f32, -0.25, 0.0, 0.123, 0.5, 0.999] {
            let back = i32_to_f32(f32_to_i32(x));
            assert!((back - x).abs() < 1e-6, "x={x} back={back}");
        }
    }

    #[test]
    fn dither_is_bounded_and_advances() {
        let mut lfsr = 0xACE1u32;
        let mut seen_change = false;
        let mut prev = lfsr;
        for _ in 0..1000 {
            let d = dither_sample(&mut lfsr);
            // ±16 LSB of 24-bit, shifted left 8 → within [-0x1000, 0xF00].
            assert!((-0x1000..=0x0F00).contains(&d), "dither {d} out of range");
            if lfsr != prev {
                seen_change = true;
            }
            prev = lfsr;
        }
        assert!(seen_change, "LFSR must advance");
    }

    #[test]
    fn dither_lfsr_is_deterministic() {
        let run = || {
            let mut l = 0x1234u32;
            (0..8).map(|_| dither_sample(&mut l)).collect::<std::vec::Vec<_>>()
        };
        assert_eq!(run(), run(), "same seed → same sequence");
    }
}
