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
//!   single-shot mode ([`pulse`]) the timer then stops and [`PULSE_DONE`] wakes its waiter.
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

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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
/// [`LevelWriter::into_raw`] in the low 24 bits. Zero before the first [`start`] or [`pulse`].
/// Written with the timer masked, before `start` unmasks it.
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
    let word = GATE.load(Ordering::Acquire);
    if word & GATE_SET == 0 {
        return None;
    }
    // SAFETY: GATE only holds words `set_gate` packed from a live writer; the caller of `start`
    // or `pulse` keeps its `Output` for as long as the clock runs.
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
/// If called twice.
pub fn init(timer: Peri<'static, ClockTimer>) {
    // Fill TIMER before the bindings can fire: the handlers need it.
    if TIMER.init(PulseTrain::new(timer, Irqs)).is_err() {
        unreachable!("MTU2 channel 4 is owned once");
    }
}

fn timer() -> &'static PulseTrain<'static> {
    TIMER.try_get().expect("clock_out::init hasn't run")
}

/// Run the clock: at P0φ / `prescaler`, `gate` goes `active` at every boundary of a `period`-tick
/// period and back after `width` ticks. The first pulse starts one period from now. Whatever ran
/// before (a clock, or a single shot) is stopped first, and a timing [`retime`] left pending is
/// dropped.
///
/// `gate`'s `Output` must outlive the clock: keep it until [`stop`]. `width` is 1 to
/// `period - 1`.
///
/// # Panics
/// Before [`init`], or if channel 4 has no /`prescaler` (see `rza1_hal::mtu2::prescalers`).
pub fn start(gate: LevelWriter, active: Level, prescaler: u16, period: u16, width: u16) {
    let timer = timer();
    stop();
    PENDING.store(0, Ordering::Relaxed);
    // The timer is masked: the handlers see the new gate once `start` unmasks them.
    set_gate(gate, active);
    RUNNING.store(true, Ordering::Release);
    timer.start(prescaler, period, width);
}

/// Change the running clock's timing at its next period boundary: the compare-A handler applies
/// it. A later call before that boundary replaces it. Takes the same arguments as [`start`].
pub fn retime(prescaler: u16, period: u16, width: u16) {
    PENDING.store(
        pack(Timing {
            prescaler,
            period,
            width,
        }),
        Ordering::Release,
    );
}

/// Stop the clock or the single shot, then drive the gate inactive. A single shot in flight
/// wakes [`PULSE_DONE`]. Does nothing to the timer before [`init`].
pub fn stop() {
    if let Some(timer) = TIMER.try_get() {
        // Masked first, so no handler raises the gate after it drops.
        timer.stop();
    }
    RUNNING.store(false, Ordering::Release);
    let was_single_shot = SINGLE_SHOT.swap(false, Ordering::AcqRel);
    if let Some((writer, active)) = gate() {
        writer.set(opposite(active));
    }
    if was_single_shot {
        PULSE_DONE.wake();
    }
}

/// One pulse: `gate` goes `active` now and back after `width` ticks at P0φ / `prescaler`, then
/// the timer stops and [`PULSE_DONE`] wakes. Whatever ran before is stopped first.
///
/// `gate`'s `Output` must outlive the pulse. `width` is 1 to 65534.
///
/// # Panics
/// Before [`init`], or if channel 4 has no /`prescaler`.
pub fn pulse(gate: LevelWriter, active: Level, prescaler: u16, width: u16) {
    let timer = timer();
    stop();
    PENDING.store(0, Ordering::Relaxed);
    set_gate(gate, active);
    SINGLE_SHOT.store(true, Ordering::Release);
    gate.set(active);
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
    use rza1_hal::peripherals::{MTU2_4, P6_7};

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
    fn a_pending_timing_applies_at_the_boundary_once() {
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
    fn stop_drops_a_high_gate() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| init(unsafe { MTU2_4::steal() }));
        let led = Output::new(unsafe { P6_7::steal() }, Level::Low);
        test::reset();
        start(led.level_writer(), Level::High, 1024, 64_578, 163);
        // A boundary: the compare-A handler raises the gate.
        unsafe { <PeriodHandler as Handler<typelevel::TGI4A>>::on_interrupt() };
        assert!(running());
        stop();
        assert!(!running());
        // PSR6 is 0xFCFE_3118; bit 7's select is bit 23, its level bit 7.
        let writes = test::writes();
        assert!(
            writes.contains(&(0xFCFE_3118, (1 << 23) | (1 << 7))),
            "the boundary raised the gate"
        );
        assert_eq!(
            writes.last(),
            Some(&(0xFCFE_3118, 1 << 23)),
            "stop's last write drops the gate"
        );
    }
}
