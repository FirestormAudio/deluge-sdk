//! Analog clock I/O: the trigger-clock input jack and a clock output.
//!
//! [`ClockIn`] wraps the edge-counting driver
//! ([`deluge_bsp::trigger_clock`]) behind the SDK's take-once handle style.
//! [`ClockOut`] has no dedicated jack — it pulses one of the V-trig gate outputs
//! (see [`Gate`](crate::Gate)), so the channel it claims should not also be
//! driven through `Gate`. In bare-metal builds it is timed by MTU2 channel 4
//! (`deluge_bsp::clock_out`); native Linux and the simulator use a software loop.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_time::{Duration, Instant};

#[cfg(not(target_os = "none"))]
pub(crate) mod soft;
pub(crate) mod timing;

// ── Clock input ─────────────────────────────────────────────────────────────

/// Bring up the trigger-clock input's edge-counting IRQ once. No-op on the
/// host simulator (there is no trigger-clock jack).
fn ensure_clock_in_init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    crate::plat::clock_in_init();
}

/// The analog trigger-clock **input** jack.
///
/// Taken once from [`Deluge::clock_in`](crate::Deluge::clock_in). Each external
/// pulse advances an edge counter; [`tick`](ClockIn::tick) awaits the next one
/// and reports the interval since the previous tick (handy for tempo).
///
/// Host: there is no trigger-clock jack in the simulator, so the input never
/// pulses. `tick` waits forever, `count`/`last_edge` report nothing.
pub struct ClockIn {
    /// Embassy-time tick of the previously observed edge, for interval math.
    prev_ticks: Option<u64>,
    _not_send: crate::NotSend,
}

impl ClockIn {
    pub(crate) fn new() -> Self {
        ensure_clock_in_init();
        Self {
            prev_ticks: None,
            _not_send: crate::NOT_SEND,
        }
    }

    /// Await the next external clock pulse, returning the interval since the
    /// previous tick (or `None` on the first tick, when there's no prior edge).
    /// Never arrives on the host simulator.
    pub async fn tick(&mut self) -> Option<Duration> {
        let now = crate::plat::clock_in_wait_edge().await;
        let interval = self
            .prev_ticks
            .map(|p| Duration::from_ticks(now.saturating_sub(p)));
        self.prev_ticks = Some(now);
        interval
    }

    /// Total number of pulses seen since boot (always 0 on the host simulator).
    #[inline]
    pub fn count(&self) -> u32 {
        crate::plat::clock_in_count()
    }

    /// The time of the most recent pulse, or `None` if none has arrived yet
    /// (always `None` on the host simulator).
    #[inline]
    pub fn last_edge(&self) -> Option<Instant> {
        crate::plat::clock_in_last_edge()
    }
}

// ── Clock output ────────────────────────────────────────────────────────────

/// Default high time of an emitted clock pulse.
const DEFAULT_PULSE_WIDTH: Duration = Duration::from_millis(5);
/// The period before any is set: a quarter note at 120 BPM.
const DEFAULT_PERIOD: Duration = Duration::from_millis(500);

/// The clock **output**: a pulse train on one V-trig gate channel.
///
/// Taken once from [`Deluge::clock_out`](crate::Deluge::clock_out), which binds it to a gate
/// channel. [`start`](ClockOut::start) runs a pulse every period until
/// [`stop`](ClockOut::stop), or until the handle is dropped; [`pulse`](ClockOut::pulse) emits one
/// pulse by hand.
///
/// In bare-metal builds, MTU2 channel 4 times the train and its interrupts drive the gate, so the
/// edges don't wait on the executor. Native Linux and the simulator time it with a software loop
/// (a task and `Timer`s).
///
/// ```ignore
/// let mut clk = dlg.clock_out(0);
/// clk.start(ClockOut::period_from_bpm(120.0, 24));
/// // The tempo moved: the next period takes it.
/// clk.set_period(ClockOut::period_from_bpm(128.0, 24));
/// clk.stop();
/// ```
///
/// Periods are clamped to 1 ms..=2 s and the pulse width to under the period; the first clamp is
/// logged at `warn`.
pub struct ClockOut {
    /// The period the train runs at, or will at the next start.
    period: Duration,
    pulse_width: Duration,
    _not_send: crate::NotSend,
}

impl ClockOut {
    pub(crate) fn new(channel: u8, spawner: embassy_executor::Spawner) -> Self {
        // Configure the gate GPIOs (shared one-time bring-up with Cv/Gate).
        crate::cv_gate::ensure_init();
        crate::plat::clock_out_init(channel, spawner);
        Self::handle()
    }

    /// A handle on `channel` with no timing task, for tests, which have no executor.
    #[cfg(all(test, feature = "sim"))]
    fn unspawned(channel: u8) -> Self {
        crate::cv_gate::ensure_init();
        soft::bind(channel);
        Self::handle()
    }

    fn handle() -> Self {
        Self {
            period: DEFAULT_PERIOD,
            pulse_width: DEFAULT_PULSE_WIDTH,
            _not_send: crate::NOT_SEND,
        }
    }

    /// Run a pulse every `period`. The first pulse starts now.
    ///
    /// Starting a running clock begins a new period at once and cuts the current pulse: the new
    /// pulse starts now. In bare-metal builds the gate goes inactive for a few hundred
    /// nanoseconds first.
    pub fn start(&mut self, period: Duration) {
        self.period = period;
        crate::plat::clock_out_start(period, self.pulse_width);
    }

    /// Change the period. A running clock takes it at the next period boundary. A stopped one
    /// doesn't start: the period is only kept, as the one a later
    /// [`set_pulse_width`](ClockOut::set_pulse_width) retimes against once
    /// [`start`](ClockOut::start), which takes its own period, has run.
    pub fn set_period(&mut self, period: Duration) {
        self.period = period;
        if self.is_running() {
            crate::plat::clock_out_retime(period, self.pulse_width);
        }
    }

    /// Stop the clock and drop the gate. A no-op while stopped.
    pub fn stop(&mut self) {
        crate::plat::clock_out_stop();
    }

    /// The clock is running (between [`start`](ClockOut::start) and
    /// [`stop`](ClockOut::stop)).
    pub fn is_running(&self) -> bool {
        crate::plat::clock_out_running()
    }

    /// Set the high time of each pulse (default 5 ms). A running clock takes it at the next
    /// period boundary.
    pub fn set_pulse_width(&mut self, width: Duration) {
        self.pulse_width = width;
        if self.is_running() {
            crate::plat::clock_out_retime(self.period, width);
        }
    }

    /// Emit one pulse: raise the gate for the pulse width, then drop it.
    ///
    /// Returns at once, changing nothing, while the clock runs. A cancelled pulse (the future
    /// dropped before it completes) ends low at once on every backend.
    pub async fn pulse(&mut self) {
        if self.is_running() {
            return;
        }
        crate::plat::clock_out_pulse(self.pulse_width).await;
    }

    /// [`start`](ClockOut::start) at `period`, then never return. The clock runs until it is
    /// stopped or dropped, even if this future is dropped first.
    ///
    /// Pair with [`period_from_bpm`](ClockOut::period_from_bpm) for musical rates.
    pub async fn run(&mut self, period: Duration) -> ! {
        self.start(period);
        core::future::pending().await
    }

    /// The period between pulses for a given tempo and pulses-per-beat
    /// resolution (e.g. `period_from_bpm(120.0, 24)` for 24 ppqn at 120 BPM).
    pub fn period_from_bpm(bpm: f32, pulses_per_beat: u32) -> Duration {
        let beats_per_us = bpm / 60.0 / 1_000_000.0;
        let pulses_per_us = beats_per_us * pulses_per_beat as f32;
        let us = if pulses_per_us > 0.0 {
            (1.0 / pulses_per_us) as u64
        } else {
            0
        };
        Duration::from_micros(us)
    }
}

impl Drop for ClockOut {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(all(test, feature = "sim"))]
mod tests {
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Mutex, OnceLock, PoisonError};

    use embassy_executor::{Executor, Spawner};
    use embassy_futures::select::{Either, select};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_time::{Duration, Instant, Timer};

    use super::ClockOut;
    use super::soft::{self, Timing};

    /// The soft clock is one per process, so these tests take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn install_panel() {
        crate::plat::sim_panel::install(
            deluge_simulator::link::SharedPanel::new(),
            deluge_simulator::link::audio::new_bridge().0,
        );
    }

    /// A clock out on `ch` with the simulator panel installed, without spawning its timing task:
    /// what it does synchronously is what most of these tests see. Once the real-loop test has
    /// spawned the task it stays alive, so a test that reads the gate after a start runs a long
    /// pulse, which the task can't end first.
    fn clock_out(ch: u8) -> ClockOut {
        install_panel();
        ClockOut::unspawned(ch)
    }

    /// Start `clk` with a 1 s pulse every 2 s.
    fn start_long(clk: &mut ClockOut) {
        clk.set_pulse_width(Duration::from_secs(1));
        clk.start(Duration::from_secs(2));
    }

    fn gate(ch: u8) -> bool {
        crate::host::panel().gate_snapshot()[ch as usize]
    }

    #[test]
    fn start_then_stop_leaves_the_gate_low() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let mut clk = clock_out(1);
        start_long(&mut clk);
        assert!(clk.is_running());
        assert!(gate(1), "start is a boundary: the gate rises at once");
        clk.stop();
        assert!(!clk.is_running());
        assert!(!gate(1));
    }

    #[test]
    fn pulse_while_running_is_a_no_op() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let mut clk = clock_out(2);
        start_long(&mut clk);
        // Ready on the first poll: it waits on nothing and changes nothing.
        embassy_futures::block_on(clk.pulse());
        assert!(clk.is_running());
        assert!(gate(2));
        clk.stop();
    }

    #[test]
    fn dropping_a_clock_out_stops_it() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let mut clk = clock_out(3);
        start_long(&mut clk);
        assert!(gate(3));
        drop(clk);
        assert!(!crate::plat::clock_out_running());
        assert!(!gate(3));
    }

    #[test]
    fn set_period_while_stopped_does_not_start_it() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let mut clk = clock_out(0);
        clk.set_period(Duration::from_millis(20));
        assert!(!clk.is_running());
        assert!(!gate(0));
    }

    /// A job for the test executor: spawn a task with its spawner.
    type Job = Box<dyn FnOnce(Spawner) + Send>;

    /// Jobs for the test executor's dispatcher.
    static JOBS: Channel<CriticalSectionRawMutex, Job, 4> = Channel::new();

    /// The test executor's one task of its own: run each job as it arrives.
    #[embassy_executor::task]
    async fn dispatch(spawner: Spawner) -> ! {
        loop {
            JOBS.receive().await(spawner);
        }
    }

    /// Run `spawn` on the test executor, so it can spawn its task: a pulse awaits an embassy
    /// `Timer`, which only an embassy executor's waker can wait on. These tests share one
    /// executor, started on its own thread by the first of them, so the suite leaks one thread.
    /// The tasks are not `Send`, so the job, not a spawner, crosses to the executor's thread.
    fn on_the_test_executor(spawn: impl FnOnce(Spawner) + Send + 'static) {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            std::thread::spawn(|| {
                let executor: &'static mut Executor = Box::leak(Box::new(Executor::new()));
                executor.run(|s| s.spawn(dispatch(s).unwrap()))
            });
        });
        if JOBS.try_send(Box::new(spawn)).is_err() {
            panic!("the test executor's job queue is full");
        }
    }

    /// Start a 500 ms pulse on `clk` and drop it 1 ms in. Returns whether the gate was up at that
    /// moment. The long pulse keeps a slow test machine from ending it first.
    async fn drop_a_pulse_mid_way(clk: &mut ClockOut, ch: u8) -> bool {
        clk.set_pulse_width(Duration::from_millis(500));
        let one_ms_in = async {
            Timer::after_millis(1).await;
            gate(ch)
        };
        match select(clk.pulse(), one_ms_in).await {
            Either::First(()) => panic!("a 500 ms pulse ended within 1 ms"),
            Either::Second(up) => up,
        }
    }

    /// (gate up mid-pulse, gate after the pulse is dropped)
    #[embassy_executor::task]
    async fn a_pulse_dropped_mid_way(done: Sender<(bool, bool)>) {
        let mut clk = clock_out(0);
        let up = drop_a_pulse_mid_way(&mut clk, 0).await;
        let after = gate(0);
        // Dropped before the test hears back, so its stop can't reach the next test's clock.
        drop(clk);
        done.send((up, after)).unwrap();
    }

    #[test]
    fn dropping_a_pulse_mid_way_leaves_the_gate_low() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let (tx, rx) = channel();
        on_the_test_executor(move |s| s.spawn(a_pulse_dropped_mid_way(tx).unwrap()));
        let (up, after) = rx.recv().unwrap();
        assert!(up, "the pulse was up");
        assert!(
            !after,
            "the dropped pulse ends low at once, with the handle still alive"
        );
    }

    /// (gate up mid-pulse, gate after the pulse and then the handle are dropped)
    #[embassy_executor::task]
    async fn a_clock_out_dropped_with_a_pending_pulse(done: Sender<(bool, bool)>) {
        let mut clk = clock_out(1);
        let up = drop_a_pulse_mid_way(&mut clk, 1).await;
        drop(clk);
        done.send((up, gate(1))).unwrap();
    }

    #[test]
    fn dropping_a_clock_out_with_a_pending_pulse_leaves_the_gate_low() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let (tx, rx) = channel();
        on_the_test_executor(move |s| {
            s.spawn(a_clock_out_dropped_with_a_pending_pulse(tx).unwrap())
        });
        let (up, after) = rx.recv().unwrap();
        assert!(up, "the pulse was up");
        assert!(!after);
    }

    #[test]
    fn degenerate_periods_clamp_to_1_ms_and_2_s() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let mut clk = clock_out(2);
        for (period, clamped) in [
            (ClockOut::period_from_bpm(0.0, 24), Duration::from_millis(1)),
            (Duration::from_ticks(0), Duration::from_millis(1)),
            (Duration::from_secs(10), Duration::from_secs(2)),
            (Duration::from_secs(2), Duration::from_secs(2)),
        ] {
            clk.start(period);
            let t: Timing = soft::current_timing();
            assert_eq!(t.period, clamped, "start({period:?})");
            assert!(t.width < t.period, "start({period:?})");
            clk.set_period(period);
            assert_eq!(
                soft::current_timing().period,
                clamped,
                "set_period({period:?})"
            );
        }
        clk.stop();
    }

    /// What a clock run on the real loop did.
    #[derive(Debug)]
    struct RealRun {
        /// The gate was up straight after the start.
        up_at_start: bool,
        /// The first change seen after the start was a fall.
        fell_first: bool,
        rises: u32,
        falls: u32,
        /// The gate was down straight after the stop.
        down_at_stop: bool,
        /// The gate stayed down for 100 ms after the stop.
        stayed_down: bool,
    }

    /// Run a 10 ms clock on gate 3 with its real timing task until it has made three rises and
    /// three falls (or 5 s pass), then stop it and watch the gate for 100 ms.
    #[embassy_executor::task]
    async fn a_clock_on_the_real_loop(spawner: Spawner, done: Sender<RealRun>) {
        install_panel();
        let mut clk = ClockOut::new(3, spawner);
        clk.start(Duration::from_millis(10));
        let up_at_start = gate(3);
        let (mut level, mut first_change) = (up_at_start, None);
        let (mut rises, mut falls) = (0, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while (rises < 3 || falls < 3) && Instant::now() < deadline {
            Timer::after_micros(500).await;
            let now = gate(3);
            if now != level {
                first_change.get_or_insert(now);
                if now {
                    rises += 1
                } else {
                    falls += 1
                }
                level = now;
            }
        }
        clk.stop();
        let down_at_stop = !gate(3);
        let mut stayed_down = true;
        for _ in 0..20 {
            Timer::after_millis(5).await;
            stayed_down &= !gate(3);
        }
        drop(clk);
        done.send(RealRun {
            up_at_start,
            fell_first: first_change == Some(false),
            rises,
            falls,
            down_at_stop,
            stayed_down,
        })
        .unwrap();
    }

    #[test]
    fn the_real_loop_pulses_until_stopped() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let (tx, rx) = channel();
        on_the_test_executor(move |s| s.spawn(a_clock_on_the_real_loop(s, tx).unwrap()));
        let run = rx.recv().unwrap();
        assert!(run.up_at_start, "{run:?}");
        assert!(run.fell_first, "{run:?}");
        assert!(run.rises >= 2 && run.falls >= 2, "{run:?}");
        assert!(run.down_at_stop, "{run:?}");
        assert!(run.stayed_down, "{run:?}");
    }
}
