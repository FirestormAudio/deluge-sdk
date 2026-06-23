//! DIN MIDI input/output.

#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicBool, Ordering};

/// MIDI DIN baud rate.
#[cfg(target_os = "none")]
const MIDI_BAUD: u32 = 31_250;

#[cfg(target_os = "none")]
fn ensure_init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: runs once. Sets up SCIF0 with DMA RX and registers its TX handler
    // before the source is enabled, so it is safe at runtime.
    unsafe { deluge_bsp::uart::init_midi(MIDI_BAUD) };
}

/// The DIN MIDI port (SCIF0), taken once from [`Deluge::midi`](crate::Deluge::midi).
///
/// A raw byte stream in both directions — bring your own parser (a typed message
/// API may come later). RX is DMA-backed, so bytes are captured even while the
/// app is busy.
pub struct Midi {
    _private: (),
}

impl Midi {
    pub(crate) fn new() -> Self {
        #[cfg(target_os = "none")]
        ensure_init();
        Self { _private: () }
    }

    /// Send raw MIDI bytes. No-op on the host simulator (no DIN MIDI).
    #[inline]
    pub async fn send(&self, data: &[u8]) {
        #[cfg(target_os = "none")]
        deluge_bsp::uart::write_midi(data).await;
        // On the host, hand the bytes to the simulator panel (lights the MIDI
        // OUT activity indicator; the GUI can forward them to a host port).
        #[cfg(not(target_os = "none"))]
        crate::host::panel().push_midi_out(data);
    }

    /// Await the next received MIDI byte.
    #[inline]
    pub async fn recv(&self) -> u8 {
        #[cfg(target_os = "none")]
        {
            deluge_bsp::uart::read_midi_byte().await
        }
        // On the host, drain bytes the simulator's MIDI bridge pushed into the
        // panel, polling at ~1 ms when the queue is empty (DIN MIDI is slow, so
        // the latency is inaudible).
        #[cfg(not(target_os = "none"))]
        loop {
            if let Some(b) = crate::host::panel().pop_midi_in() {
                return b;
            }
            embassy_time::Timer::after_millis(1).await;
        }
    }

    /// Take the next received byte if one is buffered, without awaiting.
    #[inline]
    pub fn try_recv(&self) -> Option<u8> {
        #[cfg(target_os = "none")]
        {
            deluge_bsp::uart::try_read_midi()
        }
        #[cfg(not(target_os = "none"))]
        {
            crate::host::panel().pop_midi_in()
        }
    }
}
