//! MIDI/CV gate one-shot timer using MTU2 channel 2.
//!
//! The Deluge uses a hardware timer to schedule precise gate-off (or gate-on)
//! events that must occur at a sub-millisecond offset within an audio render
//! window — too fine-grained for the Embassy `Timer` abstraction, which has
//! ~30 µs resolution and requires executor context.
//!
//! ## Mechanism
//! MTU2 channel 2, clocked at P0/64 ≈ **520.833 kHz** (~1.92 µs per tick), is
//! configured in one-shot compare-match mode.  When the counter reaches TGRA
//! it automatically resets (CCLR = TGRA), fires the `TGI2A` GIC interrupt
//! (ID 150, priority 5), and calls `handler()`, which:
//!
//! 1. Clears the MTU2 compare-match flag (TSR.TGFA).
//! 2. Reads the pending action from `STATE`.
//! 3. Executes the action (e.g. de-assert a gate GPIO).
//! 4. Disarms the timer (stops ch2, masks GIC ID 150).
//!
//! ## Tick frequency
//! P0φ (33.064 MHz) / 64 = **516 625 Hz** → 1 tick ≈ **1.94 µs**.
//! Maximum one-shot delay: 65535 ticks ≈ **126.9 ms**.
//!
//! ## Usage
//! ```rust,no_run
//! use deluge_bsp::midi_gate;
//!
//! // Once at startup, with the Board's MTU2 channel 2:
//! midi_gate::setup(board.gate_timer);
//!
//! // At render time, to de-assert gate 0 in 520 ticks (~1 ms):
//! midi_gate::schedule_gate_off(0, 520);
//!
//! // To cancel a pending shot (e.g. note extended):
//! midi_gate::cancel();
//! ```

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::once_lock::OnceLock;
use rza1l_hal::Peri;
use rza1l_hal::interrupt::{Handler, typelevel};
use rza1l_hal::mtu2::OneShot;

use crate::board::GateTimer;

/// Prescaler: P0 / 64 ≈ 520 kHz.
const PRESCALER: u16 = 64;

/// GIC priority for TGI2A (must be lower-number / higher-priority than USB
/// at 9, to ensure gate events are not delayed behind USB servicing).
const PRIORITY: u8 = 5;

/// Sentinel value meaning "no pending action".
const NO_ACTION: u8 = 0xFF;

/// Gate channel to de-assert when the timer fires (0–3), or `NO_ACTION`.
/// Written by the scheduling side with `Release`; read by the ISR with `Acquire`.
static GATE_CH: AtomicU8 = AtomicU8::new(NO_ACTION);

/// The timer, once [`setup`] has run. The interrupt handler reaches it here.
static TIMER: OnceLock<OneShot<'static>> = OnceLock::new();

/// TGI2A: the shot fired.
struct ShotHandler;

impl Handler<typelevel::TGI2A> for ShotHandler {
    unsafe fn on_interrupt() {
        // Bound only by `setup`, which fills TIMER first.
        let Some(timer) = TIMER.try_get() else { return };
        // 1. Clear the compare-match flag first, so the GIC doesn't re-trigger.
        timer.clear_flag();
        // 2. Take the pending action (Acquire pairs with schedule_gate_off's Release); swapping
        //    NO_ACTION back makes a concurrent cancel or re-schedule race-free.
        let gate_ch = GATE_CH.swap(NO_ACTION, Ordering::Acquire);
        // 3. Do it.
        if gate_ch != NO_ACTION {
            // SAFETY: a gate GPIO, configured by cv_gate::init.
            unsafe { crate::cv_gate::gate_set(gate_ch, false) }; // de-assert: gate OFF
        }
        // 4. Disarm. The GIC mask takes effect after the assembly IRQ stub writes ICCEOIR.
        timer.disarm();
    }
}

rza1l_hal::bind_interrupts!(struct Irqs {
    TGI2A => ShotHandler;
});

/// Configure MTU2 channel 2 for one-shot gate timing. The timer is left stopped; it is started
/// by [`schedule_gate_off`].
pub fn setup(timer: Peri<'static, GateTimer>) {
    // Fill TIMER before the binding can fire: the handler needs it.
    if TIMER.init(OneShot::new(timer, PRESCALER, PRIORITY, Irqs)).is_err() {
        unreachable!("MTU2 channel 2 is owned once");
    }
}

fn timer() -> &'static OneShot<'static> {
    TIMER.try_get().expect("midi_gate::setup hasn't run")
}

/// Schedule a gate-off event `ticks` from now on gate channel `gate_ch`.
///
/// Arms MTU2 ch2 to fire after `ticks` P0/64 ticks (~1.92 µs each).  The
/// ISR will de-assert `gate_ch` when the timer expires.
///
/// - `gate_ch`: hardware gate output to turn off (0–3).
/// - `ticks`: number of ticks (1–65535).  Minimum useful value is a few ticks
///   to allow for ISR latency.
///
/// Calling this again before the timer fires cancels the previous target and
/// re-arms with the new parameters (the ISR will see the updated gate channel).
///
/// # Panics
/// Before [`setup`].
pub fn schedule_gate_off(gate_ch: u8, ticks: u16) {
    // Store the action before arming; pairs with the Acquire in the handler.
    GATE_CH.store(gate_ch, Ordering::Release);
    timer().arm(ticks);
}

/// Cancel a pending gate-off shot without firing.
///
/// If the timer has already fired this is a no-op (the handler already cleared
/// `GATE_CH`).  If the timer is still running it is stopped and disarmed before
/// the ISR can execute the gate action.
///
/// # Panics
/// Before [`setup`].
pub fn cancel() {
    // Disarm first so the ISR cannot begin executing between the store and the disarm.
    timer().disarm();
    GATE_CH.store(NO_ACTION, Ordering::Release);
}

/// Tick frequency of the one-shot timer (Hz): P0φ / 64 = 516,625 Hz.
///
/// Use this to convert a duration to ticks:
/// `ticks = duration_us * TICK_HZ / 1_000_000`
pub const TICK_HZ: u32 = crate::system::CLOCKS.p0_hz() / PRESCALER as u32;
const _: () = assert!(TICK_HZ == 516_625, "MIDI gate tick rate changed");
