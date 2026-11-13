//! Front-panel rotary encoders: two-pin quadrature decoding into per-encoder edge accumulators.
//!
//! Each encoder's A pin raises an interrupt on both edges; its B pin has none, so MTU2 channel 3
//! interrupts every millisecond to read every encoder's pins and catch B-only transitions. Both feed
//! the encoder's [`QuadratureDecoder`], which counts four edges per cycle and rejects single-pin
//! glitches (see [`crate::encoder_quadrature`]).
//!
//! Each decoded run of edges is kept twice: in the encoder's [`ENCODER_DELTAS`] count, for a consumer that only
//! needs how far it turned, and with the time it was decoded, for one that accelerates on how fast it turned
//! ([`take_timed_edges`]).

use core::sync::atomic::{AtomicI8, Ordering};

use embassy_sync::waitqueue::AtomicWaker;

use crate::encoder_edge_log::EdgeLog;
pub use crate::encoder_edge_log::{EDGE_LOG_CAPACITY, EdgeEvent};
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

/// Every encoder's timed runs, in [`ENCODER_DELTAS`] order: what [`take_timed_edges`] hands out. Written with
/// [`ENCODER_DELTAS`] under one critical section, so a consumer that takes both sees the same edges in each.
static EDGE_LOGS: critical_section::Mutex<core::cell::RefCell<[EdgeLog; NUM_ENCODERS]>> =
    critical_section::Mutex::new(core::cell::RefCell::new([EdgeLog::new(); NUM_ENCODERS]));

/// Drain the accumulated edge delta for one encoder and convert it into whole detents.
#[inline]
pub fn take_detents(encoder_index: usize, edge_accumulator: &mut i8) -> i8 {
    let delta = take_edges(encoder_index);
    // Pure edge→detent accumulation lives in `crate::encoder_detent` (host-tested).
    crate::encoder_detent::accumulate_detents(delta, edge_accumulator)
}

/// Adds `edges`, decoded at `time_us`, to encoder `index`: to its [`ENCODER_DELTAS`] count, saturating, and to its
/// timed runs. Wakes the consumer. The encoder interrupts record through this; so can a simulator or a test
/// harness that stands in for them, on the clock its consumer reads.
pub fn record_edges(index: usize, edges: i8, time_us: u64) {
    if edges == 0 || index >= NUM_ENCODERS {
        return;
    }
    critical_section::with(|cs| {
        let _ = ENCODER_DELTAS[index].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_add(edges))
        });
        EDGE_LOGS.borrow_ref_mut(cs)[index].push(edges, time_us);
    });
    ENCODER_WAKER.wake();
}

/// Takes encoder `index`'s edge count and clears its timed runs: the untimed take.
pub fn take_edges(index: usize) -> i8 {
    if index >= NUM_ENCODERS {
        return 0;
    }
    critical_section::with(|cs| {
        EDGE_LOGS.borrow_ref_mut(cs)[index].clear();
        ENCODER_DELTAS[index].swap(0, Ordering::Relaxed)
    })
}

/// Takes encoder `index`'s edges as timed runs, oldest first, into `out`; returns how many slots it wrote.
///
/// Each run carries the time it was decoded, so a consumer that runs late still sees how fast the edges came.
/// Edges added to [`ENCODER_DELTAS`] without a time (a writer that does not go through [`record_edges`]) come
/// last, as one run at `now_us`. When `out` is shorter than the runs, the rest fold into its last slot. Clears the
/// count too, so the untimed and the timed take never hand out the same edges.
pub fn take_timed_edges(index: usize, out: &mut [EdgeEvent], now_us: u64) -> usize {
    if index >= NUM_ENCODERS {
        return 0;
    }
    critical_section::with(|cs| {
        let mut logs = EDGE_LOGS.borrow_ref_mut(cs);
        let log = &mut logs[index];
        let untimed = i32::from(ENCODER_DELTAS[index].swap(0, Ordering::Relaxed)) - log.edges();
        log.push(
            untimed.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8,
            now_us,
        );
        log.drain_into(out)
    })
}

/// The interrupts' clock for [`record_edges`].
#[cfg(target_os = "none")]
fn now_us() -> u64 {
    embassy_time::Instant::now().as_micros()
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
    /// A black encoder, which clicks into a detent: it rests with both contacts open (A and B high).
    detented: bool,
}

#[cfg(target_os = "none")]
const WIRING: [Wiring; NUM_ENCODERS] = [
    Wiring {
        irq_pin: 11,
        comp_pin: 12,
        gic_id: 35,
        irq_num: 3,
        invert: false,
        detented: true,
    },
    Wiring {
        irq_pin: 6,
        comp_pin: 7,
        gic_id: 34,
        irq_num: 2,
        invert: true,
        detented: true,
    },
    Wiring {
        irq_pin: 0,
        comp_pin: 15,
        gic_id: 36,
        irq_num: 4,
        invert: false,
        detented: false,
    },
    Wiring {
        irq_pin: 5,
        comp_pin: 4,
        gic_id: 33,
        irq_num: 1,
        invert: false,
        detented: false,
    },
    Wiring {
        irq_pin: 8,
        comp_pin: 10,
        gic_id: 32,
        irq_num: 0,
        invert: false,
        detented: true,
    },
    // SELECT: P1_14 (trigger-clock input) owns IRQ6 (GIC 38), so SELECT uses IRQ7 on P1_3
    // instead. A/B are swapped versus the polled wiring, so `invert` keeps CW positive.
    Wiring {
        irq_pin: 3,
        comp_pin: 2,
        gic_id: 39,
        irq_num: 7,
        invert: true,
        detented: true,
    },
];

// Each encoder's A pin is bound to the typed interrupt below; its GIC ID must agree.
#[cfg(target_os = "none")]
const _: () = {
    use rza1_hal::interrupt::{Interrupt, typelevel::*};
    assert!(WIRING[0].gic_id == IRQ3::ID && WIRING[1].gic_id == IRQ2::ID);
    assert!(WIRING[2].gic_id == IRQ4::ID && WIRING[3].gic_id == IRQ1::ID);
    assert!(WIRING[4].gic_id == IRQ0::ID && WIRING[5].gic_id == IRQ7::ID);
};

/// Every encoder's decoder. Shared by the encoder interrupts and the poll interrupt, which run at the
/// same priority and so never preempt one another; the critical section only states that, as
/// interrupts are already masked in a handler.
#[cfg(target_os = "none")]
static DECODERS: critical_section::Mutex<core::cell::RefCell<[QuadratureDecoder; NUM_ENCODERS]>> =
    critical_section::Mutex::new(core::cell::RefCell::new(
        [QuadratureDecoder::new(false, false); NUM_ENCODERS],
    ));

#[cfg(target_os = "none")]
fn pin(pins: u16, n: u8) -> bool {
    (pins >> n) & 1 != 0
}

#[cfg(target_os = "none")]
fn enc_irq_handler(index: usize) {
    let w = &WIRING[index];
    // Clear the pending edge BEFORE reading the pins: an A edge after the read then raises the
    // interrupt again, rather than being cleared away unread.
    unsafe { rza1_hal::gic::clear_irq_pending(w.irq_num) };
    let pins = rza1_hal::gpio::read_port(rza1_hal::gpio::Port::P1);
    let edges = critical_section::with(|cs| {
        DECODERS.borrow_ref_mut(cs)[index].observe_a_edge(
            pin(pins, w.irq_pin),
            pin(pins, w.comp_pin),
            w.invert,
        )
    });
    record_edges(index, edges, now_us());
}

/// P0φ/64, about 517 kHz.
#[cfg(target_os = "none")]
const POLL_PRESCALER: u16 = 64;
/// One millisecond of P0φ/64 ticks: often enough that a click coming to rest on a B transition is
/// reported at once, rather than when the knob next moves.
#[cfg(target_os = "none")]
const POLL_TICKS: u16 = (crate::system::CLOCKS.p0_hz() / POLL_PRESCALER as u32 / 1000) as u16;
#[cfg(target_os = "none")]
const _: () = assert!(POLL_TICKS == 516, "encoder poll period changed");

/// The poll timer (MTU2 channel 3), once [`irq_init`] has run. Its handler reaches it here.
#[cfg(target_os = "none")]
static POLL_TIMER: embassy_sync::once_lock::OnceLock<rza1_hal::mtu2::OneShot<'static>> =
    embassy_sync::once_lock::OnceLock::new();

/// The poll's compare-match interrupt. The channel clears its counter on the match, so leaving it
/// armed makes it fire every period.
#[cfg(target_os = "none")]
struct PollHandler;

#[cfg(target_os = "none")]
impl rza1_hal::interrupt::Handler<rza1_hal::interrupt::typelevel::TGI3A> for PollHandler {
    unsafe fn on_interrupt() {
        if let Some(timer) = POLL_TIMER.try_get() {
            timer.clear_flag();
        }
        poll();
    }
}

#[cfg(target_os = "none")]
rza1_hal::bind_interrupts!(struct PollIrqs {
    TGI3A => PollHandler;
});

/// Sample every encoder's pins once, catching the B-only transitions the A interrupts cannot.
#[cfg(target_os = "none")]
fn poll() {
    critical_section::with(|cs| {
        let pins = rza1_hal::gpio::read_port(rza1_hal::gpio::Port::P1);
        let mut decoders = DECODERS.borrow_ref_mut(cs);
        for (index, w) in WIRING.iter().enumerate() {
            let edges =
                decoders[index].observe(pin(pins, w.irq_pin), pin(pins, w.comp_pin), w.invert);
            record_edges(index, edges, now_us());
        }
    });
}

/// Encoder `I`'s A-pin interrupt.
#[cfg(target_os = "none")]
struct EncoderHandler<const I: usize>;

#[cfg(target_os = "none")]
macro_rules! encoder_handlers {
    ($($i:literal => $irq:ident),* $(,)?) => {$(
        impl rza1_hal::interrupt::Handler<rza1_hal::interrupt::typelevel::$irq>
            for EncoderHandler<$i>
        {
            unsafe fn on_interrupt() {
                enc_irq_handler($i);
            }
        }
    )*};
}

#[cfg(target_os = "none")]
encoder_handlers!(0 => IRQ3, 1 => IRQ2, 2 => IRQ4, 3 => IRQ1, 4 => IRQ0, 5 => IRQ7);

#[cfg(target_os = "none")]
rza1_hal::bind_interrupts!(struct EncoderIrqs {
    IRQ3 => EncoderHandler<0>;
    IRQ2 => EncoderHandler<1>;
    IRQ4 => EncoderHandler<2>;
    IRQ1 => EncoderHandler<3>;
    IRQ0 => EncoderHandler<4>;
    IRQ7 => EncoderHandler<5>;
});

/// One-time encoder GPIO + GIC edge-IRQ bring-up. Device only — the host has no
/// front-panel encoders; deltas simply never accumulate (see the host no-op).
///
/// Also starts the millisecond pin poll on MTU2 channel 3. The firmware task consumes
/// [`ENCODER_DELTAS`] and [`ENCODER_WAKER`] to apply product-specific behavior.
///
/// # Safety
/// Must be called before `cortex_ar::interrupt::enable()`.
#[cfg(target_os = "none")]
pub unsafe fn irq_init(
    poll_timer: rza1_hal::Peri<'static, crate::board::EncoderPollTimer>,
    pins: crate::board::EncoderPins,
) {
    use rza1_hal::gpio::{Alternate, Flex, Input, Pin, Port};
    unsafe {
        let crate::board::EncoderPins { irq, comp } = pins;
        for ((w, irq), comp) in WIRING.iter().zip(irq).zip(comp) {
            debug_assert_eq!(
                (irq.port(), irq.bit(), comp.port(), comp.bit()),
                (Port::P1, w.irq_pin, Port::P1, w.comp_pin)
            );
            // IRQn on alternative 2, its input buffer on so the ISR can read the level too.
            let mut irq = Flex::new(irq);
            irq.set_as_alternate(Alternate::Alt2);
            irq.enable_input_buffer();
            Input::new(comp);
        }

        // Start each gold encoder at its pins' current levels, so bring-up isn't read as a turn. A black one
        // starts at the detent's resting state instead, whatever the pins read: its clicks are counted from that
        // state, so a read that catches contact bounce or an unsettled input would put every click out of step
        // until the next reboot, making the first click after each reversal do nothing.
        let pins = rza1_hal::gpio::read_port(rza1_hal::gpio::Port::P1);
        critical_section::with(|cs| {
            let mut decoders = DECODERS.borrow_ref_mut(cs);
            for (decoder, w) in decoders.iter_mut().zip(&WIRING) {
                *decoder = if w.detented {
                    QuadratureDecoder::new(true, true)
                } else {
                    QuadratureDecoder::new(pin(pins, w.irq_pin), pin(pins, w.comp_pin))
                };
            }
        });

        for w in &WIRING {
            rza1_hal::gic::set_irq_both_edges(w.irq_num);
        }

        use rza1_hal::interrupt::{self, typelevel::*};
        interrupt::bind::<IRQ3, EncoderHandler<0>>(EncoderIrqs);
        interrupt::bind::<IRQ2, EncoderHandler<1>>(EncoderIrqs);
        interrupt::bind::<IRQ4, EncoderHandler<2>>(EncoderIrqs);
        interrupt::bind::<IRQ1, EncoderHandler<3>>(EncoderIrqs);
        interrupt::bind::<IRQ0, EncoderHandler<4>>(EncoderIrqs);
        interrupt::bind::<IRQ7, EncoderHandler<5>>(EncoderIrqs);

        // Their priority is the board's ENCODER_PRIORITY, set by `Board::new`.
        fn arm<I: interrupt::Interrupt>() {
            // SAFETY: the handler was bound above and its encoder state is initialised.
            unsafe { I::enable() };
        }
        arm::<IRQ3>();
        arm::<IRQ2>();
        arm::<IRQ4>();
        arm::<IRQ1>();
        arm::<IRQ0>();
        arm::<IRQ7>();

        // The pin poll, at the encoders' own priority so neither preempts the other. Arming it once
        // is enough: see `PollHandler`.
        // Store the timer before arming it: its handler clears the match through POLL_TIMER,
        // and an uncleared match re-raises the interrupt at once.
        let timer = rza1_hal::mtu2::OneShot::new(poll_timer, POLL_PRESCALER, PollIrqs);
        if POLL_TIMER.init(timer).is_err() {
            unreachable!("MTU2 channel 3 is owned once");
        }
        if let Some(timer) = POLL_TIMER.try_get() {
            timer.arm(POLL_TICKS);
        }

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
pub unsafe fn irq_init(
    _poll_timer: rza1_hal::Peri<'static, crate::board::EncoderPollTimer>,
    _pins: crate::board::EncoderPins,
) {
}

#[cfg(all(test, not(target_os = "none")))]
mod host_tests {
    use super::*;
    use core::sync::atomic::Ordering;

    /// The encoders' state is global: the tests here take turns with it.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn host_irq_init_is_a_safe_noop_and_encoders_are_quiescent() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // Must not touch hardware / must not panic on host.
        let (board, _) = crate::board::Board::split(unsafe { rza1_hal::Peripherals::steal() });
        unsafe { irq_init(board.encoder_poll_timer, board.pins.encoders) };
        assert_eq!(NUM_ENCODERS, 6);
        // Deltas start (and stay) zero with no ISRs firing.
        for d in &ENCODER_DELTAS {
            assert_eq!(d.load(Ordering::Relaxed), 0);
        }
        // take_detents on a zero delta yields no detents (pure-logic sanity).
        let mut acc = 0i8;
        assert_eq!(take_detents(0, &mut acc), 0);
    }

    #[test]
    fn a_late_timed_take_sees_each_run_at_its_own_time() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_edges(3);
        record_edges(3, 1, 3_000);
        record_edges(3, 1, 6_000);
        record_edges(3, -1, 9_000);
        let mut out = [EdgeEvent::default(); EDGE_LOG_CAPACITY + 1];
        let n = take_timed_edges(3, &mut out, 20_000);
        assert_eq!(
            out[..n],
            [
                EdgeEvent {
                    time_us: 3_000,
                    edges: 1
                },
                EdgeEvent {
                    time_us: 6_000,
                    edges: 1
                },
                EdgeEvent {
                    time_us: 9_000,
                    edges: -1
                },
            ]
        );
        assert_eq!(ENCODER_DELTAS[3].load(Ordering::Relaxed), 0);
        assert_eq!(take_timed_edges(3, &mut out, 21_000), 0);
    }

    #[test]
    fn untimed_edges_come_last_at_the_take_time() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_edges(2);
        record_edges(2, 1, 1_000);
        ENCODER_DELTAS[2].fetch_add(4, Ordering::Relaxed);
        let mut out = [EdgeEvent::default(); 4];
        let n = take_timed_edges(2, &mut out, 5_000);
        assert_eq!(
            out[..n],
            [
                EdgeEvent {
                    time_us: 1_000,
                    edges: 1
                },
                EdgeEvent {
                    time_us: 5_000,
                    edges: 4
                },
            ]
        );
    }

    #[test]
    fn the_untimed_take_clears_the_timed_runs() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        record_edges(1, 2, 1_000);
        assert_eq!(take_edges(1), 2);
        let mut out = [EdgeEvent::default(); 4];
        assert_eq!(take_timed_edges(1, &mut out, 2_000), 0);
    }
}
