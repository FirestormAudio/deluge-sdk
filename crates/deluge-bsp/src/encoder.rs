//! Front-panel rotary encoders: two-pin quadrature decoding into per-encoder edge accumulators.
//!
//! Each encoder's A pin raises an interrupt on both edges; its B pin has none, so MTU2 channel 3
//! interrupts every millisecond to read every encoder's pins and catch B-only transitions. Both feed
//! the encoder's [`QuadratureDecoder`], which counts four edges per cycle and rejects single-pin
//! glitches (see [`crate::encoder_quadrature`]).

use core::sync::atomic::{AtomicI8, Ordering};

use embassy_sync::waitqueue::AtomicWaker;
#[cfg(target_os = "none")]
use log::info;

#[cfg(target_os = "none")]
use crate::encoder_quadrature::QuadratureDecoder;

pub const NUM_ENCODERS: usize = 6;

/// Per-encoder signed quadrature-edge accumulators, four edges per cycle, written by the encoder
/// and poll interrupts. They saturate rather than wrap: a wrap would read as a turn the other way.
#[allow(clippy::declare_interior_mutable_const)]
pub static ENCODER_DELTAS: [AtomicI8; NUM_ENCODERS] = [
    AtomicI8::new(0),
    AtomicI8::new(0),
    AtomicI8::new(0),
    AtomicI8::new(0),
    AtomicI8::new(0),
    AtomicI8::new(0),
];

/// Wakes the firmware encoder task whenever any encoder delta becomes non-zero.
pub static ENCODER_WAKER: AtomicWaker = AtomicWaker::new();

/// Drain the accumulated edge delta for one encoder and convert it into whole detents.
#[inline]
pub fn take_detents(encoder_index: usize, edge_accumulator: &mut i8) -> i8 {
    let delta = ENCODER_DELTAS[encoder_index].swap(0, Ordering::Relaxed);
    // Pure edge→detent accumulation lives in `crate::encoder_detent` (host-tested).
    crate::encoder_detent::accumulate_detents(delta, edge_accumulator)
}

/// Add `edges` to encoder `index`'s accumulator, saturating, and wake the consumer.
fn add_edges(index: usize, edges: i8) {
    if edges == 0 {
        return;
    }
    let _ = ENCODER_DELTAS[index].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_add(edges))
    });
    ENCODER_WAKER.wake();
}

/// How one encoder is wired, in [`ENCODER_DELTAS`] order. All pins are on port 1.
#[cfg(target_os = "none")]
struct Wiring {
    /// The A pin, which raises the interrupt.
    irq_pin: u8,
    /// The B pin.
    comp_pin: u8,
    gic_id: u16,
    irq_num: u8,
    /// A and B are swapped relative to the others, so the direction is reversed.
    invert: bool,
}

#[cfg(target_os = "none")]
const WIRING: [Wiring; NUM_ENCODERS] = [
    Wiring {
        irq_pin: 11,
        comp_pin: 12,
        gic_id: 35,
        irq_num: 3,
        invert: false,
    },
    Wiring {
        irq_pin: 6,
        comp_pin: 7,
        gic_id: 34,
        irq_num: 2,
        invert: true,
    },
    Wiring {
        irq_pin: 0,
        comp_pin: 15,
        gic_id: 36,
        irq_num: 4,
        invert: false,
    },
    Wiring {
        irq_pin: 5,
        comp_pin: 4,
        gic_id: 33,
        irq_num: 1,
        invert: false,
    },
    Wiring {
        irq_pin: 8,
        comp_pin: 10,
        gic_id: 32,
        irq_num: 0,
        invert: false,
    },
    // SELECT: P1_14 (trigger-clock input) owns IRQ6 (GIC 38), so SELECT uses IRQ7 on P1_3
    // instead. A/B are swapped versus the polled wiring, so `invert` keeps CW positive.
    Wiring {
        irq_pin: 3,
        comp_pin: 2,
        gic_id: 39,
        irq_num: 7,
        invert: true,
    },
];

/// Every encoder's decoder. Shared by the encoder interrupts and the poll interrupt, which run at the
/// same priority and so never preempt one another; the critical section only states that, as
/// interrupts are already masked in a handler.
#[cfg(target_os = "none")]
static DECODERS: critical_section::Mutex<core::cell::RefCell<[QuadratureDecoder; NUM_ENCODERS]>> =
    critical_section::Mutex::new(core::cell::RefCell::new(
        [QuadratureDecoder::new(false, false); NUM_ENCODERS],
    ));

/// The GIC priority of the encoder and poll interrupts.
#[cfg(target_os = "none")]
const ENCODER_PRIORITY: u8 = 14;

#[cfg(target_os = "none")]
fn pin(pins: u16, n: u8) -> bool {
    (pins >> n) & 1 != 0
}

#[cfg(target_os = "none")]
fn enc_irq_handler(index: usize) {
    let w = &WIRING[index];
    // Clear the pending edge BEFORE reading the pins: an A edge after the read then raises the
    // interrupt again, rather than being cleared away unread.
    unsafe { rza1l_hal::gic::clear_irq_pending(w.irq_num) };
    let pins = unsafe { rza1l_hal::gpio::read_port(1) };
    let edges = critical_section::with(|cs| {
        DECODERS.borrow_ref_mut(cs)[index].observe_a_edge(
            pin(pins, w.irq_pin),
            pin(pins, w.comp_pin),
            w.invert,
        )
    });
    add_edges(index, edges);
}

/// The MTU2 channel whose compare-match interrupt samples the encoder pins. The MIDI/gate one-shot
/// has channel 2; the Embassy time driver runs on the OSTM.
#[cfg(target_os = "none")]
const POLL_CH: u8 = 3;
/// P0φ/64, about 517 kHz.
#[cfg(target_os = "none")]
const POLL_PRESCALER: u16 = 64;
/// One millisecond of P0φ/64 ticks: often enough that a click coming to rest on a B transition is
/// reported at once, rather than when the knob next moves.
#[cfg(target_os = "none")]
const POLL_TICKS: u16 = (rza1l_hal::mtu2::MTU2_P0_HZ / POLL_PRESCALER as u32 / 1000) as u16;

/// The poll's compare-match interrupt. The channel clears its counter on the match, so leaving it
/// armed makes it fire every period.
#[cfg(target_os = "none")]
fn poll_irq_handler() {
    unsafe { rza1l_hal::mtu2::clear_flag(POLL_CH) };
    poll();
}

/// Sample every encoder's pins once, catching the B-only transitions the A interrupts cannot.
#[cfg(target_os = "none")]
fn poll() {
    critical_section::with(|cs| {
        let pins = unsafe { rza1l_hal::gpio::read_port(1) };
        let mut decoders = DECODERS.borrow_ref_mut(cs);
        for (index, w) in WIRING.iter().enumerate() {
            let edges =
                decoders[index].observe(pin(pins, w.irq_pin), pin(pins, w.comp_pin), w.invert);
            add_edges(index, edges);
        }
    });
}

/// One-time encoder GPIO + GIC edge-IRQ bring-up. Device only — the host has no
/// front-panel encoders; deltas simply never accumulate (see the host no-op).
///
/// Also starts the millisecond pin poll on MTU2 channel 3. The firmware task consumes
/// [`ENCODER_DELTAS`] and [`ENCODER_WAKER`] to apply product-specific behavior.
///
/// # Safety
/// Must be called before `cortex_ar::interrupt::enable()`.
#[cfg(target_os = "none")]
pub unsafe fn irq_init() {
    unsafe {
        for w in &WIRING {
            rza1l_hal::gpio::set_pin_mux(1, w.irq_pin, 2);
            rza1l_hal::gpio::enable_input_buffer(1, w.irq_pin);
            rza1l_hal::gpio::set_as_input(1, w.comp_pin);
        }

        // Start every decoder at the pins' current levels, so bring-up isn't read as a turn.
        let pins = rza1l_hal::gpio::read_port(1);
        critical_section::with(|cs| {
            let mut decoders = DECODERS.borrow_ref_mut(cs);
            for (decoder, w) in decoders.iter_mut().zip(&WIRING) {
                *decoder = QuadratureDecoder::new(pin(pins, w.irq_pin), pin(pins, w.comp_pin));
            }
        });

        for w in &WIRING {
            rza1l_hal::gic::set_irq_both_edges(w.irq_num);
        }

        rza1l_hal::gic::register(WIRING[0].gic_id, || enc_irq_handler(0));
        rza1l_hal::gic::register(WIRING[1].gic_id, || enc_irq_handler(1));
        rza1l_hal::gic::register(WIRING[2].gic_id, || enc_irq_handler(2));
        rza1l_hal::gic::register(WIRING[3].gic_id, || enc_irq_handler(3));
        rza1l_hal::gic::register(WIRING[4].gic_id, || enc_irq_handler(4));
        rza1l_hal::gic::register(WIRING[5].gic_id, || enc_irq_handler(5));

        for w in &WIRING {
            rza1l_hal::gic::set_priority(w.gic_id, ENCODER_PRIORITY);
            rza1l_hal::gic::enable(w.gic_id);
        }

        // The pin poll, at the encoders' own priority so neither preempts the other. Arming it once
        // is enough: see `poll_irq_handler`.
        rza1l_hal::mtu2::enable_write();
        rza1l_hal::mtu2::setup_one_shot(
            POLL_CH,
            POLL_PRESCALER,
            poll_irq_handler,
            ENCODER_PRIORITY,
        );
        rza1l_hal::mtu2::arm(POLL_CH, POLL_TICKS);

        info!(
            "encoder: interrupt-driven init complete (IRQ0/1/2/3/4/7 → GIC 32–36, 39; MTU2 ch3 poll)"
        );
    }
}

/// Host: no front-panel hardware. Encoders are quiescent; `ENCODER_DELTAS`
/// stays zero and `ENCODER_WAKER` never fires, so any consumer parks cleanly.
///
/// # Safety
/// No-op; trivially safe. Signature matches the device variant so callers
/// don't need to branch on target.
#[cfg(not(target_os = "none"))]
pub unsafe fn irq_init() {}

#[cfg(all(test, not(target_os = "none")))]
mod host_tests {
    use super::*;
    use core::sync::atomic::Ordering;

    #[test]
    fn host_irq_init_is_a_safe_noop_and_encoders_are_quiescent() {
        // Must not touch hardware / must not panic on host.
        unsafe { irq_init() };
        assert_eq!(NUM_ENCODERS, 6);
        // Deltas start (and stay) zero with no ISRs firing.
        for d in &ENCODER_DELTAS {
            assert_eq!(d.load(Ordering::Relaxed), 0);
        }
        // take_detents on a zero delta yields no detents (pure-logic sanity).
        let mut acc = 0i8;
        assert_eq!(take_detents(0, &mut acc), 0);
    }
}
