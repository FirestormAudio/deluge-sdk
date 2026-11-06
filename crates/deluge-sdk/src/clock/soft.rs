//! The clock out on a host backend, where there is no timer peripheral to drive the gate: one
//! task times the pulse train with [`Timer`]s.
//!
//! The handle's calls drive the edges they imply at once ([`clock_out_start`] raises the gate,
//! [`clock_out_stop`] drops it) and kick the task, which times every later edge from the start
//! instant. [`step`] is the task's decision at each wake-up, a pure function so it is testable
//! without an executor. A new timing ([`clock_out_retime`]) is read at the next rising edge, so it
//! applies at a period boundary.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_deadline};

use super::timing::{MAX_PERIOD, MIN_PERIOD};

/// One period and pulse width of the train.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Timing {
    pub period: Duration,
    pub width: Duration,
}

impl Timing {
    /// `period` clamped to [`MIN_PERIOD`]..=[`MAX_PERIOD`] and `width` to one tick..=one tick
    /// under the period, and whether either clamp changed a value.
    pub(crate) fn new(period: Duration, width: Duration) -> (Self, bool) {
        let tick = Duration::from_ticks(1);
        let p = period.clamp(MIN_PERIOD, MAX_PERIOD);
        let w = width.clamp(tick, p - tick);
        (
            Self {
                period: p,
                width: w,
            },
            p != period || w != width,
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
    /// The timing of the period in progress, taken at its rising edge.
    current: Timing,
}

impl Soft {
    /// A train whose first rising edge was at `at`.
    pub(crate) fn started(at: Instant, timing: Timing) -> Self {
        Self {
            next: Edge::Fall,
            at: at + timing.width,
            current: timing,
        }
    }

    /// When the next edge is due.
    pub(crate) fn wake(&self) -> Instant {
        self.at
    }
}

/// Make the edge `state` has due, and schedule the one after it.
///
/// `timing` is the train's timing now, or `None` once it has stopped. A rising edge takes it as
/// the new period's timing, so a change lands at a boundary. Returns the gate's new level (`true`
/// active), or `None` when the train has stopped: no rise is made, and the gate is left alone.
pub(crate) fn step(state: &mut Soft, timing: Option<Timing>) -> Option<bool> {
    match state.next {
        Edge::Fall => {
            state.next = Edge::Rise;
            state.at += state.current.period - state.current.width;
            Some(false)
        }
        Edge::Rise => {
            let timing = timing?;
            state.current = timing;
            state.next = Edge::Fall;
            state.at += timing.width;
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

/// `Timing::new`, logging the first clamp.
fn clamped_timing(period: Duration, width: Duration) -> Timing {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let (timing, clamped) = Timing::new(period, width);
    if clamped && !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!(
            "clock out: period {period:?} / width {width:?} is out of range, running at {:?} / {:?}",
            timing.period,
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

/// One pulse of `width`. The caller has checked the train is stopped.
pub(crate) async fn clock_out_pulse(width: Duration) {
    gate_set(true);
    Timer::after(width).await;
    gate_set(false);
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
            match step(s, running_timing()) {
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
        assert_eq!(step(&mut s, Some(timing(10, 2))), Some(false));
        assert_eq!(s.wake(), at(110));
        assert_eq!(step(&mut s, Some(timing(10, 2))), Some(true));
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
            if step(&mut s, new) == Some(true) {
                rises.push(now);
            }
        }
        assert_eq!(rises, [at(0), at(10), at(15)]);
    }

    #[test]
    fn a_stopped_train_leaves_the_gate_alone_at_its_next_rise() {
        let mut s = Soft::started(at(0), timing(10, 2));
        assert_eq!(step(&mut s, None), Some(false), "the pulse in flight ends");
        assert_eq!(step(&mut s, None), None, "no rise once stopped");
    }

    #[test]
    fn a_width_at_or_over_the_period_is_clamped_below_it() {
        let (t, clamped) = Timing::new(ms(10), ms(10));
        assert!(clamped);
        assert!(t.width < t.period);
        let (t, clamped) = Timing::new(ms(10), ms(3));
        assert!(!clamped);
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
        assert!(clamped);
        assert_eq!(t.period, ms(1));
    }
}
