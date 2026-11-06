//! The trigger clock's output, timed by MTU2 channel 4.
//!
//! The clock out is a pulse train on a gate jack. MTU2 channel 4 times it: the counter clears on
//! a compare-A match, which starts each period, and a compare-B match inside the period ends the
//! pulse. The two interrupt handlers drive the gate's GPIO, so each edge lands an interrupt
//! latency after its compare, not a task wake-up later.
//!
//! - TGI4A (compare A, a period boundary): the gate goes active, the flag is cleared, then a
//!   timing [`retime`] left pending is applied.
//! - TGI4B (compare B, the pulse's end): the gate goes inactive and the flag is cleared. In
//!   single-shot mode ([`pulse`]) the timer then stops, lets go of the gate and wakes
//!   [`PULSE_DONE`]'s waiter.
//!
//! [`start`] is itself a boundary: it raises the gate and starts the counter from 0.
//!
//! Each handler writes its edge first, as one PSR store through a [`LevelWriter`], and takes no
//! lock to do it: the gate's writer and active level live in one atomic word, and the next timing
//! in another.
//!
//! ## Usage
//! ```rust,no_run
//! use deluge_bsp::{clock_out, cv_gate};
//! use rza1_hal::gpio::Level;
//!
//! // Once at startup, with the Board's MTU2 channel 4 (after `cv_gate::init`):
//! clock_out::init(board.clock_timer);
//!
//! // Gate 0 (V-trig, active low): P0φ/1024, a period of 64578 ticks, a pulse of 163.
//! let gate = cv_gate::gate_writer(0).unwrap();
//! clock_out::start(gate, Level::Low, 1024, 64_578, 163);
//! // The tempo moved: the next period takes the new timing.
//! clock_out::retime(1024, 64_000, 163);
//! clock_out::stop();
//! ```

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, compiler_fence};

use embassy_sync::once_lock::OnceLock;
use embassy_sync::waitqueue::AtomicWaker;
use rza1_hal::Peri;
use rza1_hal::gpio::{Level, LevelWriter};
use rza1_hal::interrupt::{Handler, typelevel};
use rza1_hal::mtu2::PulseTrain;

use crate::board::ClockTimer;

/// The compare A of a single shot ([`pulse`]): the longest period, so compare B, which stops
/// the timer, always comes first.
const SINGLE_SHOT_PERIOD: u16 = 0xFFFF;

/// The timer, once [`init`] has run. The handlers reach it here.
static TIMER: OnceLock<PulseTrain<'static>> = OnceLock::new();

/// The gate the handlers drive: [`GATE_SET`], [`GATE_ACTIVE_HIGH`] and the writer's
/// [`LevelWriter::into_raw`] in the low 24 bits. Zero while nothing runs. Written with the timer
/// masked, before `start` or `pulse` unmasks it.
static GATE: AtomicU32 = AtomicU32::new(0);
/// [`GATE`] holds a writer.
const GATE_SET: u32 = 1 << 31;
/// The gate is active high (else active low).
const GATE_ACTIVE_HIGH: u32 = 1 << 24;
/// The writer's bits in [`GATE`].
const GATE_WRITER: u32 = (1 << 24) - 1;

/// The timing [`retime`] left for the next boundary, packed by [`pack`]; zero when none.
static PENDING: AtomicU64 = AtomicU64::new(0);
/// A packed timing is valid.
const VALID: u64 = 1 << 63;

/// The periodic clock is running.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// A single shot ([`pulse`]) is in flight.
static SINGLE_SHOT: AtomicBool = AtomicBool::new(false);

/// Woken when a single shot ([`pulse`]) ends, or is cut short by [`start`], [`pulse`] or
/// [`stop`].
pub static PULSE_DONE: AtomicWaker = AtomicWaker::new();

/// One timing of the train, as [`PulseTrain::start`] takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Timing {
    prescaler: u16,
    period: u16,
    width: u16,
}

/// `t` in one word: [`VALID`], then the prescaler, the period and the width, 16 bits each.
fn pack(t: Timing) -> u64 {
    VALID | (t.prescaler as u64) << 32 | (t.period as u64) << 16 | t.width as u64
}

/// The timing [`pack`] packed, or `None` for a word without [`VALID`].
fn unpack(word: u64) -> Option<Timing> {
    (word & VALID != 0).then(|| Timing {
        prescaler: (word >> 32) as u16,
        period: (word >> 16) as u16,
        width: word as u16,
    })
}

/// Take the timing waiting in `slot`, leaving none. Acquire pairs with [`retime`]'s Release.
fn take_pending(slot: &AtomicU64) -> Option<Timing> {
    unpack(slot.swap(0, Ordering::Acquire))
}

/// The gate the handlers drive, and its active level.
#[inline(always)]
fn gate() -> Option<(LevelWriter, Level)> {
    unpack_gate(GATE.load(Ordering::Acquire))
}

/// The writer and active level in a [`GATE`] word, or `None` for an empty one.
#[inline(always)]
fn unpack_gate(word: u32) -> Option<(LevelWriter, Level)> {
    if word & GATE_SET == 0 {
        return None;
    }
    // SAFETY: GATE only holds words `set_gate` packed from a live writer, and only while a clock
    // or a single shot runs (`stop` and the single shot's end empty it); the caller of `start` or
    // `pulse` keeps its `Output` for that long.
    let writer = unsafe { LevelWriter::from_raw(word & GATE_WRITER) };
    let active = if word & GATE_ACTIVE_HIGH != 0 {
        Level::High
    } else {
        Level::Low
    };
    Some((writer, active))
}

/// Make `writer`, active at `active`, the gate the handlers drive.
fn set_gate(writer: LevelWriter, active: Level) {
    let raw = writer.into_raw();
    debug_assert_eq!(raw & !GATE_WRITER, 0, "a LevelWriter packs into 24 bits");
    let level = match active {
        Level::High => GATE_ACTIVE_HIGH,
        Level::Low => 0,
    };
    GATE.store(GATE_SET | level | raw, Ordering::Release);
}

/// The level that isn't `level`.
#[inline(always)]
fn opposite(level: Level) -> Level {
    match level {
        Level::High => Level::Low,
        Level::Low => Level::High,
    }
}

/// TGI4A: a period boundary.
struct PeriodHandler;

impl Handler<typelevel::TGI4A> for PeriodHandler {
    unsafe fn on_interrupt() {
        // The edge first: the gate goes active.
        if let Some((writer, active)) = gate() {
            writer.set(active);
        }
        // Bound only by `init`, which fills TIMER first.
        let Some(timer) = TIMER.try_get() else { return };
        timer.clear_a();
        if let Some(t) = take_pending(&PENDING) {
            timer.retime(t.prescaler, t.period, t.width);
        }
    }
}

/// TGI4B: the pulse's end.
struct WidthHandler;

impl Handler<typelevel::TGI4B> for WidthHandler {
    unsafe fn on_interrupt() {
        // The edge first: the gate goes inactive.
        if let Some((writer, active)) = gate() {
            writer.set(opposite(active));
        }
        let Some(timer) = TIMER.try_get() else { return };
        timer.clear_b();
        if SINGLE_SHOT.load(Ordering::Acquire) {
            timer.stop();
            // Let go of the gate: a later `stop` or `start` doesn't touch it again.
            GATE.store(0, Ordering::Release);
            SINGLE_SHOT.store(false, Ordering::Release);
            PULSE_DONE.wake();
        }
    }
}

rza1_hal::bind_interrupts!(struct Irqs {
    TGI4A => PeriodHandler;
    TGI4B => WidthHandler;
});

/// Take MTU2 channel 4 and bind its compare-A and compare-B interrupts. The timer is left
/// stopped and masked until [`start`] or [`pulse`].
///
/// # Panics
/// If called twice. The check comes first, so the running timer is left alone.
pub fn init(timer: Peri<'static, ClockTimer>) {
    assert!(TIMER.try_get().is_none(), "clock_out::init ran twice");
    // Fill TIMER before the bindings can fire: the handlers need it.
    if TIMER.init(PulseTrain::new(timer, Irqs)).is_err() {
        unreachable!("MTU2 channel 4 is owned once");
    }
}

fn timer() -> &'static PulseTrain<'static> {
    TIMER.try_get().expect("clock_out::init hasn't run")
}

/// Panic, at the caller rather than in the compare-A handler, if channel 4 has no /`prescaler`.
#[track_caller]
fn check_prescaler(prescaler: u16) {
    assert!(
        rza1_hal::mtu2::prescalers(4).contains(&prescaler),
        "clock_out: MTU2 channel 4 has no /{prescaler} prescaler (see rza1_hal::mtu2::prescalers)"
    );
}

/// `period` and `width` made a timing whose compare B lands inside the period: a period of at
/// least 2 ticks, and a width of 1 to `period - 1`. A width of 0, or of the period or more,
/// would leave compare B unmatched and the gate stuck active.
fn clamp_timing(period: u16, width: u16) -> (u16, u16) {
    let period = period.max(2);
    (period, width.clamp(1, period - 1))
}

/// Run the clock: at P0φ / `prescaler`, `gate` goes `active` at every boundary of a `period`-tick
/// period and back after `width` ticks.
///
/// The first edge is now: the gate goes active and the counter starts from 0, so this call is a
/// boundary. Compare B ends that first pulse after `width` ticks, and compare A brings the next
/// edge one period later. Whatever ran before (a clock, or a single shot) is stopped first, and
/// a timing [`retime`] left pending is dropped.
///
/// `width` is clamped to 1 to `period - 1`, and `period` to at least 2. The clock drives `gate`
/// until [`stop`] and then lets go of it, so `gate`'s `Output` must outlive the clock.
///
/// # Panics
/// Before [`init`], or if channel 4 has no /`prescaler` (see `rza1_hal::mtu2::prescalers`).
/// Either panic comes before anything changes.
#[track_caller]
pub fn start(gate: LevelWriter, active: Level, prescaler: u16, period: u16, width: u16) {
    let timer = timer();
    check_prescaler(prescaler);
    let (period, width) = clamp_timing(period, width);
    stop();
    PENDING.store(0, Ordering::Relaxed);
    // The timer is masked: the handlers see the new gate once `timer.start` unmasks them.
    set_gate(gate, active);
    RUNNING.store(true, Ordering::Release);
    gate.set(active);
    // The stores above land before the unmask, whatever the codegen.
    compiler_fence(Ordering::SeqCst);
    timer.start(prescaler, period, width);
}

/// Change the running clock's timing at its next period boundary: the compare-A handler applies
/// it. A later call before that boundary replaces it. `period` and `width` are clamped as
/// [`start`] clamps them.
///
/// The handler writes the new compares right after the boundary, while the counter is still
/// near 0, so they take effect within the period that just began. At the running prescaler the
/// counter is not reset (at a new prescaler it restarts from 0, which is safe). If the handler
/// ran so late that the counter had already passed the new compare B, that period's pulse would
/// not end until the next match, and past a new, shorter compare A the counter would run on to
/// 0xFFFF and wrap before the next boundary. The handler's priority
/// (`board::CLOCK_TIMER_PRIORITY`) keeps it prompt.
///
/// # Panics
/// If channel 4 has no /`prescaler`.
#[track_caller]
pub fn retime(prescaler: u16, period: u16, width: u16) {
    check_prescaler(prescaler);
    let (period, width) = clamp_timing(period, width);
    PENDING.store(
        pack(Timing {
            prescaler,
            period,
            width,
        }),
        Ordering::Release,
    );
}

/// Stop the clock or the single shot, drive its gate inactive and let go of it: a later
/// [`stop`], [`start`] or [`pulse`] doesn't touch that pin again. A single shot in flight wakes
/// [`PULSE_DONE`]. With nothing running it writes no pin. Does nothing to the timer before
/// [`init`].
pub fn stop() {
    if let Some(timer) = TIMER.try_get() {
        // Masked first, so no handler raises the gate after it drops.
        timer.stop();
    }
    RUNNING.store(false, Ordering::Release);
    let was_single_shot = SINGLE_SHOT.swap(false, Ordering::AcqRel);
    if let Some((writer, active)) = unpack_gate(GATE.swap(0, Ordering::AcqRel)) {
        writer.set(opposite(active));
    }
    if was_single_shot {
        PULSE_DONE.wake();
    }
}

/// One pulse: `gate` goes `active` now and back after `width` ticks at P0φ / `prescaler`, then
/// the timer stops, lets go of the gate and wakes [`PULSE_DONE`]. Whatever ran before is stopped
/// first.
///
/// `width` is clamped to 1 to 65534. `gate`'s `Output` must outlive the pulse.
///
/// # Panics
/// Before [`init`], or if channel 4 has no /`prescaler`. Either panic comes before anything
/// changes.
#[track_caller]
pub fn pulse(gate: LevelWriter, active: Level, prescaler: u16, width: u16) {
    let timer = timer();
    check_prescaler(prescaler);
    let (_, width) = clamp_timing(SINGLE_SHOT_PERIOD, width);
    stop();
    PENDING.store(0, Ordering::Relaxed);
    set_gate(gate, active);
    SINGLE_SHOT.store(true, Ordering::Release);
    gate.set(active);
    compiler_fence(Ordering::SeqCst);
    timer.start(prescaler, SINGLE_SHOT_PERIOD, width);
}

/// Whether the periodic clock ([`start`]) is running.
pub fn running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// Whether a single shot ([`pulse`]) is still in flight.
pub fn pulse_pending() -> bool {
    SINGLE_SHOT.load(Ordering::Acquire)
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};
    use rza1_hal::gpio::{Level, Output};
    use rza1_hal::interrupt::{Handler, typelevel};
    use rza1_hal::mmio::test;
    use rza1_hal::peripherals::{MTU2_4, P4_0, P6_7};
    use std::sync::{Mutex, MutexGuard, Once};

    /// PSR6, P6_7's set/reset register: bit 23 selects the pin, bit 7 is its level.
    const PSR6: usize = 0xFCFE_3118;
    const P6_7_HIGH: u32 = (1 << 23) | (1 << 7);
    const P6_7_LOW: u32 = 1 << 23;
    /// PSR4, P4_0's.
    const PSR4: usize = 0xFCFE_3110;
    const TSTR: usize = 0xFCFF_0280;
    const CST4: u32 = 0x80;
    const TCR4: usize = 0xFCFF_0201;
    const TSR4: usize = 0xFCFF_022D;
    const TGRA4: usize = 0xFCFF_021C;
    const TGRB4: usize = 0xFCFF_021E;

    /// The module's statics are process-global and the tests run in parallel: each test holds
    /// this, with the timer bound once. (The MMIO log is per thread.)
    fn exclusive() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        static INIT: Once = Once::new();
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        INIT.call_once(|| init(unsafe { MTU2_4::steal() }));
        stop();
        guard
    }

    fn led() -> Output<'static> {
        Output::new(unsafe { P6_7::steal() }, Level::Low)
    }

    fn boundary() {
        unsafe { <PeriodHandler as Handler<typelevel::TGI4A>>::on_interrupt() };
    }

    fn pulse_end() {
        unsafe { <WidthHandler as Handler<typelevel::TGI4B>>::on_interrupt() };
    }

    fn index_of(writes: &[(usize, u32)], f: impl Fn(&(usize, u32)) -> bool) -> usize {
        writes.iter().position(f).expect("the write happened")
    }

    fn is_psr(addr: usize) -> bool {
        (0xFCFE_3100..0xFCFE_3130).contains(&addr)
    }

    #[test]
    fn pending_timing_round_trips() {
        let packed = pack(Timing {
            prescaler: 1024,
            period: 64_578,
            width: 163,
        });
        assert_ne!(packed & VALID, 0, "a packed timing is valid");
        assert_eq!(
            unpack(packed),
            Some(Timing {
                prescaler: 1024,
                period: 64_578,
                width: 163,
            })
        );
        assert_eq!(unpack(0), None, "zero is no timing");
    }

    #[test]
    fn a_pending_timing_is_taken_once() {
        let slot = AtomicU64::new(0);
        let t = Timing {
            prescaler: 64,
            period: 1000,
            width: 10,
        };
        slot.store(pack(t), Ordering::Release);
        assert_eq!(take_pending(&slot), Some(t));
        assert_eq!(take_pending(&slot), None, "taken once");
    }

    #[test]
    fn a_pending_timing_applies_at_the_boundary_once() {
        let _g = exclusive();
        let led = led();
        start(led.level_writer(), Level::High, 1024, 64_578, 163);
        retime(1024, 1000, 10);
        test::reset();
        test::poke8(TCR4, 0b101 | (0b001 << 5)); // running at /1024
        boundary();
        let writes = test::writes();
        assert!(writes.contains(&(TGRA4, 1000)), "{writes:x?}");
        assert!(writes.contains(&(TGRB4, 10)), "{writes:x?}");
        test::reset();
        boundary();
        let writes = test::writes();
        assert!(
            !writes.iter().any(|&(a, _)| a == TGRA4 || a == TGRB4),
            "a second boundary writes no compares: {writes:x?}"
        );
        stop();
    }

    #[test]
    fn the_boundary_raises_the_gate_before_it_clears_the_flag() {
        let _g = exclusive();
        let led = led();
        start(led.level_writer(), Level::High, 1024, 64_578, 163);
        test::reset();
        test::poke8(TSR4, 0b01); // TGFA set
        boundary();
        let writes = test::writes();
        let gate = index_of(&writes, |&w| w == (PSR6, P6_7_HIGH));
        let flag = index_of(&writes, |&(a, _)| a == TSR4);
        assert!(gate < flag, "{writes:x?}");
        assert_eq!(gate, 0, "the edge is the handler's first write");
        stop();
    }

    #[test]
    fn start_raises_the_first_edge_before_the_counter_runs() {
        let _g = exclusive();
        let led = led();
        test::reset();
        start(led.level_writer(), Level::High, 1024, 64_578, 163);
        let writes = test::writes();
        let gate = index_of(&writes, |&w| w == (PSR6, P6_7_HIGH));
        let run = index_of(&writes, |&(a, v)| a == TSTR && v & CST4 != 0);
        assert!(gate < run, "{writes:x?}");
        assert!(running());
        stop();
    }

    #[test]
    fn stop_drops_a_high_gate() {
        let _g = exclusive();
        let led = led();
        test::reset();
        start(led.level_writer(), Level::High, 1024, 64_578, 163);
        boundary();
        assert!(running());
        stop();
        assert!(!running());
        let writes = test::writes();
        assert!(writes.contains(&(PSR6, P6_7_HIGH)), "the gate went high");
        assert_eq!(
            writes.last(),
            Some(&(PSR6, P6_7_LOW)),
            "stop's last write drops the gate"
        );
    }

    #[test]
    fn stop_lets_go_of_the_gate() {
        let _g = exclusive();
        let led = led();
        let other = Output::new(unsafe { P4_0::steal() }, Level::High);
        start(led.level_writer(), Level::High, 1024, 64_578, 163);
        stop();
        test::reset();
        stop();
        assert!(
            !test::writes().iter().any(|&(a, _)| is_psr(a)),
            "a stop with nothing running writes no pin"
        );
        test::reset();
        start(other.level_writer(), Level::Low, 1024, 64_578, 163);
        let writes = test::writes();
        assert!(
            !writes.iter().any(|&(a, _)| a == PSR6),
            "a start on another gate leaves the old one alone: {writes:x?}"
        );
        assert!(writes.iter().any(|&(a, _)| a == PSR4), "{writes:x?}");
        stop();
    }

    #[test]
    fn start_drops_a_stale_pending_timing() {
        let _g = exclusive();
        let led = led();
        retime(1024, 1000, 10);
        start(led.level_writer(), Level::High, 1024, 64_578, 163);
        test::reset();
        test::poke8(TCR4, 0b101 | (0b001 << 5));
        boundary();
        let writes = test::writes();
        assert!(
            !writes.iter().any(|&(a, _)| a == TGRA4 || a == TGRB4),
            "{writes:x?}"
        );
        stop();
    }

    #[test]
    fn a_pulse_raises_the_gate_and_its_end_stops_the_timer() {
        let _g = exclusive();
        let led = led();
        test::reset();
        pulse(led.level_writer(), Level::High, 64, 500);
        let writes = test::writes();
        let gate = index_of(&writes, |&w| w == (PSR6, P6_7_HIGH));
        let run = index_of(&writes, |&(a, v)| a == TSTR && v & CST4 != 0);
        assert!(gate < run, "{writes:x?}");
        assert!(writes.contains(&(TGRA4, 0xFFFF)), "{writes:x?}");
        assert!(writes.contains(&(TGRB4, 500)), "{writes:x?}");
        assert!(pulse_pending());
        assert!(!running());

        test::reset();
        test::poke8(TSTR, CST4 as u8);
        pulse_end();
        let writes = test::writes();
        assert_eq!(writes.first(), Some(&(PSR6, P6_7_LOW)), "the edge first");
        assert_eq!(test::peek8(TSTR) & CST4 as u8, 0, "the counter stopped");
        assert!(!pulse_pending());
        assert_eq!(GATE.load(Ordering::Acquire), 0, "the gate let go");
        test::reset();
        stop();
        assert!(!test::writes().iter().any(|&(a, _)| is_psr(a)));
    }

    #[test]
    fn widths_are_clamped_inside_the_period() {
        let _g = exclusive();
        let led = led();
        for (period, width, want) in [(100, 200, 99), (100, 100, 99), (100, 0, 1)] {
            test::reset();
            start(led.level_writer(), Level::High, 64, period, width);
            assert!(test::writes().contains(&(TGRB4, want)), "{period}/{width}");
        }
        test::reset();
        pulse(led.level_writer(), Level::High, 64, 0xFFFF);
        assert!(test::writes().contains(&(TGRB4, 0xFFFE)));
        test::reset();
        pulse(led.level_writer(), Level::High, 64, 0);
        assert!(test::writes().contains(&(TGRB4, 1)));
        stop();
        retime(64, 100, 0);
        assert_eq!(
            take_pending(&PENDING),
            Some(Timing {
                prescaler: 64,
                period: 100,
                width: 1
            })
        );
    }

    #[test]
    fn a_bad_prescaler_panics_at_the_caller_before_anything_changes() {
        let _g = exclusive();
        let led = led();
        test::reset();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            start(led.level_writer(), Level::High, 8, 100, 10)
        }));
        assert!(r.is_err());
        assert!(!running());
        assert_eq!(GATE.load(Ordering::Acquire), 0);
        assert!(test::writes().is_empty(), "no register or pin was touched");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pulse(led.level_writer(), Level::High, 8, 10)
        }));
        assert!(r.is_err());
        assert!(!pulse_pending());
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| retime(8, 100, 10)));
        assert!(r.is_err());
        assert_eq!(PENDING.load(Ordering::Acquire), 0);
    }
}
