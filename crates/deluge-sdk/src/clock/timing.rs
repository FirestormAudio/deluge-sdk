//! Pulse timing: turn a period and a pulse width into a timer prescaler and
//! compare values.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_time::Duration;

/// Shortest period the calculator produces.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const MIN_PERIOD: Duration = Duration::from_millis(1);
/// Longest period the calculator produces.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const MAX_PERIOD: Duration = Duration::from_secs(2);

/// Prescaler and compare values for one pulse train.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PulseTiming {
    pub prescaler: u16,
    pub period: u32,
    pub width: u32,
}

impl PulseTiming {
    /// The period this runs: `period + 1` states at `clock_hz / prescaler`.
    #[cfg_attr(not(target_os = "none"), allow(dead_code))]
    pub(crate) fn period_time(&self, clock_hz: u32) -> Duration {
        time(u64::from(self.period) + 1, clock_hz, self.prescaler)
    }

    /// The pulse width this runs: `width` ticks at `clock_hz / prescaler`.
    #[cfg_attr(not(target_os = "none"), allow(dead_code))]
    pub(crate) fn width_time(&self, clock_hz: u32) -> Duration {
        time(u64::from(self.width), clock_hz, self.prescaler)
    }
}

/// Which clamps changed a requested timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Clamped {
    /// The period was out of range, or the timer can't hold it.
    pub period: bool,
    /// The width was out of range for the period.
    pub width: bool,
}

/// Warn-once flags for the period clamp and the width clamp. They are kept apart, so a warning
/// about one can't use up the other's.
pub(crate) struct ClampWarnings {
    period: AtomicBool,
    width: AtomicBool,
}

impl ClampWarnings {
    pub(crate) const fn new() -> Self {
        Self {
            period: AtomicBool::new(false),
            width: AtomicBool::new(false),
        }
    }

    /// The clamps in `clamped` not warned about yet. They count as warned from here on.
    pub(crate) fn first(&self, clamped: Clamped) -> Clamped {
        Clamped {
            period: clamped.period && !self.period.swap(true, Ordering::Relaxed),
            width: clamped.width && !self.width.swap(true, Ordering::Relaxed),
        }
    }
}

/// `ticks` at `clock_hz / prescaler` as a time, rounded to the nearest microsecond.
fn time(ticks: u64, clock_hz: u32, prescaler: u16) -> Duration {
    let num = ticks * u64::from(prescaler) * 1_000_000;
    let hz = u64::from(clock_hz);
    Duration::from_micros((num + hz / 2) / hz)
}

/// Ticks of `us` microseconds at `clock_hz / prescaler`, rounded to nearest.
fn ticks(us: u64, clock_hz: u32, prescaler: u16) -> u64 {
    let denom = u64::from(prescaler) * 1_000_000;
    (us * u64::from(clock_hz) + denom / 2) / denom
}

/// The finest prescaler whose counter holds `period`.
///
/// `period` is clamped to [`MIN_PERIOD`]..=[`MAX_PERIOD`] and `width` to
/// 1..=(`PulseTiming::period` - 1) ticks: the longest width is the compare
/// value minus one. A width under half a tick rounds to zero ticks, and the
/// clamp lifts it to one. `period` in the result is the compare-A value: the
/// counter clears on it, so it runs `period + 1` states. It is at least 2, so a
/// one-tick width stays under it even when the period is a tick or two long.
/// `max_count` is the largest compare value the timer holds. If no prescaler
/// holds the period, the coarsest is used and `period` saturates at
/// `max_count`. The returned [`Clamped`] says which of these changed a value.
///
/// `prescalers` must be ascending, finest first: the first that holds the
/// period is taken.
///
/// # Panics
///
/// If `prescalers` is empty. In debug builds, also if `prescalers` is not
/// ascending or `max_count` is under 2.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn pulse_timing(
    period: Duration,
    width: Duration,
    clock_hz: u32,
    prescalers: &[u16],
    max_count: u32,
) -> (PulseTiming, Clamped) {
    debug_assert!(
        prescalers.windows(2).all(|w| w[0] < w[1]),
        "prescalers must be ascending: {prescalers:?}"
    );
    debug_assert!(max_count >= 2, "max_count {max_count} is under 2");
    let clamped_period = period.clamp(MIN_PERIOD, MAX_PERIOD);
    let mut clamped = Clamped {
        period: clamped_period != period,
        width: false,
    };
    let period_us = clamped_period.as_micros();
    // Cap the width at the period before converting, so a huge width cannot
    // overflow the tick arithmetic; the tick clamp below still reports it.
    let width_us = width.min(clamped_period).as_micros();

    let last = *prescalers.last().expect("no prescalers");
    let prescaler = prescalers
        .iter()
        .copied()
        .find(|&p| ticks(period_us, clock_hz, p).saturating_sub(1) <= u64::from(max_count))
        .unwrap_or(last);

    // The counter runs `period + 1` states, so the compare value is one less.
    let wanted_period = ticks(period_us, clock_hz, prescaler).saturating_sub(1);
    let period_ticks = wanted_period.clamp(2, u64::from(max_count).max(2));
    clamped.period |= period_ticks != wanted_period;
    let wanted = ticks(width_us, clock_hz, prescaler);
    let width_ticks = wanted.clamp(1, period_ticks - 1);
    clamped.width = width_ticks != wanted;

    (
        PulseTiming {
            prescaler,
            period: period_ticks as u32,
            width: width_ticks as u32,
        },
        clamped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ClockOut;

    const HZ: u32 = 33_064_062;
    const PRESCALERS: &[u16] = &[1, 4, 16, 64, 256, 1024];

    fn timing(period: Duration, width: Duration) -> (PulseTiming, Clamped) {
        pulse_timing(period, width, HZ, PRESCALERS, 0xFFFF)
    }

    #[test]
    fn the_default_tempo_uses_the_finest_prescaler() {
        let (t, clamped) = timing(Duration::from_micros(20_833), Duration::from_millis(5));
        assert_eq!(t.prescaler, 16);
        // 20_833 us at 33_064_062 / 16 Hz is 43_051.48 ticks: 43_051 states,
        // so compare A is 43_050.
        assert_eq!(t.period, 43_050, "{t:?}");
        // 5 ms is 10_332.52 ticks.
        assert_eq!(t.width, 10_333, "{t:?}");
        assert_eq!(clamped, Clamped::default());
    }

    #[test]
    fn two_seconds_needs_1024() {
        let (t, clamped) = timing(Duration::from_secs(2), Duration::from_millis(5));
        assert_eq!(t.prescaler, 1024);
        // 64_578.25 ticks: 64_578 states.
        assert_eq!(t.period + 1, 64_578, "{t:?}");
        assert_eq!(clamped, Clamped::default());
    }

    #[test]
    fn out_of_range_periods_clamp() {
        let width = Duration::from_micros(500);
        let (min, _) = timing(MIN_PERIOD, width);
        let (max, _) = timing(MAX_PERIOD, width);
        let (t, c) = timing(Duration::from_ticks(0), width);
        assert_eq!((t.prescaler, t.period), (min.prescaler, min.period));
        assert!(c.period);
        let (t, c) = timing(Duration::from_secs(10), width);
        assert_eq!((t.prescaler, t.period), (max.prescaler, max.period));
        assert!(c.period);
        let (t, c) = timing(ClockOut::period_from_bpm(0.0, 24), width);
        assert_eq!((t.prescaler, t.period), (min.prescaler, min.period));
        assert!(c.period);
    }

    #[test]
    fn a_shorter_period_clamps_the_width() {
        let (t, clamped) = timing(Duration::from_millis(2), Duration::from_millis(5));
        assert_eq!(t.width, t.period - 1);
        assert_eq!(
            clamped,
            Clamped {
                period: false,
                width: true
            }
        );
    }

    #[test]
    fn a_width_below_one_tick_is_one_tick() {
        let (t, clamped) = timing(Duration::from_secs(2), Duration::from_micros(1));
        assert_eq!(t.width, 1);
        assert!(clamped.width && !clamped.period);
    }

    #[test]
    fn a_huge_width_clamps_to_the_period() {
        let (t, clamped) = timing(Duration::from_micros(20_833), Duration::MAX);
        assert_eq!(t.width, t.period - 1);
        assert!(clamped.width && !clamped.period);
    }

    #[test]
    fn a_wide_counter_keeps_the_finest_tick() {
        let (t, _) = pulse_timing(
            Duration::from_secs(2),
            Duration::from_millis(5),
            HZ,
            &[1],
            u32::MAX,
        );
        assert_eq!(t.prescaler, 1);
    }

    #[test]
    fn a_period_no_prescaler_holds_saturates_and_is_reported() {
        // 2 s is 64_578 ticks at the coarsest prescaler: over a 255 counter.
        let (t, clamped) = pulse_timing(
            Duration::from_secs(2),
            Duration::from_millis(5),
            HZ,
            PRESCALERS,
            0xFF,
        );
        assert_eq!((t.prescaler, t.period), (1024, 0xFF), "{t:?}");
        assert!(clamped.period);
    }

    #[test]
    fn a_period_of_one_tick_still_leaves_the_width_under_it() {
        // At 1 kHz and 2 kHz, 1 ms is one and two ticks: compare A would be 0 and 1.
        for hz in [1_000, 2_000] {
            let (t, clamped) = pulse_timing(MIN_PERIOD, Duration::from_millis(5), hz, &[1], 0xFFFF);
            assert!(t.width >= 1, "{hz} Hz: {t:?}");
            assert!(t.width < t.period, "{hz} Hz: {t:?}");
            assert!(clamped.period, "{hz} Hz");
        }
    }

    #[test]
    fn a_period_clamp_alone_does_not_report_the_width() {
        let (_, clamped) = timing(Duration::from_secs(10), Duration::from_millis(5));
        assert_eq!(
            clamped,
            Clamped {
                period: true,
                width: false
            }
        );
    }

    #[test]
    fn each_clamp_warns_once_and_neither_uses_up_the_other() {
        let warnings = ClampWarnings::new();
        let period = Clamped {
            period: true,
            width: false,
        };
        let width = Clamped {
            period: false,
            width: true,
        };
        let both = Clamped {
            period: true,
            width: true,
        };
        assert_eq!(warnings.first(Clamped::default()), Clamped::default());
        assert_eq!(warnings.first(period), period);
        assert_eq!(warnings.first(period), Clamped::default());
        assert_eq!(
            warnings.first(both),
            width,
            "the period's warning left the width's"
        );
        assert_eq!(warnings.first(both), Clamped::default());
    }

    #[test]
    fn the_timing_used_converts_back_to_time() {
        let (t, _) = timing(Duration::from_micros(20_833), Duration::from_millis(5));
        // 43_051 states and 10_333 ticks at 33_064_062 / 16 Hz.
        assert_eq!(t.period_time(HZ), Duration::from_micros(20_833));
        assert_eq!(t.width_time(HZ), Duration::from_micros(5_000));
    }
}
