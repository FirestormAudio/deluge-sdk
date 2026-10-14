//! Pure arithmetic behind the OSTM Embassy time driver: the tick/µs rational, the
//! 32-bit-to-64-bit wrap extension, and the alarm-arm bound.
//!
//! Kept separate from `crate::time_driver`, which is device-only (it touches OSTM/GIC registers
//! and installs the `embassy_time_driver` impl), so that this arithmetic is always compiled and
//! unit-tested on the host.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::clocks::ClockConfig;

/// OSTM0 ticks per Embassy tick (1 µs), as an exact reduced rational `num / den`.
///
/// Taken from [`ClockConfig::p0_ticks_per_us`]: 21161/640 on the Deluge (P0φ = 33,064,062.5 Hz),
/// which avoids the ~0.2% drift of truncating to 33 ticks/µs. `num` < 2^27 and `den` < 2^21.
/// The all-zero value means "not set yet" (before `time_driver::init`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TickRatio {
    num: u64,
    den: u64,
}

impl TickRatio {
    pub(crate) const fn from_clocks(c: &ClockConfig) -> Self {
        let (num, den) = c.p0_ticks_per_us();
        Self {
            num: num as u64,
            den: den as u64,
        }
    }

    pub(crate) const fn is_set(&self) -> bool {
        self.num != 0
    }

    /// Raw OSTM0 ticks to Embassy µs, `floor(ticks × den / num)`, without overflow for any
    /// `ticks`: with `ticks = q·num + r`, `q·den` never exceeds the result, and
    /// `r·den < num·den < 2^48`. Reads 0 while unset.
    #[inline]
    pub(crate) const fn ticks_to_us(&self, ticks: u64) -> u64 {
        if self.num == 0 {
            return 0;
        }
        let q = ticks / self.num;
        let r = ticks % self.num;
        q * self.den + r * self.den / self.num
    }

    /// Embassy µs to OSTM0 ticks, rounded **up** so a short alarm never fires early:
    /// `ceil(us × num / den)`, saturating at `u64::MAX`. Callers clamp it to the 32-bit counter.
    #[inline]
    pub(crate) const fn us_to_ticks(&self, us: u64) -> u64 {
        if self.num == 0 {
            return 0;
        }
        let q = us / self.den;
        let r = us % self.den;
        q.saturating_mul(self.num)
            .saturating_add((r * self.num).div_ceil(self.den))
    }

    /// Longest OSTM1 alarm this driver will program at once (~100 ms), rather than the counter's
    /// full u32 range (~129.9 s).
    ///
    /// Capping costs no accuracy: a farther deadline is re-armed when the intermediate alarm fires
    /// (`ostm1_alarm_isr` re-reads the queue every time), at the cost of at most ~10 timer
    /// interrupts per second on an idle system. It bounds the damage of any error in the
    /// `at - now` computation: a miscalculated alarm self-heals within 100 ms.
    pub(crate) const fn max_arm_ticks(&self) -> u32 {
        (self.num / self.den) as u32 * 100_000
    }
}

/// A [`TickRatio`] the time driver sets once at init and reads on every `now()`.
pub(crate) struct AtomicTickRatio {
    num: AtomicU32,
    den: AtomicU32,
}

impl AtomicTickRatio {
    pub(crate) const fn unset() -> Self {
        Self {
            num: AtomicU32::new(0),
            den: AtomicU32::new(0),
        }
    }

    pub(crate) fn store(&self, r: TickRatio) {
        // `den` first: a reader that sees a non-zero `num` then sees its `den` (single core;
        // set once, before interrupts are enabled).
        self.den.store(r.den as u32, Ordering::Relaxed);
        self.num.store(r.num as u32, Ordering::Release);
    }

    #[inline]
    pub(crate) fn load(&self) -> TickRatio {
        let num = self.num.load(Ordering::Acquire) as u64;
        let den = self.den.load(Ordering::Relaxed) as u64;
        TickRatio { num, den }
    }
}

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

    use crate::clocks::{ClockConfig, TEST_CLOCKS};

    const DELUGE: TickRatio = TickRatio::from_clocks(&TEST_CLOCKS);
    const CRYSTALS: [u32; 4] = [10_000_000, 12_000_000, 13_225_625, 13_333_333];

    /// Exact `floor(ticks × den / num)` and saturating `ceil(us × num / den)`, in u128.
    fn ref_ticks_to_us(r: TickRatio, ticks: u64) -> u64 {
        (ticks as u128 * r.den as u128 / r.num as u128) as u64
    }
    fn ref_us_to_ticks(r: TickRatio, us: u64) -> u64 {
        (us as u128 * r.num as u128)
            .div_ceil(r.den as u128)
            .min(u64::MAX as u128) as u64
    }

    #[test]
    fn deluge_ratio_is_21161_over_640() {
        assert_eq!((DELUGE.num, DELUGE.den), (21_161, 640));
        assert!(DELUGE.is_set());
    }

    #[test]
    fn max_arm_bounds_a_mis_armed_alarm_to_about_100ms() {
        assert_eq!(DELUGE.max_arm_ticks(), 3_300_000);
        // A full-range arm is u32::MAX ticks = 129.898353 s; the cap must be far below that.
        let full_range_us = DELUGE.ticks_to_us(u32::MAX as u64);
        assert!((129_898_000..=129_899_000).contains(&full_range_us), "got {full_range_us}");
        let capped_us = DELUGE.ticks_to_us(DELUGE.max_arm_ticks() as u64);
        assert!((99_000..=101_000).contains(&capped_us), "got {capped_us}");
        assert!(DELUGE.max_arm_ticks() < u32::MAX / 1000);
    }

    #[test]
    fn ticks_to_us_uses_the_rational_not_33() {
        let us = DELUGE.ticks_to_us(33_064_062);
        assert!((999_999..=1_000_000).contains(&us), "got {us}");
        // A naive /33 would give 1_001_941 µs, a 0.19% error.
        assert!(us < 1_001_000);
    }

    #[test]
    fn us_to_ticks_rounds_up_so_alarms_never_fire_early() {
        assert!(DELUGE.us_to_ticks(1) >= 1);
        assert_eq!(DELUGE.us_to_ticks(640), 21_161);
        assert!(DELUGE.us_to_ticks(641) > 21_161);
        assert_eq!(DELUGE.us_to_ticks(0), 0);
    }

    #[test]
    fn conversions_round_trip_within_one_us() {
        for &us in &[1u64, 1000, 1_000_000, 5_000_000] {
            let back = DELUGE.ticks_to_us(DELUGE.us_to_ticks(us));
            assert!(back >= us && back <= us + 1, "us={us} back={back}");
        }
    }

    #[test]
    fn ticks_to_us_matches_u128_reference_across_crystals() {
        for extal in CRYSTALS {
            let r = TickRatio::from_clocks(&ClockConfig::new(extal));
            for ticks in [0, 1, r.num - 1, r.num, r.num + 1, 1 << 32, 1 << 50, 1 << 62, u64::MAX] {
                assert_eq!(r.ticks_to_us(ticks), ref_ticks_to_us(r, ticks), "extal {extal} ticks {ticks}");
            }
        }
    }

    #[test]
    fn us_to_ticks_matches_u128_reference_across_crystals() {
        for extal in CRYSTALS {
            let r = TickRatio::from_clocks(&ClockConfig::new(extal));
            for us in [0, 1, r.den - 1, r.den, r.den + 1, 1 << 32, 1 << 50, u64::MAX] {
                assert_eq!(r.us_to_ticks(us), ref_us_to_ticks(r, us), "extal {extal} us {us}");
            }
        }
    }

    #[test]
    fn ticks_to_us_survives_u64_max() {
        // The old single-multiply formula overflowed here; the split one must not panic.
        let _ = TickRatio::from_clocks(&ClockConfig::new(13_333_333)).ticks_to_us(u64::MAX);
        let _ = DELUGE.ticks_to_us(u64::MAX);
    }

    #[test]
    fn unset_ratio_is_not_set() {
        assert!(!AtomicTickRatio::unset().load().is_set());
    }

    #[test]
    fn unset_ratio_reads_zero_time() {
        // `Instant::now()` before `time_driver::init` reads 0 instead of dividing by zero.
        assert_eq!(AtomicTickRatio::unset().load().ticks_to_us(123_456), 0);
    }

    #[test]
    fn atomic_ratio_round_trips() {
        let a = AtomicTickRatio::unset();
        a.store(DELUGE);
        assert_eq!(a.load(), DELUGE);
    }
}
