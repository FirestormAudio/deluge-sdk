//! The clock out on a host backend, where there is no timer peripheral to drive the gate: one
//! task times the pulse train with [`Timer`]s.
//!
//! The handle's calls drive the edges they imply at once ([`clock_out_start`] raises the gate,
//! [`clock_out_stop`] drops it) and kick the task, which times every later edge from the start
//! instant. [`step`] is the task's decision at each wake-up, a pure function so it is testable
//! without an executor. A new timing ([`clock_out_retime`]) is read at the next rising edge, so it
//! applies at a period boundary.

use core::cell::Cell;
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_deadline};

use super::timing::{ClampWarnings, Clamped, MAX_PERIOD, MIN_PERIOD};

/// One period and pulse width of the train.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Timing {
    pub period: Duration,
    pub width: Duration,
}

impl Timing {
    /// `period` clamped to [`MIN_PERIOD`]..=[`MAX_PERIOD`] and `width` to one tick..=one tick
    /// under the period, and which clamps changed a value.
    pub(crate) fn new(period: Duration, width: Duration) -> (Self, Clamped) {
        let tick = Duration::from_ticks(1);
        let p = period.clamp(MIN_PERIOD, MAX_PERIOD);
        let w = width.clamp(tick, p - tick);
        (
            Self {
                period: p,
                width: w,
            },
            Clamped {
                period: p != period,
                width: w != width,
            },
        )
    }
}

/// The edge the train makes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Rise,
    Fall,
}

/// Where a running train is in its cycle.
pub(crate) struct Soft {
    /// The edge due at [`wake`](Soft::wake).
    next: Edge,
    /// When the next edge is due.
    at: Instant,
    /// The grid time of the period in progress's rise. The rise itself may have been made later;
    /// the next is due on the grid at `rise + current.period`.
    rise: Instant,
    /// The timing of the period in progress, taken at its rising edge.
    current: Timing,
}

impl Soft {
    /// A train whose first rising edge was at `at`.
    pub(crate) fn started(at: Instant, timing: Timing) -> Self {
        Self {
            next: Edge::Fall,
            at: at + timing.width,
            rise: at,
            current: timing,
        }
    }

    /// When the next edge is due.
    pub(crate) fn wake(&self) -> Instant {
        self.at
    }
}

impl Timing {
    /// Half the low time between a pulse and the next rise.
    fn half_low(&self) -> Duration {
        (self.period - self.width) / 2
    }
}

/// Make the edge `state` has due at `now`, and schedule the one after it.
///
/// `timing` is the train's timing now, or `None` once it has stopped. A rising edge takes it as
/// the new period's timing, so a change lands at a boundary. Returns the gate's new level (`true`
/// active), or `None` when the train has stopped: no rise is made, and the gate is left alone.
///
/// Rises are due on a grid anchored at the start (re-anchored at each rise's grid time when the
/// period changes), so the number of ticks never drifts. An edge made late by under a period still
/// keeps the gate's pulses and lows usable, so gear downstream reads each one:
///
/// - A late rise keeps its full width, measured from when it was made, when that still leaves
///   half the nominal low time before the next grid rise. Otherwise it ends half the low time
///   before that rise, but never less than half the width after its own rise: a rise too late
///   even for that keeps half its width, and the next rise waits as after a late fall.
/// - A fall is always made, and the next rise follows at its grid time or half the nominal low
///   time after the fall, whichever is later. A rise that waits this way is late on the grid, so
///   its pulse is shortened as above, and the rise after it is back on the grid.
///
/// A due edge more than a period late (the task stalled, or picked a start up late) is not
/// caught up edge by edge, which would burst near-zero-width pulses that gear downstream counts
/// as ticks. A rise counts as late from its grid time, even when it was waiting for half a low.
/// The lost periods are skipped instead: the gate goes inactive, and the next rise is the first
/// one past `now` on the grid of the period in progress.
pub(crate) fn step(state: &mut Soft, timing: Option<Timing>, now: Instant) -> Option<bool> {
    let period = state.current.period;
    let next_rise = state.rise + period;
    let due = match state.next {
        Edge::Fall => state.at,
        Edge::Rise => next_rise,
    };
    if now
        .checked_duration_since(due)
        .is_some_and(|late| late > period)
    {
        if state.next == Edge::Rise && timing.is_none() {
            return None;
        }
        let skipped = (now - state.rise).as_ticks() / period.as_ticks();
        state.rise += Duration::from_ticks(period.as_ticks() * skipped);
        state.next = Edge::Rise;
        state.at = state.rise + period;
        return Some(false);
    }
    match state.next {
        Edge::Fall => {
            state.next = Edge::Rise;
            state.at = next_rise.max(now + state.current.half_low());
            Some(false)
        }
        Edge::Rise => {
            let timing = timing?;
            state.current = timing;
            state.rise = next_rise;
            state.next = Edge::Fall;
            let after = next_rise + timing.period;
            let half_width = (timing.width / 2).max(Duration::from_ticks(1));
            let fall = (now + timing.width).min(after - timing.half_low());
            state.at = fall.max(now + half_width);
            Some(true)
        }
    }
}

/// What the handle last set, for the task to read.
#[derive(Clone, Copy)]
struct Shared {
    running: bool,
    timing: Timing,
    /// The last start's rising edge.
    started: Instant,
}

static SHARED: Mutex<CriticalSectionRawMutex, Cell<Shared>> = Mutex::new(Cell::new(Shared {
    running: false,
    timing: Timing {
        period: MIN_PERIOD,
        width: Duration::from_ticks(1),
    },
    started: Instant::from_ticks(0),
}));

/// The gate channel the clock drives.
static CHANNEL: AtomicU8 = AtomicU8::new(0);
/// Wakes the task after a start or a stop.
static KICK: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn shared() -> Shared {
    SHARED.lock(Cell::get)
}

fn update(f: impl FnOnce(&mut Shared)) {
    SHARED.lock(|c| {
        let mut s = c.get();
        f(&mut s);
        c.set(s);
    });
}

/// The timing the train runs at now, or `None` while stopped.
fn running_timing() -> Option<Timing> {
    let s = shared();
    s.running.then_some(s.timing)
}

/// `Timing::new`, logging the first period clamp and the first width clamp.
fn clamped_timing(period: Duration, width: Duration) -> Timing {
    static WARNINGS: ClampWarnings = ClampWarnings::new();
    let (timing, clamped) = Timing::new(period, width);
    let warn = WARNINGS.first(clamped);
    if warn.period {
        log::warn!(
            "clock out: period {period:?} is out of range, running at {:?}",
            timing.period
        );
    }
    if warn.width {
        log::warn!(
            "clock out: pulse width {width:?} is out of range, running at {:?}",
            timing.width
        );
    }
    timing
}

fn gate_set(on: bool) {
    crate::plat::gate_set(CHANNEL.load(Ordering::Relaxed), on);
}

/// Bind the clock to gate channel `ch` without starting its task.
pub(crate) fn bind(ch: u8) {
    CHANNEL.store(ch, Ordering::Relaxed);
}

/// Bind the clock to gate channel `ch` and spawn its task. Runs once.
pub(crate) fn clock_out_init(ch: u8, spawner: Spawner) {
    bind(ch);
    spawner.spawn(soft_clock().unwrap());
}

/// Raise the gate now and run the train from here.
pub(crate) fn clock_out_start(period: Duration, width: Duration) {
    let timing = clamped_timing(period, width);
    gate_set(true);
    update(|s| {
        s.running = true;
        s.timing = timing;
        s.started = Instant::now();
    });
    KICK.signal(());
}

/// Run at `period` and `width` from the next period boundary.
pub(crate) fn clock_out_retime(period: Duration, width: Duration) {
    let timing = clamped_timing(period, width);
    update(|s| s.timing = timing);
}

/// Stop the train and drop the gate.
pub(crate) fn clock_out_stop() {
    update(|s| s.running = false);
    gate_set(false);
    KICK.signal(());
}

/// The gate, up while this lives: dropping it ends the pulse.
struct PulseUp;

impl PulseUp {
    fn raise() -> Self {
        gate_set(true);
        Self
    }
}

impl Drop for PulseUp {
    fn drop(&mut self) {
        gate_set(false);
    }
}

/// One pulse of `width`, clamped as the device clamps it. The caller has checked the train is
/// stopped. A pulse dropped before it ends still ends low at once, as the bare-metal one does.
pub(crate) async fn clock_out_pulse(width: Duration) {
    let _up = PulseUp::raise();
    Timer::after(pulse_width(width)).await;
}

/// The timing the train runs, or last ran, at.
#[cfg(all(test, feature = "sim"))]
pub(crate) fn current_timing() -> Timing {
    shared().timing
}

/// A single pulse's width, clamped as the device clamps it: under a period of twice the width,
/// itself clamped to [`MIN_PERIOD`]..=[`MAX_PERIOD`] first, so only a width out of range is
/// logged.
fn pulse_width(width: Duration) -> Duration {
    let period = width
        .checked_mul(2)
        .unwrap_or(MAX_PERIOD)
        .clamp(MIN_PERIOD, MAX_PERIOD);
    clamped_timing(period, width).width
}

/// The train is running.
pub(crate) fn clock_out_running() -> bool {
    shared().running
}

/// The train's task: idle until a start, then make each edge as it falls due.
#[embassy_executor::task]
async fn soft_clock() -> ! {
    let mut state: Option<Soft> = None;
    loop {
        let kicked = match &state {
            None => {
                KICK.wait().await;
                true
            }
            Some(s) => with_deadline(s.wake(), KICK.wait()).await.is_ok(),
        };
        if kicked {
            // A start restarts the train from its rising edge (which it made); a stop idles it.
            let s = shared();
            state = s.running.then(|| Soft::started(s.started, s.timing));
        } else if let Some(s) = state.as_mut() {
            match step(s, running_timing(), Instant::now()) {
                Some(on) => gate_set(on),
                None => state = None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn at(n: u64) -> Instant {
        Instant::from_millis(n)
    }

    fn timing(period: u64, width: u64) -> Timing {
        Timing::new(ms(period), ms(width)).0
    }

    #[test]
    fn a_started_train_falls_after_the_width_and_rises_after_the_period() {
        let mut s = Soft::started(at(100), timing(10, 2));
        assert_eq!(s.wake(), at(102));
        assert_eq!(step(&mut s, Some(timing(10, 2)), at(102)), Some(false));
        assert_eq!(s.wake(), at(110));
        assert_eq!(step(&mut s, Some(timing(10, 2)), at(110)), Some(true));
        assert_eq!(s.wake(), at(112));
    }

    #[test]
    fn set_period_applies_at_the_next_boundary() {
        let mut rises = vec![at(0)];
        let mut s = Soft::started(at(0), timing(10, 2));
        // Mid-pulse, the period moves to 5 ms.
        let new = Some(timing(5, 2));
        for _ in 0..4 {
            let now = s.wake();
            if step(&mut s, new, now) == Some(true) {
                rises.push(now);
            }
        }
        assert_eq!(rises, [at(0), at(10), at(15)]);
    }

    #[test]
    fn a_stopped_train_leaves_the_gate_alone_at_its_next_rise() {
        let mut s = Soft::started(at(0), timing(10, 2));
        assert_eq!(
            step(&mut s, None, at(2)),
            Some(false),
            "the pulse in flight ends"
        );
        assert_eq!(step(&mut s, None, at(10)), None, "no rise once stopped");
    }

    /// The rises a train started at 0 makes up to `until`, when the task makes no edge before
    /// `stalled_until` and then catches up at each wake-up.
    fn rises_after_a_stall(t: Timing, stalled_until: Instant, until: Instant) -> Vec<Instant> {
        let mut rises = vec![];
        let mut s = Soft::started(at(0), t);
        while s.wake() <= until {
            let now = s.wake().max(stalled_until);
            if step(&mut s, Some(t), now) == Some(true) {
                rises.push(now);
            }
        }
        rises
    }

    #[test]
    fn a_stall_of_three_and_a_half_periods_gives_one_rise_on_the_grid() {
        // Due to fall at 2 ms, the task wakes at 37: the edges at 2..=30 are lost, not burst.
        let rises = rises_after_a_stall(timing(10, 2), at(37), at(40));
        assert_eq!(rises, [at(40)]);
    }

    #[test]
    fn a_stall_ends_the_pulse_in_flight_and_rises_on_the_grid() {
        let mut s = Soft::started(at(0), timing(10, 2));
        assert_eq!(step(&mut s, Some(timing(10, 2)), at(37)), Some(false));
        assert_eq!(s.wake(), at(40));
        // Late for a rise, by more than a period: no rise until the grid's next.
        let mut s = Soft::started(at(0), timing(10, 2));
        step(&mut s, Some(timing(10, 2)), at(2));
        assert_eq!(step(&mut s, Some(timing(10, 2)), at(25)), Some(false));
        assert_eq!(s.wake(), at(30));
        assert_eq!(step(&mut s, Some(timing(10, 2)), at(30)), Some(true));
        assert_eq!(s.wake(), at(32));
    }

    #[test]
    fn a_wake_late_by_under_a_period_still_makes_every_edge() {
        // 8 ms late for the fall: the rise due at 10 waits half the low time (4 ms) after it.
        let rises = rises_after_a_stall(timing(10, 2), at(10), at(20));
        assert_eq!(rises, [at(14), at(20)]);
    }

    /// Step `s` at `now`, returning the level made and the next wake-up.
    fn step_at(s: &mut Soft, t: Timing, now: u64) -> (Option<bool>, Instant) {
        let level = step(s, Some(t), at(now));
        (level, s.wake())
    }

    #[test]
    fn edges_on_time_stay_on_the_grid() {
        let t = timing(10, 2);
        let mut s = Soft::started(at(0), t);
        for k in 0..5 {
            assert_eq!(
                step_at(&mut s, t, 10 * k + 2),
                (Some(false), at(10 * k + 10))
            );
            assert_eq!(
                step_at(&mut s, t, 10 * k + 10),
                (Some(true), at(10 * k + 12))
            );
        }
    }

    #[test]
    fn a_rise_late_by_more_than_the_width_keeps_its_width() {
        // A 20 ms period with a 4 ms pulse: the nominal low is 16 ms, half of it 8.
        let t = timing(20, 4);
        let mut s = Soft::started(at(0), t);
        assert_eq!(step_at(&mut s, t, 4), (Some(false), at(20)));
        // 6 ms late: a full pulse to 30 still leaves 10 ms low before the rise at 40.
        assert_eq!(step_at(&mut s, t, 26), (Some(true), at(30)));
        assert_eq!(step_at(&mut s, t, 30), (Some(false), at(40)));
        assert_eq!(step_at(&mut s, t, 40), (Some(true), at(44)));
    }

    #[test]
    fn a_rise_too_late_for_its_full_width_ends_half_the_low_before_the_next_rise() {
        let t = timing(20, 4);
        let mut s = Soft::started(at(0), t);
        step_at(&mut s, t, 4);
        // 9 ms late: a full pulse would end at 33, leaving under 8 ms low before 40.
        assert_eq!(step_at(&mut s, t, 29), (Some(true), at(32)));
        assert_eq!(step_at(&mut s, t, 32), (Some(false), at(40)));
        assert_eq!(step_at(&mut s, t, 40), (Some(true), at(44)));
    }

    #[test]
    fn a_rise_too_late_for_half_its_width_keeps_half_and_the_next_rise_waits() {
        let t = timing(20, 4);
        let mut s = Soft::started(at(0), t);
        step_at(&mut s, t, 4);
        // 15 ms late: past 32, so the pulse keeps half its width, and the rise due at 40 waits
        // half the low time after the fall.
        assert_eq!(step_at(&mut s, t, 35), (Some(true), at(37)));
        assert_eq!(step_at(&mut s, t, 37), (Some(false), at(45)));
        // That rise is 5 ms late, and the train is back on the grid by the next one.
        assert_eq!(step_at(&mut s, t, 45), (Some(true), at(49)));
        assert_eq!(step_at(&mut s, t, 49), (Some(false), at(60)));
    }

    #[test]
    fn a_fall_late_past_the_next_rise_still_falls_and_the_rise_waits_half_the_low() {
        let t = timing(10, 2);
        let mut s = Soft::started(at(0), t);
        // Due at 2, made at 11, past the rise due at 10.
        assert_eq!(step_at(&mut s, t, 11), (Some(false), at(15)));
        // The late rise's pulse ends half the low time before the rise at 20.
        assert_eq!(step_at(&mut s, t, 15), (Some(true), at(16)));
        assert_eq!(step_at(&mut s, t, 16), (Some(false), at(20)));
        assert_eq!(step_at(&mut s, t, 20), (Some(true), at(22)));
    }

    #[test]
    fn a_late_fall_still_leaves_half_the_low() {
        let t = timing(10, 2);
        let mut s = Soft::started(at(0), t);
        // Due at 2, made at 7: the rise at 10 would follow 3 ms after it, under half the low.
        assert_eq!(step_at(&mut s, t, 7), (Some(false), at(11)));
    }

    #[test]
    fn a_waiting_rise_more_than_a_period_past_its_grid_time_is_skipped() {
        let t = timing(10, 2);
        let mut s = Soft::started(at(0), t);
        assert_eq!(step_at(&mut s, t, 11), (Some(false), at(15)));
        // Woken at 21: 6 ms late for the wait, but 11 ms past the grid rise at 10.
        assert_eq!(step_at(&mut s, t, 21), (Some(false), at(30)));
        assert_eq!(step_at(&mut s, t, 30), (Some(true), at(32)));
    }

    #[test]
    fn a_width_at_or_over_the_period_is_clamped_below_it() {
        let (t, clamped) = Timing::new(ms(10), ms(10));
        assert!(clamped.width && !clamped.period);
        assert!(t.width < t.period);
        let (t, clamped) = Timing::new(ms(10), ms(3));
        assert_eq!(clamped, Clamped::default());
        assert_eq!(
            t,
            Timing {
                period: ms(10),
                width: ms(3)
            }
        );
    }

    #[test]
    fn a_period_outside_the_range_is_clamped() {
        let (t, clamped) = Timing::new(Duration::from_micros(10), Duration::from_micros(2));
        assert!(clamped.period && !clamped.width);
        assert_eq!(t.period, ms(1));
    }

    #[test]
    fn a_pulse_width_is_clamped_to_the_range_the_device_uses() {
        assert_eq!(pulse_width(ms(5)), ms(5));
        assert_eq!(
            pulse_width(Duration::from_ticks(0)),
            Duration::from_ticks(1)
        );
        let long = pulse_width(Duration::from_secs(5));
        assert!(long < MAX_PERIOD, "{long:?}");
    }
}
