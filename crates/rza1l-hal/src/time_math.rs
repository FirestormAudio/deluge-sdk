//! Pure arithmetic behind the OSTM Embassy time driver: the tick/µs rational, the
//! 32-bit-to-64-bit wrap extension, and the alarm-arm bound.
//!
//! Kept separate from `crate::time_driver`, which is device-only (it touches OSTM/GIC registers
//! and installs the `embassy_time_driver` impl), so that this arithmetic is always compiled and
//! unit-tested on the host.

/// OSTM0 ticks per Embassy tick (1 µs), as an exact rational.
///
/// P0φ on the Deluge is 13,225,625 Hz × 30 / 12 = 33,064,062.5 Hz, so
/// ticks-per-µs = 33,064,062.5 / 1e6 = 21,161 / 640 exactly.  Using the
/// rational avoids the ~0.2% drift of truncating to 33 ticks/µs.
/// Effective resolution: 1 OSTM tick ≈ 30 ns, rounded to 1 µs at output.
pub(crate) const OSTM_PER_US_NUM: u64 = 21_161;
pub(crate) const OSTM_PER_US_DEN: u64 = 640;

/// Convert a raw OSTM0 tick count to Embassy µs ticks (`ticks × 640 / 21161`).
/// The multiply overflows u64 only after ~28 years of uptime at 33.064 MHz.
#[inline]
pub(crate) fn ostm_ticks_to_us(ticks: u64) -> u64 {
    ticks * OSTM_PER_US_DEN / OSTM_PER_US_NUM
}

/// Convert an Embassy µs delta to OSTM0 ticks, rounding **up** so a short
/// alarm never fires early (`ceil(delta_us × 21161 / 640)`). Saturating to
/// avoid overflow; callers clamp the result to the OSTM 32-bit counter range.
#[inline]
pub(crate) fn us_to_ostm_ticks(delta_us: u64) -> u64 {
    delta_us
        .saturating_mul(OSTM_PER_US_NUM)
        .div_ceil(OSTM_PER_US_DEN)
}

/// Longest OSTM1 alarm this driver will program at once (~100 ms), rather than the counter's full
/// u32 range (~129.9 s).
///
/// Capping costs no accuracy: a farther deadline is re-armed when the intermediate alarm fires
/// (`ostm1_alarm_isr` re-reads the queue every time), at the cost of at most ~10 timer interrupts
/// per second on an idle system.
///
/// It bounds the damage of any error in the `at - now` computation: instead of stalling every
/// timer-driven task for up to 129.9 s, a miscalculated alarm self-heals within 100 ms. This is
/// defence in depth; the wrap extension ([`extend`]) is what keeps `now()` monotonic.
pub(crate) const MAX_ARM_OSTM_TICKS: u32 =
    (OSTM_PER_US_NUM as u32 / OSTM_PER_US_DEN as u32) * 100_000;

/// Extend a fresh 32-bit `cnt` against the previously published 64-bit `last`.
///
/// `cnt < (last as u32)` means OSTM0 wrapped since `last` was taken, so the epoch advances. Split
/// out as a pure function so the wrap arithmetic is unit-testable on the host, away from the
/// hardware read.
#[inline]
pub(crate) const fn extend(last: u64, cnt: u32) -> u64 {
    let last_cnt = last as u32;
    let epoch = last >> 32;
    let epoch = if cnt < last_cnt { epoch + 1 } else { epoch };
    (epoch << 32) | (cnt as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_advances_the_epoch_exactly_when_cnt_goes_backwards() {
        // Same epoch while CNT climbs.
        assert_eq!(extend(0, 0), 0);
        assert_eq!(extend(10, 20), 20);
        assert_eq!(extend(0xFFFF_FFFE, 0xFFFF_FFFF), 0xFFFF_FFFF);
        // CNT going backwards is a wrap: epoch += 1, low word = the new CNT.
        assert_eq!(extend(0xFFFF_FFFF, 0), 1 << 32);
        assert_eq!(extend(0xFFFF_FFFF, 5), (1 << 32) | 5);
        // A wrap from an arbitrary epoch carries the epoch, not just 0 -> 1.
        assert_eq!(extend((7 << 32) | 0xFFFF_F000, 0x10), (8 << 32) | 0x10);
        // Equal CNT is not a wrap (no spurious epoch bump on a repeated read).
        assert_eq!(extend((3 << 32) | 1234, 1234), (3 << 32) | 1234);
    }

    #[test]
    fn extend_is_monotonic_across_a_full_wrap_sweep() {
        // Walk several epochs in coarse steps and assert the extended clock never goes backwards:
        // a backwards `now()` makes `at - now` an epoch-sized delta, arming the alarm for its
        // maximum (~129.9 s).
        let mut last = 0u64;
        let step = 0x2000_0000u32; // 8 steps per wrap
        for epoch in 0..4u64 {
            for i in 0..8u64 {
                let cnt = (i as u32).wrapping_mul(step);
                let val = extend(last, cnt);
                assert!(val >= last, "went backwards: {last:#x} -> {val:#x}");
                assert_eq!(val >> 32, epoch, "epoch drifted at i={i}");
                last = val;
            }
        }
        // Four epochs of forward progress, no resets.
        assert_eq!(last >> 32, 3);
    }

    #[test]
    fn max_arm_bounds_a_mis_armed_alarm_to_about_100ms() {
        // A full-range arm is u32::MAX ticks = 129.898353 s; the cap must be far below that.
        let full_range_us = ostm_ticks_to_us(u32::MAX as u64);
        assert!(
            (129_898_000..=129_899_000).contains(&full_range_us),
            "full-range arm should be ~129.8984 s, got {full_range_us} us"
        );
        let capped_us = ostm_ticks_to_us(MAX_ARM_OSTM_TICKS as u64);
        assert!(
            (99_000..=101_000).contains(&capped_us),
            "cap should be ~100 ms, got {capped_us} us"
        );
        assert!(MAX_ARM_OSTM_TICKS < u32::MAX / 1000);
    }

    #[test]
    fn tick_ratio_reflects_deluge_p0_clock() {
        // P0phi = 33,064,062.5 Hz => 21161/640 OSTM ticks per microsecond.
        assert_eq!(OSTM_PER_US_NUM, 21_161);
        assert_eq!(OSTM_PER_US_DEN, 640);
        // The rational is 33.0640625 ticks/us — verify to 4 dp.
        let ratio = OSTM_PER_US_NUM as f64 / OSTM_PER_US_DEN as f64;
        assert!((ratio - 33.0640625).abs() < 1e-9);
    }

    #[test]
    fn ticks_to_us_uses_the_rational_not_33() {
        // One second of OSTM ticks = 33,064,062 (floor) -> ~1e6 us.
        let one_second_ticks = 33_064_062u64;
        let us = ostm_ticks_to_us(one_second_ticks);
        // Within 1 us of a million (floor division).
        assert!((999_999..=1_000_000).contains(&us), "got {us}");
        // A naive /33 would give 1_001_941 us — a 0.19% error we must avoid.
        assert!(us < 1_001_000);
    }

    #[test]
    fn us_to_ticks_rounds_up_so_alarms_never_fire_early() {
        // 1 us must round up to at least 1 tick, never 0.
        assert!(us_to_ostm_ticks(1) >= 1);
        // ceil: 640 us = exactly 21161 ticks; 641 us strictly more.
        assert_eq!(us_to_ostm_ticks(640), 21_161);
        assert!(us_to_ostm_ticks(641) > 21_161);
        // 0 us -> 0 ticks.
        assert_eq!(us_to_ostm_ticks(0), 0);
    }

    #[test]
    fn conversions_round_trip_within_one_us() {
        for &us in &[1u64, 1000, 1_000_000, 5_000_000] {
            let ticks = us_to_ostm_ticks(us);
            let back = ostm_ticks_to_us(ticks);
            assert!(back >= us && back <= us + 1, "us={us} back={back}");
        }
    }
}
