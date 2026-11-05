//! Deluge board UART assignments.
//!
//! Maps the two SCIF channels used on the Deluge to their physical pins and
//! gives them meaningful names, keeping [`rza1_hal::uart`] itself board-agnostic.
//!
//! | Channel | Signal  | TxD pin  | mux | RxD pin | mux |
//! |---------|---------|----------|-----|---------|-----|
//! | SCIF0   | MIDI DIN| P6_15    |  5  | P6_14   |  5  |
//! | SCIF1   | PIC32   | P3_15    |  5  | P1_9    |  3  |
//!
//! [`init_midi`] and [`init_pic`] take the SCIF and its DMA channels from the [`crate::Board`]
//! and keep the UART's halves; the other functions reach the UART they brought up.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::once_lock::OnceLock;
use rza1_hal::Peri;
use rza1_hal::dmac::{self, Dma};
use rza1_hal::mode::Async;
use rza1_hal::uart::{self, Uart, UartRx, UartTx};

use crate::board::{MidiPins, MidiRxDma, MidiScif, PicPins, PicRxDma, PicScif, PicTxDma};

/// SCIF channel used for MIDI DIN (31250 bps).
pub const MIDI_CH: usize = 0;

/// SCIF channel used for the PIC32 co-processor.
pub const PIC_CH: usize = 1;

rza1_hal::bind_interrupts!(struct MidiIrqs {
    BRI0 => uart::BreakInterruptHandler<MidiScif>;
    ERI0 => uart::ErrorInterruptHandler<MidiScif>;
    TXI0 => uart::TxInterruptHandler<MidiScif>;
    DMAINT13 => dmac::InterruptHandler<MidiRxDma>;
});

rza1_hal::bind_interrupts!(struct PicIrqs {
    BRI1 => uart::BreakInterruptHandler<PicScif>;
    ERI1 => uart::ErrorInterruptHandler<PicScif>;
    TXI1 => uart::TxInterruptHandler<PicScif>;
    DMAINT10 => dmac::InterruptHandler<PicTxDma>;
    DMAINT12 => dmac::InterruptHandler<PicRxDma>;
});

/// A UART's receiver, async-locked because a read holds it across `.await`.
type RxSlot = Mutex<CriticalSectionRawMutex, Option<UartRx<'static, Async>>>;

static MIDI_RX: RxSlot = Mutex::new(None);
static MIDI_TX: OnceLock<UartTx<'static, Async>> = OnceLock::new();
static PIC_RX: RxSlot = Mutex::new(None);
static PIC_TX: OnceLock<UartTx<'static, Async>> = OnceLock::new();

/// Bring up the MIDI DIN UART (SCIF0): receive by DMA, transmit through the FIFO and TXI
/// (MIDI messages are short).
pub fn init_midi(
    scif: Peri<'static, MidiScif>,
    pins: MidiPins,
    rx_dma: Peri<'static, MidiRxDma>,
    baud: u32,
) {
    log::debug!("bsp uart: MIDI SCIF{} @ {} bps", MIDI_CH, baud);
    let rx_dma = Dma::new(rx_dma, MidiIrqs);
    let (rx, tx) =
        Uart::new_with_rx_dma(scif, pins.tx, pins.rx, rx_dma, None, MidiIrqs, config(baud)).split();
    install(&MIDI_RX, &MIDI_TX, rx, tx);
}

/// Bring up the PIC32 UART (SCIF1): receive and transmit by DMA.
///
/// DMA rather than RXI because a fault path reads the PIC ring without its owner
/// ([`steal_pic_rx`]), which only the DMA ring supports.
pub fn init_pic(
    scif: Peri<'static, PicScif>,
    pins: PicPins,
    rx_dma: Peri<'static, PicRxDma>,
    tx_dma: Peri<'static, PicTxDma>,
    baud: u32,
) {
    log::debug!("bsp uart: PIC SCIF{} @ {} bps", PIC_CH, baud);
    let rx_dma = Dma::new(rx_dma, PicIrqs);
    let tx_dma = Dma::new(tx_dma, PicIrqs);
    let (rx, tx) = Uart::new_with_rx_dma(
        scif,
        pins.tx,
        pins.rx,
        rx_dma,
        Some(tx_dma),
        PicIrqs,
        config(baud),
    )
    .split();
    install(&PIC_RX, &PIC_TX, rx, tx);
}

/// 8N1 at `baud`: both links' line settings.
fn config(baud: u32) -> uart::Config {
    let mut config = uart::Config::default();
    config.baudrate = baud;
    config
}

fn install(
    rx_slot: &'static RxSlot,
    tx_slot: &'static OnceLock<UartTx<'static, Async>>,
    rx: UartRx<'static, Async>,
    tx: UartTx<'static, Async>,
) {
    // Each SCIF is owned once, so both slots are empty.
    match rx_slot.try_lock() {
        Ok(mut slot) => *slot = Some(rx),
        Err(_) => unreachable!("a UART's receiver is locked before the UART exists"),
    }
    if tx_slot.init(tx).is_err() {
        unreachable!("a UART's transmitter is installed twice");
    }
}

/// The next byte, logging (at debug: a fault path reads through here, and a full RTT ring blocks)
/// and skipping receive errors: a framing error on a MIDI cable is noise
/// to its readers, not a reason to stop.
async fn read_byte(slot: &'static RxSlot, name: &str) -> u8 {
    let mut rx = slot.lock().await;
    let Some(rx) = rx.as_mut() else {
        panic!("the {name} UART isn't initialised");
    };
    loop {
        match rx.read_byte().await {
            Ok(byte) => return byte,
            Err(e) => log::debug!("bsp uart: {name} receive error: {e:?}"),
        }
    }
}

fn try_read(slot: &'static RxSlot, name: &str) -> Option<u8> {
    let mut slot = slot.try_lock().ok()?;
    let rx = slot.as_mut()?;
    loop {
        match rx.try_read() {
            Ok(byte) => return byte,
            Err(e) => log::debug!("bsp uart: {name} receive error: {e:?}"),
        }
    }
}

fn tx(
    slot: &'static OnceLock<UartTx<'static, Async>>,
    name: &str,
) -> &'static UartTx<'static, Async> {
    slot.try_get()
        .unwrap_or_else(|| panic!("the {name} UART isn't initialised"))
}

/// Write bytes to the MIDI DIN UART.
///
/// # Panics
/// Before [`init_midi`].
pub async fn write_midi(data: &[u8]) {
    tx(&MIDI_TX, "MIDI").write(data).await;
}

/// Read one byte from the MIDI DIN UART (its DMA ring).
///
/// # Panics
/// Before [`init_midi`].
pub async fn read_midi_byte() -> u8 {
    read_byte(&MIDI_RX, "MIDI").await
}

/// The next raw byte from the MIDI DIN ring, without waiting. `None` if the ring is empty, if
/// [`init_midi`] hasn't run, or while a [`read_midi_byte`] is waiting (it drains the ring).
pub fn try_read_midi() -> Option<u8> {
    try_read(&MIDI_RX, "MIDI")
}

/// Put as much of `data` in the MIDI DIN transmit FIFO as fits now; returns how much. Enough for
/// short messages such as clock pulses (`0xF8`). 0 before [`init_midi`].
pub fn try_write_midi(data: &[u8]) -> usize {
    MIDI_TX.try_get().map_or(0, |tx| tx.try_write(data))
}

/// Free space in the MIDI DIN TX FIFO, in bytes: how many [`try_write_midi`] would take now.
/// 0 before [`init_midi`].
pub fn midi_write_space() -> usize {
    MIDI_TX.try_get().map_or(0, |tx| tx.tx_free())
}

/// Write bytes to the PIC UART. [`crate::pic`] serialises whole commands around this.
///
/// # Panics
/// Before [`init_pic`].
pub async fn write_pic(data: &[u8]) {
    tx(&PIC_TX, "PIC").write(data).await;
}

/// Read one byte from the PIC UART (its DMA ring).
///
/// # Panics
/// Before [`init_pic`].
pub async fn read_pic_byte() -> u8 {
    read_byte(&PIC_RX, "PIC").await
}

/// The next raw byte from the PIC ring, without waiting; see [`try_read_midi`] for `None`. A fault
/// path, where the PIC task may hold the receiver, uses [`steal_pic_rx`].
pub fn try_read_pic() -> Option<u8> {
    try_read(&PIC_RX, "PIC")
}

/// The PIC UART's receiver without its lock, for a fault or freeze path: the PIC task may be
/// suspended mid-read holding the lock, which makes [`try_read_pic`] return `None`. It reads from
/// the PIC task's position, so a byte it consumes isn't delivered again. `None` before
/// [`init_pic`].
///
/// # Safety
/// The PIC task doesn't read the ring while the receiver is in use: interrupts are masked, the
/// executor that runs the PIC task is blocked in the caller, or the caller is an interrupt (an
/// interrupt-priority executor) that preempted it.
pub unsafe fn steal_pic_rx() -> Option<UartRx<'static, Async>> {
    // SAFETY: the PIC ring runs on PicRxDma; the caller's contract covers the rest.
    unsafe { UartRx::steal::<PicScif, PicRxDma>() }
}

/// Put as much of `data` in the PIC transmit FIFO as fits now; returns how much. For fault and
/// panic paths that can't wait for the DMA writer. 0 before [`init_pic`].
pub fn try_write_pic(data: &[u8]) -> usize {
    PIC_TX.try_get().map_or(0, |tx| tx.try_write(data))
}

/// Change the PIC UART's baud rate (the PIC handshake switches 31 250 → 200 000 bps). Wait for
/// the last write to drain first.
///
/// # Panics
/// Before [`init_pic`].
pub fn set_pic_baud(baud: u32) {
    tx(&PIC_TX, "PIC").set_config(&config(baud));
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::{MIDI_CH, PIC_CH};

    #[test]
    fn midi_channel_is_scif0() {
        assert_eq!(MIDI_CH, 0);
    }

    #[test]
    fn pic_channel_is_scif1() {
        assert_eq!(PIC_CH, 1);
    }

    /// Channels must be distinct so both UARTs can operate concurrently.
    #[test]
    fn channels_are_distinct() {
        assert_ne!(MIDI_CH, PIC_CH);
    }

    /// Both channels must be within the RZ/A1L SCIF range (0–4).
    #[test]
    fn channels_in_range() {
        assert!(MIDI_CH < rza1_hal::uart::NUM_CHANNELS);
        assert!(PIC_CH < rza1_hal::uart::NUM_CHANNELS);
    }
}
