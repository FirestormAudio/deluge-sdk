//! The board's peripherals that no SDK capability has claimed yet.
//!
//! `#[deluge::app]` brings the board up before `setup` runs and keeps here the peripherals its
//! capabilities drive. Each capability takes its own the first time it is used: [`Deluge::midi`]
//! takes SCIF0 and its receive DMA channel, and so on. An app that drives one of these through
//! `deluge-bsp` itself takes it first, with [`with`]. The capability that needed it then panics,
//! naming the peripheral, instead of driving hardware the app owns.
//!
//! [`Deluge::midi`]: crate::Deluge::midi

use core::cell::RefCell;

use deluge_bsp::board::{
    EncoderPollTimer, GateTimer, MidiRxDma, MidiScif, OledDma, PicRxDma, PicScif, PicTxDma,
    SharedSpi,
};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use rza1l_hal::Peri;

/// The unclaimed peripherals. A `None` field has been taken.
pub struct Parts {
    /// RSPI0, for `deluge_bsp::bus::init` (the OLED and CV).
    pub shared_spi: Option<Peri<'static, SharedSpi>>,
    /// The OLED's DMA channel, for `deluge_bsp::bus::init`.
    pub oled_dma: Option<Peri<'static, OledDma>>,
    /// SCIF0, for `deluge_bsp::uart::init_midi`.
    pub midi_uart: Option<Peri<'static, MidiScif>>,
    /// MIDI's receive DMA channel, for `deluge_bsp::uart::init_midi`.
    pub midi_rx_dma: Option<Peri<'static, MidiRxDma>>,
    /// SCIF1, for `deluge_bsp::uart::init_pic`.
    pub pic_uart: Option<Peri<'static, PicScif>>,
    /// The PIC link's receive DMA channel, for `deluge_bsp::uart::init_pic`.
    pub pic_rx_dma: Option<Peri<'static, PicRxDma>>,
    /// The PIC link's transmit DMA channel, for `deluge_bsp::uart::init_pic`.
    pub pic_tx_dma: Option<Peri<'static, PicTxDma>>,
    /// MTU2 channel 2, for `deluge_bsp::midi_gate::setup`.
    pub gate_timer: Option<Peri<'static, GateTimer>>,
    /// MTU2 channel 3, for `deluge_bsp::encoder::irq_init`.
    pub encoder_poll_timer: Option<Peri<'static, EncoderPollTimer>>,
}

static PARTS: Mutex<CriticalSectionRawMutex, RefCell<Option<Parts>>> =
    Mutex::new(RefCell::new(None));

/// Keep `parts` for the capabilities and the app. The runtime calls this once, before `setup`.
pub(crate) fn stash(parts: Parts) {
    PARTS.lock(|p| *p.borrow_mut() = Some(parts));
}

/// Run `f` on the unclaimed peripherals, in a critical section. Take one with `Option::take`:
///
/// ```ignore
/// let timer = deluge::parts::with(|p| p.gate_timer.take()).expect("MTU2 channel 2 is free");
/// deluge_bsp::midi_gate::setup(timer);
/// ```
///
/// # Panics
/// Before `#[deluge::app]` has brought the board up, or if `f` calls `with` again.
pub fn with<R>(f: impl FnOnce(&mut Parts) -> R) -> R {
    PARTS.lock(|p| {
        f(p.borrow_mut()
            .as_mut()
            .expect("deluge::parts::with ran before #[deluge::app] brought the board up"))
    })
}

/// Take the peripheral `f` picks, for a capability.
///
/// # Panics
/// Naming `what`, if the app took it through [`with`].
pub(crate) fn take<T>(what: &str, f: impl FnOnce(&mut Parts) -> Option<T>) -> T {
    with(f).unwrap_or_else(|| panic!("{what} was taken by the app through deluge::parts"))
}
