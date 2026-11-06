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
/// value minus one. A width under half a tick rounds up to one tick. The
/// returned flag says whether either clamp changed a value, including that
/// round-up. `period` in the result is the compare-A value: the counter clears
/// on it, so it runs `period + 1` states. `max_count` is the largest compare
/// value the timer holds. If no prescaler holds the period, the coarsest is
/// used and `period` saturates at `max_count`.
///
/// # Panics
///
/// If `prescalers` is empty.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn pulse_timing(
    period: Duration,
    width: Duration,
    clock_hz: u32,
    prescalers: &[u16],
    max_count: u32,
) -> (PulseTiming, bool) {
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
    let period_ticks = ticks(period_us, clock_hz, prescaler)
        .saturating_sub(1)
        .clamp(1, u64::from(max_count));
    let wanted = ticks(width_us, clock_hz, prescaler);
    let width_ticks = wanted.clamp(1, period_ticks.saturating_sub(1).max(1));
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

    fn near(actual: u32, expected: u32) -> bool {
        actual.abs_diff(expected) <= 1
    }

    #[test]
    fn the_default_tempo_uses_the_finest_prescaler() {
        let (t, clamped) = timing(Duration::from_micros(20_833), Duration::from_millis(5));
        assert_eq!(t.prescaler, 16);
        // 20_833 us at 33_064_062 / 16 Hz is 43_051.48 ticks: 43_051 states,
        // so compare A is exactly 43_050.
        assert_eq!(t.period, 43_050, "{t:?}");
        // 5 ms is 10_332.52 ticks; rounding makes +-1 legitimate here.
        assert!(near(t.width, 10_333), "{t:?}");
        assert!(!clamped);
    }

    #[test]
    fn two_seconds_needs_1024() {
        let (t, clamped) = timing(Duration::from_secs(2), Duration::from_millis(5));
        assert_eq!(t.prescaler, 1024);
        // 64_578.25 ticks; rounding makes +-1 legitimate here.
        assert!(near(t.period + 1, 64_578), "{t:?}");
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
}
