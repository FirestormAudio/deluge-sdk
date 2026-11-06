//! Pulse timing: turn a period and a pulse width into a timer prescaler and
//! compare values.

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
/// `max_count`. The returned flag says whether any of this changed a value.
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
) -> (PulseTiming, bool) {
    debug_assert!(
        prescalers.windows(2).all(|w| w[0] < w[1]),
        "prescalers must be ascending: {prescalers:?}"
    );
    debug_assert!(max_count >= 2, "max_count {max_count} is under 2");
    let clamped_period = period.clamp(MIN_PERIOD, MAX_PERIOD);
    let mut clamped = clamped_period != period;
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
    clamped |= period_ticks != wanted_period;
    let wanted = ticks(width_us, clock_hz, prescaler);
    let width_ticks = wanted.clamp(1, period_ticks - 1);
    clamped |= width_ticks != wanted;

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

    fn timing(period: Duration, width: Duration) -> (PulseTiming, bool) {
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
        assert!(!clamped);
    }

    #[test]
    fn two_seconds_needs_1024() {
        let (t, clamped) = timing(Duration::from_secs(2), Duration::from_millis(5));
        assert_eq!(t.prescaler, 1024);
        // 64_578.25 ticks: 64_578 states.
        assert_eq!(t.period + 1, 64_578, "{t:?}");
        assert!(!clamped);
    }

    #[test]
    fn out_of_range_periods_clamp() {
        let width = Duration::from_micros(500);
        let (min, _) = timing(MIN_PERIOD, width);
        let (max, _) = timing(MAX_PERIOD, width);
        let (t, c) = timing(Duration::from_ticks(0), width);
        assert_eq!((t.prescaler, t.period), (min.prescaler, min.period));
        assert!(c);
        let (t, c) = timing(Duration::from_secs(10), width);
        assert_eq!((t.prescaler, t.period), (max.prescaler, max.period));
        assert!(c);
        let (t, c) = timing(ClockOut::period_from_bpm(0.0, 24), width);
        assert_eq!((t.prescaler, t.period), (min.prescaler, min.period));
        assert!(c);
    }

    #[test]
    fn a_shorter_period_clamps_the_width() {
        let (t, clamped) = timing(Duration::from_millis(2), Duration::from_millis(5));
        assert_eq!(t.width, t.period - 1);
        assert!(clamped);
    }

    #[test]
    fn a_width_below_one_tick_is_one_tick() {
        let (t, clamped) = timing(Duration::from_secs(2), Duration::from_micros(1));
        assert_eq!(t.width, 1);
        assert!(clamped);
    }

    #[test]
    fn a_huge_width_clamps_to_the_period() {
        let (t, clamped) = timing(Duration::from_micros(20_833), Duration::MAX);
        assert_eq!(t.width, t.period - 1);
        assert!(clamped);
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
        assert!(clamped);
    }

    #[test]
    fn a_period_of_one_tick_still_leaves_the_width_under_it() {
        // At 1 kHz and 2 kHz, 1 ms is one and two ticks: compare A would be 0 and 1.
        for hz in [1_000, 2_000] {
            let (t, clamped) =
                pulse_timing(MIN_PERIOD, Duration::from_millis(5), hz, &[1], 0xFFFF);
            assert!(t.width >= 1, "{hz} Hz: {t:?}");
            assert!(t.width < t.period, "{hz} Hz: {t:?}");
            assert!(clamped, "{hz} Hz");
        }
    }
}
