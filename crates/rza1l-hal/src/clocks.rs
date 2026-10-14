//! The RZ/A1L clock tree, derived from the board's EXTAL crystal.
//!
//! The CPG runs the PLL at ×30, and the peripheral clocks are fixed fractions of it:
//! P1φ = EXTAL × 30 / 6 and P0φ = EXTAL × 30 / 12. The crystal is a board fact, so the HAL
//! takes it as a [`ClockConfig`] from the BSP and derives everything else.
//!
//! P0φ is a half-integer for an odd crystal (13,225,625 Hz gives 33,064,062.5 Hz), so code that
//! needs it exactly (the Embassy time driver) uses [`ClockConfig::p0_ticks_per_us`], not the
//! truncated [`ClockConfig::p0_hz`].
//!
//! The BSP calls [`init`] once, first thing at boot. Drivers then read the clocks with [`get`].

use core::sync::atomic::{AtomicU32, Ordering};

/// The board's clock source. Everything else in the clock tree is derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockConfig {
    /// EXTAL crystal frequency in Hz.
    pub extal_hz: u32,
}

impl ClockConfig {
    /// A clock tree driven by an `extal_hz` crystal.
    pub const fn new(extal_hz: u32) -> Self {
        assert!(extal_hz != 0, "ClockConfig: extal_hz must be non-zero");
        Self { extal_hz }
    }

    /// P1φ in Hz (EXTAL × 30 / 6). Exact.
    pub const fn p1_hz(&self) -> u32 {
        self.extal_hz * 5
    }

    /// P0φ in Hz (EXTAL × 30 / 12), truncated to a whole Hz.
    pub const fn p0_hz(&self) -> u32 {
        (self.extal_hz as u64 * 5 / 2) as u32
    }

    /// P0φ ticks per microsecond as an exact, fully reduced rational `(num, den)`.
    ///
    /// For the Deluge's 13,225,625 Hz crystal this is `(21161, 640)`. `num` < 2^27 and
    /// `den` ≤ 2,000,000 < 2^21 for any crystal the part accepts.
    pub const fn p0_ticks_per_us(&self) -> (u32, u32) {
        let num = self.extal_hz as u64 * 5;
        let den = 2_000_000u64;
        let g = gcd(num, den);
        ((num / g) as u32, (den / g) as u32)
    }
}

/// The clock tree the BSP registered, as its crystal frequency (0 = not registered yet).
struct Store(AtomicU32);

impl Store {
    const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    fn set(&self, config: ClockConfig) {
        self.0.store(config.extal_hz, Ordering::Relaxed);
    }

    fn get(&self) -> Option<ClockConfig> {
        match self.0.load(Ordering::Relaxed) {
            0 => None,
            hz => Some(ClockConfig { extal_hz: hz }),
        }
    }
}

static STORE: Store = Store::new();

/// Register the board's clock tree. Call once at boot, before any clock-dependent driver.
pub fn init(config: ClockConfig) {
    STORE.set(config);
}

/// The registered clock tree.
///
/// # Panics
/// If [`init`] has not been called.
pub fn get() -> ClockConfig {
    STORE
        .get()
        .expect("rza1l_hal::clocks::init was not called before a clock-dependent driver")
}

/// The registered clock tree, or `None` before [`init`].
pub fn try_get() -> Option<ClockConfig> {
    STORE.get()
}

/// The Deluge's crystal, for HAL unit tests only. Every test that registers clocks uses this one
/// value, because the store is global and test threads run in parallel.
#[cfg(test)]
pub(crate) const TEST_CLOCKS: ClockConfig = ClockConfig::new(13_225_625);

const fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn test_crystal_derives_the_values_the_old_constants_had() {
        // The removed `rspi::P1_HZ`, `ostm::OSTM_HZ`/`mtu2::MTU2_P0_HZ`, and time_math ratio.
        assert_eq!(TEST_CLOCKS.p1_hz(), 66_128_125);
        assert_eq!(TEST_CLOCKS.p0_hz(), 33_064_062);
        assert_eq!(TEST_CLOCKS.p0_ticks_per_us(), (21_161, 640));
    }

    #[test]
    fn nominal_crystal_ratio_is_reduced() {
        // The RZ/A1L's nominal 13.33 MHz crystal: 66,666,665 / 2,000,000 reduces by 5.
        let c = ClockConfig::new(13_333_333);
        assert_eq!(c.p0_ticks_per_us(), (13_333_333, 400_000));
        assert_eq!(c.p1_hz(), 66_666_665);
        assert_eq!(c.p0_hz(), 33_333_332);
    }

    #[test]
    fn ratio_is_always_fully_reduced_and_matches_p0() {
        for extal in [10_000_000, 12_000_000, 13_000_000, 13_225_625, 13_333_333] {
            let c = ClockConfig::new(extal);
            let (num, den) = c.p0_ticks_per_us();
            assert_eq!(gcd(num as u64, den as u64), 1, "extal {extal}: not reduced");
            // num/den ticks per µs × 1e6 µs = P0φ, truncated to whole Hz.
            assert_eq!(num as u64 * 1_000_000 / den as u64, c.p0_hz() as u64, "extal {extal}");
        }
    }

    #[test]
    fn store_is_unset_until_set() {
        // A local store: the global one is shared with other tests.
        let store = Store::new();
        assert_eq!(store.get(), None);
        store.set(TEST_CLOCKS);
        assert_eq!(store.get(), Some(TEST_CLOCKS));
    }

    #[test]
    fn init_then_get_round_trips() {
        init(TEST_CLOCKS);
        assert_eq!(get(), TEST_CLOCKS);
        assert_eq!(try_get(), Some(TEST_CLOCKS));
    }
}
