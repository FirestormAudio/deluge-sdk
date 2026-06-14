//! DIN MIDI input/output.

use core::sync::atomic::{AtomicBool, Ordering};

/// MIDI DIN baud rate.
const MIDI_BAUD: u32 = 31_250;

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
        ensure_init();
        Self { _private: () }
    }

    /// Send raw MIDI bytes.
    #[inline]
    pub async fn send(&self, data: &[u8]) {
        deluge_bsp::uart::write_midi(data).await;
    }

    /// Await the next received MIDI byte.
    #[inline]
    pub async fn recv(&self) -> u8 {
        deluge_bsp::uart::read_midi_byte().await
    }

    /// Take the next received byte if one is buffered, without awaiting.
    #[inline]
    pub fn try_recv(&self) -> Option<u8> {
        deluge_bsp::uart::try_read_midi()
    }
}
