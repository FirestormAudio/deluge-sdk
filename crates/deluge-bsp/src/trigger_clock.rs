//! Deluge analog "trigger clock" input.
//!
//! ## Hardware
//! A 3.3–12 V clock signal arrives at the rear-panel jack and passes through
//! a transistor that **inverts** the signal before it reaches `P1_14`.  The
//! interesting transition (the rising edge of the external pulse) therefore
//! appears as a **falling edge** on `P1_14`.  `P1_14` is routed to RZ/A1L
//! external interrupt **IRQ6** (GIC ID 38) via PFC alt-function 2.
//!
//! ## Provided to consumers
//! - [`EDGE_COUNT`]: monotonically-incrementing 32-bit counter, bumped once
//!   per detected pulse.  Callers can compare against a saved value to
//!   discover how many pulses arrived since the previous check.
//! - [`LAST_EDGE_TICKS`]: Embassy-time tick of the most recent pulse, useful
//!   for measuring the interval between consecutive pulses.
//! - [`EDGE_WAKER`]: an `AtomicWaker` woken from the ISR; an async task can
//!   register itself, await, and be polled when a pulse arrives.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::Instant;
use log::info;

/// Port and pin for the trigger-clock input.
const TRIG_PORT: u8 = 1;
const TRIG_PIN: u8 = 14;

/// External interrupt number used by `P1_14`'s PFC alt-2 routing.
const TRIG_IRQ: u8 = 6;

/// Number of pulses seen since boot (wraps at `u32::MAX`).
pub static EDGE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Embassy-time tick of the most recent pulse (`Instant::now().as_ticks()`).
/// `0` means "no pulse received yet".
pub static LAST_EDGE_TICKS: AtomicU64 = AtomicU64::new(0);

/// Woken from the ISR each time a pulse arrives.  Register a single consumer.
pub static EDGE_WAKER: AtomicWaker = AtomicWaker::new();

fn trig_irq_handler() {
    let now = Instant::now().as_ticks();
    LAST_EDGE_TICKS.store(now, Ordering::Relaxed);
    EDGE_COUNT.fetch_add(1, Ordering::Relaxed);
    unsafe { rza1_hal::gic::clear_irq_pending(TRIG_IRQ) };
    EDGE_WAKER.wake();
}

/// P1_14's IRQ6: one trigger-clock edge.
#[cfg(target_os = "none")]
struct TriggerHandler;

#[cfg(target_os = "none")]
impl rza1_hal::interrupt::Handler<rza1_hal::interrupt::typelevel::IRQ6> for TriggerHandler {
    unsafe fn on_interrupt() {
        trig_irq_handler();
    }
}

#[cfg(target_os = "none")]
rza1_hal::bind_interrupts!(struct TriggerIrqs {
    IRQ6 => TriggerHandler;
});

/// Initialise the trigger-clock input.
///
/// Must be called before global IRQs are enabled (`cortex_ar::interrupt::enable()`).
///
/// # Safety
/// Writes to GPIO/INTC/GIC MMIO.
pub unsafe fn irq_init(pin: rza1_hal::Peri<'static, rza1_hal::peripherals::P1_14>) {
    unsafe {
        // Route P1_14 → IRQ6 via PFC alt-function 2.
        let mut pin = rza1_hal::gpio::Flex::new(pin);
        pin.set_as_alternate(rza1_hal::gpio::Alternate::Alt2);
        // An alternative function doesn't enable PIBC, without which the pin's level
        // isn't visible in PPR. The IRQ itself doesn't need it (IRQn taps the
        // pad before the PIBC gate); enabled so the level stays readable, as
        // in the encoder setup.
        pin.enable_input_buffer();

        // Falling edge: the on-board transistor inverts the external clock,
        // so the external rising edge appears as a falling edge on P1_14.
        rza1_hal::gic::set_irq_falling_edge(TRIG_IRQ);

        rza1_hal::interrupt::bind(TriggerIrqs);
        rza1_hal::gic::clear_irq_pending(TRIG_IRQ);
        // Its priority is the board's TRIGGER_PRIORITY, set by `Board::new`.
        use rza1_hal::interrupt::{Interrupt, typelevel::IRQ6};
        IRQ6::enable();

        info!("trigger_clock: P1_14 → IRQ6 (GIC 38, falling-edge) ready");
    }
}
