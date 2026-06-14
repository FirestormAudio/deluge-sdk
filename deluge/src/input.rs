//! Unified async input event stream (pads, buttons, encoders).
//!
//! Two hardware sources are merged into one queue:
//! - **pads & buttons** arrive over the PIC RX stream — [`crate::pic_service`]'s
//!   pump decodes them and calls [`route_pic_event`];
//! - **encoders** are wired to RZ/A1L GPIO interrupts — an [`encoder_pump`] task
//!   wakes on the encoder IRQ, drains the detent deltas, and enqueues them.
//!
//! Apps drain the queue via [`Input::next`].

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Poll;

use deluge_bsp::{encoder, pic};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

/// A decoded input event.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A grid pad changed state. `x` 0–17, `y` 0–7.
    Pad { x: u8, y: u8, pressed: bool },
    /// A button changed state. `id` is the raw PIC button id (0–35).
    Button { id: u8, pressed: bool },
    /// An encoder turned. `index` 0–5; `delta` is signed detents since the last
    /// event (positive = clockwise).
    Encoder { index: u8, delta: i8 },
}

/// Bounded event queue. If an app stops draining, the oldest events are dropped
/// (producers use `try_send`) so input never stalls the PIC pump or an ISR-fed
/// task.
static EVENTS: Channel<CriticalSectionRawMutex, Event, 32> = Channel::new();

/// The input event stream, taken once from [`Deluge::input`](crate::Deluge::input).
pub struct Input {
    _private: (),
}

impl Input {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Await the next input event.
    #[inline]
    pub async fn next(&self) -> Event {
        EVENTS.receive().await
    }

    /// Return the next event if one is queued, without awaiting.
    #[inline]
    pub fn try_next(&self) -> Option<Event> {
        EVENTS.try_receive().ok()
    }
}

/// Map a PIC pad/button event into an [`Event`] and enqueue it.
///
/// Called by the PIC pump for every non-OLED event; OLED chip-select echoes are
/// handled by the pump itself. Drops the event if the queue is full.
pub(crate) fn route_pic_event(ev: pic::Event) {
    let mapped = match ev {
        pic::Event::PadPress { id } => {
            let (x, y) = pic::pad_coords(id);
            Event::Pad { x, y, pressed: true }
        }
        pic::Event::PadRelease { id } => {
            let (x, y) = pic::pad_coords(id);
            Event::Pad { x, y, pressed: false }
        }
        pic::Event::ButtonPress { id } => Event::Button { id, pressed: true },
        pic::Event::ButtonRelease { id } => Event::Button { id, pressed: false },
        // FirmwareVersion / NoPresses / future variants are not input events.
        _ => return,
    };
    let _ = EVENTS.try_send(mapped);
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// Configure the encoder GPIO interrupts and spawn the encoder pump. Idempotent.
///
/// Pads/buttons additionally require the PIC service; [`Deluge::input`] starts
/// that too.
pub(crate) fn ensure_started(spawner: Spawner) {
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: runs once (guarded above). `irq_init` registers each encoder's GIC
    // handler before enabling its source, so it is safe with interrupts enabled.
    unsafe { encoder::irq_init() };
    spawner.spawn(encoder_pump().unwrap());
}

/// Wake on encoder IRQ, drain detent deltas, and enqueue [`Event::Encoder`].
#[embassy_executor::task]
async fn encoder_pump() {
    let mut acc = [0i8; encoder::NUM_ENCODERS];
    loop {
        // Sleep until an ISR records a non-zero delta on some encoder.
        poll_fn(|cx| {
            encoder::ENCODER_WAKER.register(cx.waker());
            if encoder::ENCODER_DELTAS
                .iter()
                .any(|d| d.load(Ordering::Relaxed) != 0)
            {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;

        for i in 0..encoder::NUM_ENCODERS {
            let delta = encoder::take_detents(i, &mut acc[i]);
            if delta != 0 {
                let _ = EVENTS.try_send(Event::Encoder {
                    index: i as u8,
                    delta,
                });
            }
        }
    }
}
