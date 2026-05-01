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

/// GIC interrupt ID for IRQ6 (= 32 + 6).
const TRIG_GIC_ID: u16 = 38;

/// Same priority as the encoders — well below audio (5) and OLED DMA (13).
const TRIG_PRIORITY: u8 = 14;

/// Number of pulses seen since boot (saturates by wrapping at `u32::MAX`).
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
    unsafe { rza1l_hal::gic::clear_irq_pending(TRIG_IRQ) };
    EDGE_WAKER.wake();
}

/// Initialise the trigger-clock input.
///
/// Must be called before global IRQs are enabled (`cortex_ar::interrupt::enable()`).
///
/// # Safety
/// Writes to GPIO/INTC/GIC MMIO.
pub unsafe fn irq_init() {
    unsafe {
        // Route P1_14 → IRQ6 via PFC alt-function 2.
        rza1l_hal::gpio::set_pin_mux(TRIG_PORT, TRIG_PIN, 2);
        // `set_pin_mux` doesn't enable PIBC; without it, the pin's level isn't
        // visible in PPR.  Not strictly required for the IRQ itself (IRQn taps
        // the pad before the PIBC gate), but cheap insurance and matches the
        // encoder setup pattern.
        rza1l_hal::gpio::enable_input_buffer(TRIG_PORT, TRIG_PIN);

        // Falling edge: the on-board transistor inverts the external clock,
        // so the external rising edge appears as a falling edge on P1_14.
        rza1l_hal::gic::set_irq_falling_edge(TRIG_IRQ);

        rza1l_hal::gic::register(TRIG_GIC_ID, trig_irq_handler);
        rza1l_hal::gic::clear_irq_pending(TRIG_IRQ);
        rza1l_hal::gic::set_priority(TRIG_GIC_ID, TRIG_PRIORITY);
        rza1l_hal::gic::enable(TRIG_GIC_ID);

        info!("trigger_clock: P1_14 → IRQ6 (GIC 38, falling-edge) ready");
    }
}
