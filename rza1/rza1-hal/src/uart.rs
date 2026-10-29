//! Async SCIF UART driver for RZ/A1L.
//!
//! Supports all five SCIF channels available on RZ/A1L (SCIF0–SCIF4).
//! Base addresses (stride 0x800):
//!   SCIF0 @ 0xE800_7000 … SCIF4 @ 0xE800_9000
//!
//! GIC interrupt IDs (per RZ/A1L HW Manual §A.2, devdrv_intc.h):
//!   SCIF_n: BRI = 221+n*4, ERI = 222+n*4, RXI = 223+n*4, TXI = 224+n*4
//!
//! **This module is board-agnostic.** [`Uart::new`] takes the board's TxD and RxD pins
//! ([`TxPin`], [`RxPin`]) and puts them on the SCIF's function itself.
//!
//! ## Usage
//! ```ignore
//! rza1_hal::bind_interrupts!(struct Irqs {
//!     ERI0 => uart::ErrorInterruptHandler<SCIF0>;
//!     TXI0 => uart::TxInterruptHandler<SCIF0>;
//!     DMAINT13 => dmac::InterruptHandler<DMA_CH13>;
//! });
//! let rx_dma = dmac::Dma::new(p.DMA_CH13, Irqs);
//! let (mut rx, tx) = uart::Uart::new(p.SCIF0, p.P6_15, p.P6_14, rx_dma, None, Irqs, 31_250).split();
//! tx.write(b"hello").await;
//! let b = rx.read_byte().await;
//! ```

use core::convert::Infallible;
use core::future::poll_fn;
use core::task::Poll;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::waitqueue::AtomicWaker;

use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use crate::Peri;
use crate::dmac::Dma;
use crate::gic;
use crate::interrupt::{self, Binding, Handler, Interrupt};
use crate::mode::{Async, Blocking, Mode};
use embedded_hal_nb::nb;

use crate::pac::scif::regs::{Brr, Emr, Fcr, Fsr, Ftdr, Lsr, Scr, Smr, Sptr};

// SCSCR bit masks
const TE: u16 = 1 << 5; // Transmit Enable  (SCSCR bit 5, HW manual §14.3.7)
const RE: u16 = 1 << 4; // Receive Enable   (SCSCR bit 4, HW manual §14.3.7)
const RIE: u16 = 1 << 6; // Receive Interrupt Enable  (routes RX request to DMAC)
const TIE: u16 = 1 << 7; // Transmit Interrupt Enable (enables TXI)

// SCFSR bit masks
// SCFSR bit layout (RZ/A1L HW Manual §14.3.7, Table — low byte):
//   bit7 ER | bit6 TEND | bit5 TDFE | bit4 BRK | bit3 FER | bit2 PER | bit1 RDF | bit0 DR
const DR: u16 = 1 << 0; // Data Ready  (RX FIFO has data below trigger)
const RDF: u16 = 1 << 1; // Receive FIFO Data Full  (count ≥ trigger)
const PER: u16 = 1 << 2; // Parity Error
const FER: u16 = 1 << 3; // Framing Error
const BRK: u16 = 1 << 4; // Break detected
const TDFE: u16 = 1 << 5; // TX FIFO Data Empty  (count ≤ trigger)
const TEND: u16 = 1 << 6; // Transmit End  (FIFO empty and the last byte shifted out)
const ER: u16 = 1 << 7; // Receive Error

// TX FIFO depth
const TX_FIFO_SIZE: usize = 16;

// The SCIF baud-rate generator runs from P1φ (see `crate::clocks`). SCBRR is computed with
// BGDM=1 (double-speed mode): SCBRR = round(P1φ / (16 × baud)) − 1. BGDM=1 (SCEMR bit 7)
// doubles the effective clock to the BRG, so the oversampling ratio stays at 16× against the
// nominal SCBRR formula.

/// Number of SCIF channels on RZ/A1L.
pub const NUM_CHANNELS: usize = 5;

/// Parity bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Parity {
    /// No parity bit.
    None,
    /// Even parity.
    Even,
    /// Odd parity.
    Odd,
}

/// Stop bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum StopBits {
    /// One stop bit.
    One,
    /// Two stop bits.
    Two,
}

/// A UART's line settings: always 8 data bits, asynchronous mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub struct Config {
    /// Baud rate, in bits per second.
    pub baudrate: u32,
    /// Parity bit.
    pub parity: Parity,
    /// Stop bits.
    pub stop_bits: StopBits,
}

impl Default for Config {
    /// 115 200 bps, 8N1.
    fn default() -> Self {
        Self { baudrate: 115_200, parity: Parity::None, stop_bits: StopBits::One }
    }
}

/// A receive error. The SCIF has no noise flag, so there is no noise error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum Error {
    /// A byte arrived with the receive FIFO (or the receive buffer) full, and was lost.
    Overrun,
    /// A byte's stop bit was 0.
    Framing,
    /// A byte's parity bit was wrong.
    Parity,
    /// The line was held low for longer than a frame.
    Break,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::Overrun => "receive overrun",
            Error::Framing => "framing error",
            Error::Parity => "parity error",
            Error::Break => "break",
        })
    }
}

impl core::error::Error for Error {}

impl embedded_io::Error for Error {
    fn kind(&self) -> embedded_io::ErrorKind {
        match self {
            Error::Overrun => embedded_io::ErrorKind::Other,
            _ => embedded_io::ErrorKind::InvalidData,
        }
    }
}

/// SCSMR for `config`: asynchronous, 8 data bits (CHR = 0), CKS = P1φ/1.
fn smr_for(config: &Config) -> u16 {
    let parity = match config.parity {
        Parity::None => 0,
        Parity::Even => 1 << 5,
        Parity::Odd => (1 << 5) | (1 << 4),
    };
    let stop = match config.stop_bits {
        StopBits::One => 0,
        StopBits::Two => 1 << 3,
    };
    parity | stop
}

// GIC base IDs for SCIF0; per RZ/A1L HW Manual §A.2:
//   SCIFn: BRI = 221+n*4, ERI = 222+n*4, RXI = 223+n*4, TXI = 224+n*4
// (RX uses DMA, so only ERI/TXI are registered; RXI_BASE is kept for the
//  ID-layout test, BRI is not used.)
const ERI_BASE: u16 = 222;
const RXI_BASE: u16 = 223;
const TXI_BASE: u16 = 224;

// GIC interrupt priority for UART (lower number = higher priority).

// ---------------------------------------------------------------------------
// DMA RX constants and static storage
// ---------------------------------------------------------------------------

/// DMA trigger-mode bits for SCIF: DMA_AM_FOR_SCIF = 0b010 << 8.
const DMA_AM_FOR_SCIF: u32 = 0x0200;

/// Ring-buffer size for DMA RX.  Must be a power of two ≤ 256.
///
/// (The C firmware's `PIC_RX_BUFFER_SIZE` / `MIDI_RX_BUFFER_SIZE` in
/// `cpu_specific.h` are 64.)
const DMA_RX_BUF_SIZE: usize = 256;

/// Cache-line-aligned ring buffer for one SCIF DMA RX channel.
#[repr(C, align(32))]
struct DmaRxBuf([u8; DMA_RX_BUF_SIZE]);

/// Cache-line-aligned 8-word DMA link descriptor.
#[repr(C, align(32))]
struct DmaRxDesc([u32; 8]);

// One receive ring per RxDmaInstance (SCIF0, SCIF1). SAFETY: ring n is touched only by the
// UartRx of the SCIF whose RING is n, and each SCIF is owned once.
static mut DMA_RX_BUF: [DmaRxBuf; 2] = [
    DmaRxBuf([0; DMA_RX_BUF_SIZE]),
    DmaRxBuf([0; DMA_RX_BUF_SIZE]),
];
static mut DMA_RX_DESC: [DmaRxDesc; 2] = [DmaRxDesc([0; 8]), DmaRxDesc([0; 8])];

/// Receive ring `ring`'s first byte, for the CPU. The DMAC's view of the same ring is its 32-bit
/// address ([`UartRx`]'s `base`); only on the device are the two the same number.
fn rx_ring(ring: usize) -> *mut u8 {
    // SAFETY: the address only, no reference.
    unsafe { core::ptr::addr_of_mut!(DMA_RX_BUF[ring].0[0]) as *mut u8 }
}


// ---------------------------------------------------------------------------
// DMA TX static storage
// ---------------------------------------------------------------------------

/// Staging buffer size for one DMA TX transfer.  Sized to fit the largest
/// single write (PIC `set_column_pair_rgb` = 49 bytes), rounded
/// up to the next 32-byte cache-line boundary.
const DMA_TX_BUF_SIZE: usize = 64;

/// Cache-line-aligned staging buffer for one SCIF DMA TX channel.
#[repr(C, align(32))]
struct DmaTxBuf([u8; DMA_TX_BUF_SIZE]);

// SAFETY: SCIF n's staging buffer, touched only by its UartTx under that UartTx's lock.
static mut DMA_TX_BUF: [DmaTxBuf; NUM_CHANNELS] = [
    DmaTxBuf([0; DMA_TX_BUF_SIZE]),
    DmaTxBuf([0; DMA_TX_BUF_SIZE]),
    DmaTxBuf([0; DMA_TX_BUF_SIZE]),
    DmaTxBuf([0; DMA_TX_BUF_SIZE]),
    DmaTxBuf([0; DMA_TX_BUF_SIZE]),
];

// ---------------------------------------------------------------------------
// Per-channel async state
// ---------------------------------------------------------------------------

struct UartState {
    rx_waker: AtomicWaker,
    tx_waker: AtomicWaker,
}

// Safety: AtomicWaker is Send + Sync.
unsafe impl Sync for UartState {}

static UART_STATE: [UartState; NUM_CHANNELS] = [
    UartState {
        rx_waker: AtomicWaker::new(),
        tx_waker: AtomicWaker::new(),
    },
    UartState {
        rx_waker: AtomicWaker::new(),
        tx_waker: AtomicWaker::new(),
    },
    UartState {
        rx_waker: AtomicWaker::new(),
        tx_waker: AtomicWaker::new(),
    },
    UartState {
        rx_waker: AtomicWaker::new(),
        tx_waker: AtomicWaker::new(),
    },
    UartState {
        rx_waker: AtomicWaker::new(),
        tx_waker: AtomicWaker::new(),
    },
];

// ---------------------------------------------------------------------------
// Register helpers
// ---------------------------------------------------------------------------

/// SCIF channel `ch`'s registers.
fn scif(ch: usize) -> crate::pac::scif::Scif {
    [
        crate::pac::SCIF0,
        crate::pac::SCIF1,
        crate::pac::SCIF2,
        crate::pac::SCIF3,
        crate::pac::SCIF4,
    ][ch]
}


// ---------------------------------------------------------------------------
// Baud rate calculation
// ---------------------------------------------------------------------------

/// Compute SCBRR value for `baud` at P1φ = `p1_hz`.
///
/// With BGDM=1 (double-speed mode): SCBRR = round(P1φ / (16 × baud)) − 1.
/// Integer rounding: (P1φ + 8 × baud) / (16 × baud) − 1.
fn scbrr(p1_hz: u32, baud: u32) -> u8 {
    (((p1_hz + 8 * baud) / (16 * baud)) - 1) as u8
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise SCIF channel `ch` (`0`–`4`) with `config`.
///
/// This function only programmes the SCIF peripheral registers; [`Uart::new`] muxes the pins.
///
/// # Panics
/// If [`crate::clocks::init`] has not been called.
///
/// # Safety
/// Writes to memory-mapped SCIF registers. Caller must ensure `ch` < [`NUM_CHANNELS`].
unsafe fn configure(ch: usize, config: &Config) {
    debug_assert!(ch < NUM_CHANNELS);
    let brr = scbrr(crate::clocks::get().p1_hz(), config.baudrate);
    debug!("uart: ch{} init at {} bps (SCBRR={})", ch, config.baudrate, brr);
    let sc = scif(ch);

    // 1. Stop all SCIF operations.
    sc.scr().write_value(Scr(0x0000));

    // 2. Reset TX and RX FIFOs (TFRST=bit2, RFRST=bit1).
    sc.fcr().write_value(Fcr(0x0006));

    // 3. Clear status flags: mask out reserved/sticky bits, keep ER/BRK/DR clear.
    let scfsr = sc.fsr().read().0;
    sc.fsr().write_value(Fsr(scfsr & 0xFF6E));

    // 4. Clear overrun error in SCLSR (ORER = bit 0).
    let sclsr = sc.lsr().read().0;
    sc.lsr().write_value(Lsr(sclsr & !1u16));

    // 5. Internal clock source: CKE[1:0] = 00 (already 0 from step 1).

    // 6. Async, 8 data bits, the config's parity and stop bits, CKS=00 (P_CLK/1).
    sc.smr().write_value(Smr(smr_for(config)));

    // 7. Double-speed mode: BGDM = bit 7.
    sc.emr().write_value(Emr(0x0080));

    // 8. Baud rate.
    sc.brr().write_value(Brr(brr));

    // 9. Release FIFO resets.  TX trigger=8 (TTRG=00), RX trigger=1 (RTRG=00).
    sc.fcr().write_value(Fcr(0x0000));

    // 10. Break: keep TxD high.
    let scsptr = sc.sptr().read().0;
    sc.sptr().write_value(Sptr(scsptr | 0x0003));

    // 11. Enable TX and RX; leave TIE/RIE off (enabled on demand by futures).
    sc.scr().write_value(Scr(TE | RE));
    debug!("uart: ch{} ready", ch);
}

// ---------------------------------------------------------------------------
// Baud-rate change
// ---------------------------------------------------------------------------

/// Change the baud rate of an already-initialised SCIF channel without
/// resetting the FIFOs or GIC interrupt registration.
///
/// The SCIF must briefly stop TX/RX while SCBRR is updated.  Any bytes
/// currently being shifted out will be lost; the caller must ensure the TX
/// FIFO has drained (e.g. via `Timer::after_millis`) before calling this.
///
/// # Panics
/// If [`crate::clocks::init`] has not been called.
///
/// # Safety
/// Writes to memory-mapped SCIF registers. Caller must ensure `ch` < [`NUM_CHANNELS`].
unsafe fn set_baud_regs(ch: usize, config: &Config) {
    debug_assert!(ch < NUM_CHANNELS);
    let sc = scif(ch);
    // Stop TE/RE so the change takes effect cleanly.
    sc.scr().write_value(Scr(0x0000));
    sc.smr().write_value(Smr(smr_for(config)));
    sc.brr()
        .write_value(Brr(scbrr(crate::clocks::get().p1_hz(), config.baudrate)));
    // Clear any accumulated error flags from the previous baud rate.  The TRM
    // states that ORER (overrun) is NOT cleared by toggling RE, so stale errors
    // from e.g. the baud-switch transition window must be cleared explicitly;
    // otherwise the SCIF silently discards all subsequent received bytes.
    let scfsr = sc.fsr().read().0;
    sc.fsr().write_value(Fsr(scfsr & 0xFF6E)); // clear ER/BRK/DR
    let sclsr = sc.lsr().read().0;
    sc.lsr().write_value(Lsr(sclsr & !1u16)); // clear ORER
    // Re-enable TE/RE, plus the DMA trigger-enable bits that route the SCIF
    // request to the DMAC: RIE for DMA RX, TIE for DMA TX.
    let mut scscr = TE | RE;
    if RX_DMA_ACTIVE[ch].load(Ordering::Relaxed) {
        scscr |= RIE;
    }
    if TX_DMA_ACTIVE[ch].load(Ordering::Relaxed) {
        scscr |= TIE;
    }
    sc.scr().write_value(Scr(scscr));
}

// NOTE: SCIF receive-interrupt (RXI) driven RX is intentionally NOT supported.
// On RZ/A1 the SCIF receive-FIFO-data-full request is wired to the DMAC, not the
// GIC: with RIE=1 and RDF=1 (data in the FIFO) no GIC line in the SCIF range
// (220–245) ever goes pending, so an RXI handler never fires. Verified on
// hardware; the stock Renesas/Deluge driver documents the same with
// `SCSCR = 0x00F0; // Enable "interrupt" (which actually triggers DMA)`. All RX
// therefore goes through the DMA ring ([`UartRx`]).

// ---------------------------------------------------------------------------
// Interrupt handlers
// ---------------------------------------------------------------------------

/// Clear sticky receive-error flags so the SCIF resumes forwarding bytes.
/// Writes 0 to the genuine receive-error flags (ER, BRK, FER, PER); SCLSR.ORER
/// is bit 0. Shared by the ERI and BRI handlers.
fn clear_rx_errors(ch: usize) {
    let sc = scif(ch);
    // Write 0 to the genuine receive-error flags (ER, BRK, FER, PER) to clear
    // them; preserve RDF/DR (data) and the TX flags. A status flag is only
    // cleared by writing 0 after it has been read as 1, so the read-modify-
    // write below is required.
    let scfsr = sc.fsr().read().0;
    sc.fsr().write_value(Fsr(scfsr & !(ER | BRK | FER | PER)));
    let sclsr = sc.lsr().read().0;
    sc.lsr().write_value(Lsr(sclsr & !1u16)); // ORER = bit 0
}

/// Called from GIC dispatch when ERI (framing/parity error) fires for SCIF `ch`.
/// Clears the receive-error flags so the SCIF can resume forwarding bytes —
/// without this a single framing error permanently stalls RX. Wakes the RX task.
fn on_eri(ch: usize) {
    latch_rx_errors(ch);
    clear_rx_errors(ch);
    UART_STATE[ch].rx_waker.wake();
}

// Latched receive errors, one bit each, lowest reported first.
const LATCH_OVERRUN: u8 = 1 << 0;
const LATCH_FRAMING: u8 = 1 << 1;
const LATCH_PARITY: u8 = 1 << 2;
const LATCH_BREAK: u8 = 1 << 3;

/// SCIF n's receive errors seen by its error interrupt and not yet returned by a read.
static RX_ERROR: [AtomicU8; NUM_CHANNELS] = [const { AtomicU8::new(0) }; NUM_CHANNELS];

/// Record SCIF `ch`'s receive-error flags (SCFSR PER/FER/BRK, SCLSR ORER) for its reader.
fn latch_rx_errors(ch: usize) {
    let sc = scif(ch);
    let scfsr = sc.fsr().read().0;
    let mut bits = 0;
    if sc.lsr().read().0 & 1 != 0 {
        bits |= LATCH_OVERRUN;
    }
    if scfsr & FER != 0 {
        bits |= LATCH_FRAMING;
    }
    if scfsr & PER != 0 {
        bits |= LATCH_PARITY;
    }
    if scfsr & BRK != 0 {
        bits |= LATCH_BREAK;
    }
    if bits != 0 {
        RX_ERROR[ch].fetch_or(bits, Ordering::AcqRel);
    }
}

/// Take SCIF `ch`'s highest-priority latched receive error: overrun, framing, parity, then break.
fn take_error(ch: usize) -> Option<Error> {
    let bits = RX_ERROR[ch].load(Ordering::Acquire);
    if bits == 0 {
        return None;
    }
    let bit = bits & bits.wrapping_neg();
    RX_ERROR[ch].fetch_and(!bit, Ordering::AcqRel);
    Some(match bit {
        LATCH_OVERRUN => Error::Overrun,
        LATCH_FRAMING => Error::Framing,
        LATCH_PARITY => Error::Parity,
        _ => Error::Break,
    })
}

/// Called from GIC dispatch when TXI fires for SCIF `ch`.
/// Disables TIE (stops further TXI) and wakes any pending tx future.
fn on_txi(ch: usize) {
    let sc = scif(ch);
    let scscr = sc.scr().read().0;
    sc.scr().write_value(Scr(scscr & !TIE));
    UART_STATE[ch].tx_waker.wake();
}

/// Suspend until the TX FIFO has drained below the trigger level (TDFE=1).
async fn wait_tdfe(ch: usize) {
    let sc = scif(ch);
    poll_fn(|cx| {
        UART_STATE[ch].tx_waker.register(cx.waker());

        let scfsr = sc.fsr().read().0;
        if scfsr & TDFE != 0 {
            Poll::Ready(())
        } else {
            // Enable TIE so the ISR fires when TDFE goes high.
            let scscr = sc.scr().read().0;
            sc.scr().write_value(Scr(scscr | TIE));
            // Double-check after enabling TIE to close the race.
            let scfsr = sc.fsr().read().0;
            if scfsr & TDFE != 0 {
                let scscr = sc.scr().read().0;
                sc.scr().write_value(Scr(scscr & !TIE));
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    })
    .await
}

// ---------------------------------------------------------------------------
// Owned UART
// ---------------------------------------------------------------------------

/// Whether SCIF n receives / transmits by DMA: [`set_baud_regs`] restores the request enables.
static RX_DMA_ACTIVE: [AtomicBool; NUM_CHANNELS] = [const { AtomicBool::new(false) }; NUM_CHANNELS];
static TX_DMA_ACTIVE: [AtomicBool; NUM_CHANNELS] = [const { AtomicBool::new(false) }; NUM_CHANNELS];

/// Where the CPU reads each receive ring next: an offset into the ring. A ring's owner and a stolen
/// reader ([`UartRx::steal`]) share it, so bytes one consumes the other doesn't deliver again.
static RX_READ: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];

mod sealed {
    pub trait Sealed {}
}

pin_trait!(
    /// A pin that can carry SCIF `T`'s transmit data (TxDn).
    TxPin, Instance
);
pin_trait!(
    /// A pin that can carry SCIF `T`'s receive data (RxDn).
    RxPin, Instance
);

/// A SCIF channel singleton (`peripherals::SCIF0`–`SCIF4`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–4.
    const INDEX: usize;
    /// Its receive-error interrupt, ERI<INDEX>.
    type ErrorInterrupt: Interrupt;
    /// Its transmit-FIFO-empty interrupt, TXI<INDEX>.
    type TxInterrupt: Interrupt;
}

/// A SCIF channel the HAL keeps a receive-DMA ring for: SCIF0 and SCIF1.
pub trait RxDmaInstance: Instance {
    /// Its ring.
    const RING: usize;
}

macro_rules! instances {
    ($($scif:ident = $n:literal, $eri:ident, $txi:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$scif {}
        impl Instance for crate::peripherals::$scif {
            const INDEX: usize = $n;
            type ErrorInterrupt = crate::interrupt::typelevel::$eri;
            type TxInterrupt = crate::interrupt::typelevel::$txi;
        }
    )*};
}

instances! {
    SCIF0 = 0, ERI0, TXI0;
    SCIF1 = 1, ERI1, TXI1;
    SCIF2 = 2, ERI2, TXI2;
    SCIF3 = 3, ERI3, TXI3;
    SCIF4 = 4, ERI4, TXI4;
}

impl RxDmaInstance for crate::peripherals::SCIF0 {
    const RING: usize = 0;
}
impl RxDmaInstance for crate::peripherals::SCIF1 {
    const RING: usize = 1;
}

/// DMARS resource selector for SCIF `ch`'s receive request (SCIF0 = 0x62, then + 4 per channel).
fn rx_dmars(ch: usize) -> u32 {
    0x62 + 4 * ch as u32
}

/// DMARS resource selector for SCIF `ch`'s transmit request (SCIF0 = 0x61, then + 4 per channel).
fn tx_dmars(ch: usize) -> u32 {
    0x61 + 4 * ch as u32
}

/// SCIF `T`'s receive-error interrupt: clears the error flags, without which one framing error
/// stops reception for good, and wakes the reader.
pub struct ErrorInterruptHandler<T: Instance> {
    _scif: PhantomData<T>,
}

impl<T: Instance> Handler<T::ErrorInterrupt> for ErrorInterruptHandler<T> {
    unsafe fn on_interrupt() {
        on_eri(T::INDEX);
    }
}

/// SCIF `T`'s transmit interrupt: wakes a writer waiting for FIFO space.
pub struct TxInterruptHandler<T: Instance> {
    _scif: PhantomData<T>,
}

impl<T: Instance> Handler<T::TxInterrupt> for TxInterruptHandler<T> {
    unsafe fn on_interrupt() {
        on_txi(T::INDEX);
    }
}

/// A SCIF UART, 8N1: it receives into a DMA ring and transmits by DMA or through the FIFO.
/// [`split`](Self::split) it into its halves.
pub struct Uart<'d, M: Mode> {
    rx: UartRx<'d, M>,
    tx: UartTx<'d, M>,
}

impl<'d> Uart<'d, Async> {
    /// Bring SCIF `T` up with `config` on pins `tx` and `rx`: receive into a ring on `rx_dma`;
    /// transmit by DMA on `tx_dma`, or through the FIFO and TXI without one.
    ///
    /// The pins are put on the SCIF's function first, so the ring never samples an unconnected
    /// pin.
    ///
    /// # Panics
    /// If [`crate::clocks::init`] has not been called.
    pub fn new<T: RxDmaInstance>(
        _scif: Peri<'d, T>,
        tx: Peri<'d, impl TxPin<T>>,
        rx: Peri<'d, impl RxPin<T>>,
        rx_dma: Dma<'d>,
        tx_dma: Option<Dma<'d>>,
        irq: impl Binding<T::ErrorInterrupt, ErrorInterruptHandler<T>>
        + Binding<T::TxInterrupt, TxInterruptHandler<T>>
        + Copy
        + 'd,
        config: Config,
    ) -> Self {
        let (tx_alt, rx_alt) = (tx.alternate(), rx.alternate());
        crate::gpio::mux(tx, tx_alt);
        crate::gpio::mux(rx, rx_alt);
        // SAFETY: SCIF T is owned.
        unsafe { configure(T::INDEX, &config) };
        interrupt::bind::<T::ErrorInterrupt, ErrorInterruptHandler<T>>(irq);
        interrupt::bind::<T::TxInterrupt, TxInterruptHandler<T>>(irq);
        // SAFETY: SCIF T's own interrupts. ERI is enabled although reception is by DMA: receive
        // errors still reach the GIC, and one left uncleared stops reception for good. Their
        // priorities are the application's.
        unsafe {
            T::ErrorInterrupt::enable();
            T::TxInterrupt::enable();
        }
        let rx = UartRx::start::<T>(rx_dma);
        let tx = UartTx::new(T::INDEX, tx_dma);
        Self { rx, tx }
    }

}

impl<'d, M: Mode> Uart<'d, M> {
    /// The receive and transmit halves.
    pub fn split(self) -> (UartRx<'d, M>, UartTx<'d, M>) {
        (self.rx, self.tx)
    }
}

/// A UART's receiver. An async one built by [`Uart::new`] reads a 256-byte ring the DMAC fills
/// from the receive FIFO, with no interrupt; a blocking one reads the receive FIFO itself.
pub struct UartRx<'d, M: Mode> {
    ch: usize,
    path: RxPath<'d>,
    _mode: PhantomData<M>,
}

/// Where a receiver's bytes come from.
enum RxPath<'d> {
    /// The receive FIFO, polled.
    Fifo,
    /// A ring the DMAC fills.
    Dma(DmaRing<'d>),
}

/// A receive ring the DMAC fills from the receive FIFO.
struct DmaRing<'d> {
    /// Its ring, which indexes `DMA_RX_BUF` and [`RX_READ`].
    ring: usize,
    dma: Dma<'d>,
    /// The ring's start as the DMAC addresses it (cached, as CRDA reports it): compared with CRDA,
    /// never dereferenced (that is [`rx_ring`]'s).
    base: u32,
}

impl DmaRing<'_> {
    /// The next byte from the ring.
    fn try_read(&mut self) -> Option<u8> {
        // One step, interrupts masked: a stolen reader running in an interrupt (a freeze raised
        // from an interrupt-priority executor) must not land between this load and store, or
        // the store would rewind past the bytes it consumed and they'd be delivered again.
        critical_section::with(|_| {
            let read = RX_READ[self.ring].load(Ordering::Acquire);
            // CRDA: where the DMAC writes next, as the DMAC addresses the ring.
            if self.dma.current_dst() == self.base.wrapping_add(read) {
                return None;
            }
            // SAFETY: `read` is an offset inside this UART's ring. Read through the uncached
            // alias: the DMAC wrote physical RAM, and a cached line may be stale.
            let byte =
                unsafe { crate::uncached(rx_ring(self.ring).add(read as usize)).read_volatile() };
            RX_READ[self.ring].store((read + 1) & (DMA_RX_BUF_SIZE as u32 - 1), Ordering::Release);
            Some(byte)
        })
    }
}

impl<'d> UartRx<'d, Async> {
    /// Start the circular receive DMA for SCIF `T` on `dma`.
    fn start<T: RxDmaInstance>(mut dma: Dma<'d>) -> Self {
        let ch = T::INDEX;
        let sc = scif(ch);
        let dma_ch = dma.number();
        // SAFETY: ring T::RING is SCIF T's alone, and SCIF T is owned. The descriptor is written
        // through the uncached alias, the address the DMAC fetches it from, so no cache
        // maintenance is needed.
        unsafe {
            let buf_base = core::ptr::addr_of!(DMA_RX_BUF[T::RING].0[0]) as u32;
            // As the C firmware's link descriptor word [4]: 0x8110_0060 | DMA_AM_FOR_SCIF |
            // the channel's index within its group.
            let chcfg: u32 = 0x8110_0060 | DMA_AM_FOR_SCIF | (dma_ch & 7) as u32;
            let desc = crate::uncached(core::ptr::addr_of_mut!(DMA_RX_DESC[T::RING]) as *mut u32);
            let desc_addr = desc as u32;
            desc.add(0).write_volatile(0b1101); // header
            desc.add(1).write_volatile(sc.frdr().addr() as u32); // source: SCFRDR
            desc.add(2).write_volatile(buf_base); // destination: the ring
            desc.add(3).write_volatile(DMA_RX_BUF_SIZE as u32); // byte count
            desc.add(4).write_volatile(chcfg);
            desc.add(5).write_volatile(0); // interval
            desc.add(6).write_volatile(0); // extension
            desc.add(7).write_volatile(desc_addr); // next: itself, so the ring loops
            // Polled, never awaited: no completion interrupt, as the descriptor and CHCFG also say.
            dma.mask_interrupt();
            // The pointer itself, not `desc_addr`: that is the 32-bit address the DMAC follows, and
            // on a 64-bit host it no longer points at the descriptor.
            dma.init_with_link_descriptor(desc, rx_dmars(ch));
            dma.channel_start();
            // The read position before the ring counts as active: a stolen reader
            // ([`UartRx::steal`]) that sees it active must also see where to read.
            RX_READ[T::RING].store(0, Ordering::Release);
            RX_DMA_ACTIVE[ch].store(true, Ordering::Release);
            // TIE|RIE|TE|RE: route both requests; the GIC's RXI stays off, so receive requests go
            // only to the DMAC.
            sc.scr().write_value(Scr(TIE | RIE | RE | TE));
            debug!(
                "uart: ch{} DMA RX active (dma_ch={}, buf={:#010x})",
                ch,
                dma_ch,
                buf_base
            );
            Self {
                ch,
                path: RxPath::Dma(DmaRing { ring: T::RING, dma, base: buf_base }),
                _mode: PhantomData,
            }
        }
    }

    /// The next received byte, or `None` if the ring is empty. A receive error the error
    /// interrupt latched since the last read comes first, once; the bytes received around it stay
    /// readable after it.
    pub fn try_read(&mut self) -> Result<Option<u8>, Error> {
        if let Some(e) = take_error(self.ch) {
            return Err(e);
        }
        Ok(self.try_read_ring())
    }

    /// The next byte from the ring, ignoring latched errors.
    fn try_read_ring(&mut self) -> Option<u8> {
        match &mut self.path {
            RxPath::Dma(ring) => ring.try_read(),
            // An async receiver is never built on the FIFO.
            RxPath::Fifo => None,
        }
    }

    /// The ring's start as the DMAC addresses it.
    #[cfg(test)]
    fn ring_base(&self) -> u32 {
        match &self.path {
            RxPath::Dma(ring) => ring.base,
            RxPath::Fifo => unreachable!("not a ring"),
        }
    }

    /// SCIF `T`'s receiver, read through its ring's DMA channel `C`, without the owner: for a
    /// fault or freeze path whose owner is suspended mid-read. It reads from the owner's position,
    /// so a byte it consumes isn't delivered to the owner again. `None` until T's ring has
    /// started ([`Uart::new`]).
    ///
    /// # Safety
    /// `C` is the channel T's ring runs on, and the owner doesn't read T's ring while the stolen
    /// receiver is in use: interrupts are masked, the owner's executor is blocked in the caller,
    /// or the caller is an interrupt (an interrupt-priority executor) that preempted the owner.
    /// [`try_read`](Self::try_read) masks interrupts across its update, so an owner preempted in
    /// it never rewinds what the stolen receiver consumed.
    #[doc(hidden)]
    #[cfg(any(feature = "unstable-driver-steal", test))]
    pub unsafe fn steal<T: RxDmaInstance, C: crate::dmac::Channel>() -> Option<UartRx<'static, Async>> {
        if !RX_DMA_ACTIVE[T::INDEX].load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the ring's address only; the caller's contract covers its use.
        let base = unsafe { core::ptr::addr_of!(DMA_RX_BUF[T::RING].0[0]) as u32 };
        Some(UartRx {
            ch: T::INDEX,
            path: RxPath::Dma(DmaRing {
                ring: T::RING,
                // SAFETY: the caller's contract: only this reader uses the channel meanwhile, and
                // it only reads CRDA.
                dma: unsafe { Dma::steal::<C>() },
                base,
            }),
            _mode: PhantomData,
        })
    }

    /// Wait for the next received byte, or a receive error ([`try_read`](Self::try_read)). The
    /// ring raises no interrupt, so this polls it every 100 µs.
    pub async fn read_byte(&mut self) -> Result<u8, Error> {
        loop {
            if let Some(byte) = self.try_read()? {
                return Ok(byte);
            }
            embassy_time::Timer::after_micros(100).await;
        }
    }
}

/// A UART's transmitter. Its methods take `&self`; whole writes don't interleave.
pub struct UartTx<'d, M: Mode> {
    ch: usize,
    /// The transmit DMA channel, if any. The lock also keeps two writers' bytes apart.
    dma: Mutex<CriticalSectionRawMutex, Option<Dma<'d>>>,
    _mode: PhantomData<M>,
}

impl<'d> UartTx<'d, Async> {
    fn new(ch: usize, mut dma: Option<Dma<'d>>) -> Self {
        if let Some(dma) = dma.as_mut() {
            let sc = scif(ch);
            let ftdr = sc.ftdr().addr();
            // As the C firmware's DMA_SCIF_TX_CONFIG: 8-bit, source increments, destination
            // fixed (FTDR), destination-side request; AM=2 so SCIF TDFE paces it.
            let chcfg = 0x0020_0068_u32 | DMA_AM_FOR_SCIF | (dma.number() as u32 & 7);
            // SAFETY: every transfer writes SCIF ch's transmit FIFO, which this UART owns.
            unsafe {
                dma.init_register_mode(chcfg, ftdr as u32, tx_dmars(ch));
                TX_DMA_ACTIVE[ch].store(true, Ordering::Relaxed);
                // TIE is both the TXI enable and the DMAC request; OR it in, keeping RE/RIE.
                let scscr = sc.scr().read().0;
                sc.scr().write_value(Scr(scscr | TIE | TE));
                // TXI's handler clears TIE, which would stall the DMAC: mask TXI at the GIC.
                gic::disable(TXI_BASE + (ch as u16) * 4);
            }
            info!("uart: ch{} DMA TX active (dma_ch={})", ch, dma.number());
        }
        Self { ch, dma: Mutex::new(dma), _mode: PhantomData }
    }

    /// Send `buf`: by DMA, one transfer per 64 bytes, waking once per transfer; or without a DMA
    /// channel, by filling the FIFO and waking on TXI as it drains.
    pub async fn write(&self, buf: &[u8]) {
        let mut dma = self.dma.lock().await;
        let Some(dma) = dma.as_mut() else {
            let mut pos = 0;
            while pos < buf.len() {
                pos += self.try_write(&buf[pos..]);
                if pos < buf.len() {
                    // FIFO full: wait for TDFE (count ≤ trigger) through TXI.
                    wait_tdfe(self.ch).await;
                }
            }
            return;
        };
        let mut pos = 0;
        while pos < buf.len() {
            let chunk = (buf.len() - pos).min(DMA_TX_BUF_SIZE);
            // SAFETY: SCIF ch's staging buffer, used only under this lock, written through the
            // uncached alias so the DMAC reads fresh bytes; its address is the bus address.
            unsafe {
                let staging = crate::uncached(core::ptr::addr_of_mut!(DMA_TX_BUF[self.ch].0[0]));
                core::ptr::copy_nonoverlapping(buf.as_ptr().add(pos), staging, chunk);
                dma.start_transfer(staging as u32, chunk as u32);
            }
            dma.wait_transfer_complete().await;
            pos += chunk;
        }
    }

    /// Wait until everything written so far has left the shift register (TEND), so a
    /// [`set_config`](Self::set_config) after it can't garble the tail. No interrupt reports TEND,
    /// so this polls it every 100 µs; it holds the writer lock meanwhile, so no write starts.
    pub async fn flush(&self) {
        let _writer = self.dma.lock().await;
        let sc = scif(self.ch);
        while sc.fsr().read().0 & TEND == 0 {
            embassy_time::Timer::after_micros(100).await;
        }
    }

    /// Put as much of `buf` in the transmit FIFO as fits now, and return how much that was.
    /// Doesn't wait: for short, latency-sensitive messages such as a MIDI clock byte.
    pub fn try_write(&self, buf: &[u8]) -> usize {
        let free = self.tx_free();
        let n = buf.len().min(free);
        for &byte in &buf[..n] {
            scif(self.ch).ftdr().write_value(Ftdr(byte));
        }
        n
    }

}

impl<'d, M: Mode> UartTx<'d, M> {
    /// Free space in the transmit FIFO, in bytes: how many bytes fit without waiting.
    pub fn tx_free(&self) -> usize {
        let in_fifo = ((scif(self.ch).fdr().read().0 >> 8) & 0x1F) as usize;
        TX_FIFO_SIZE.saturating_sub(in_fifo)
    }

    /// Change the line settings (baud rate, parity, stop bits). TX and RX stop briefly: wait for
    /// the transmitter to drain first (a flush), or its bytes go out wrong. The DMA request
    /// enables are kept.
    pub fn set_config(&self, config: &Config) {
        // SAFETY: the owned SCIF.
        unsafe { set_baud_regs(self.ch, config) }
    }
}

// ── Blocking mode ─────────────────────────────────────────────────────────────


impl<'d> Uart<'d, Blocking> {
    /// Bring SCIF `T` up with `config` on pins `tx` and `rx`, polled: no interrupt, no DMA. Any
    /// SCIF; the receive FIFO holds 16 bytes, so read it often enough not to overrun.
    ///
    /// # Panics
    /// If [`crate::clocks::init`] has not been called.
    pub fn new_blocking<T: Instance>(
        _scif: Peri<'d, T>,
        tx: Peri<'d, impl TxPin<T>>,
        rx: Peri<'d, impl RxPin<T>>,
        config: Config,
    ) -> Self {
        let (tx_alt, rx_alt) = (tx.alternate(), rx.alternate());
        crate::gpio::mux(tx, tx_alt);
        crate::gpio::mux(rx, rx_alt);
        // SAFETY: SCIF T is owned.
        unsafe { configure(T::INDEX, &config) };
        let ch = T::INDEX;
        Self {
            rx: UartRx { ch, path: RxPath::Fifo, _mode: PhantomData },
            tx: UartTx { ch, dma: Mutex::new(None), _mode: PhantomData },
        }
    }
}

/// Whether SCIF `ch` reports a receive error: SCFSR ER/BRK/FER/PER or SCLSR ORER.
fn rx_error_flags(ch: usize) -> bool {
    let sc = scif(ch);
    sc.fsr().read().0 & (ER | BRK | FER | PER) != 0 || sc.lsr().read().0 & 1 != 0
}

/// Move SCIF `ch`'s receive-error flags into its latch, as the error interrupt does.
fn poll_rx_errors(ch: usize) {
    if rx_error_flags(ch) {
        latch_rx_errors(ch);
        clear_rx_errors(ch);
    }
}

/// The next byte in SCIF `ch`'s receive FIFO, clearing RDF and DR after it.
fn fifo_read(ch: usize) -> Option<u8> {
    let sc = scif(ch);
    if sc.fdr().read().0 & 0x1F == 0 {
        return None;
    }
    let byte = sc.frdr().read().0;
    // Written 0 after being read as 1, as `clear_rx_errors` does.
    let scfsr = sc.fsr().read().0;
    sc.fsr().write_value(Fsr(scfsr & !(RDF | DR)));
    Some(byte)
}

impl<'d> UartRx<'d, Blocking> {
    /// The next received byte, a receive error (overrun, framing, parity, break, each once), or
    /// `WouldBlock` if the FIFO is empty.
    pub fn nb_read(&mut self) -> nb::Result<u8, Error> {
        poll_rx_errors(self.ch);
        if let Some(e) = take_error(self.ch) {
            return Err(nb::Error::Other(e));
        }
        fifo_read(self.ch).ok_or(nb::Error::WouldBlock)
    }

    /// Wait for at least one byte, then take whatever else the FIFO holds, up to `buf.len()`. A
    /// receive error before the first byte is returned; one after it waits for the next call.
    pub fn blocking_read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        buf[0] = loop {
            match self.nb_read() {
                Ok(byte) => break byte,
                Err(nb::Error::WouldBlock) => {}
                Err(nb::Error::Other(e)) => return Err(e),
            }
        };
        let mut n = 1;
        while n < buf.len() {
            poll_rx_errors(self.ch);
            if RX_ERROR[self.ch].load(Ordering::Acquire) != 0 {
                break;
            }
            match fifo_read(self.ch) {
                Some(byte) => {
                    buf[n] = byte;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }
}

impl<'d> UartTx<'d, Blocking> {
    /// Put `byte` in the transmit FIFO, or `WouldBlock` if it is full.
    pub fn nb_write(&mut self, byte: u8) -> nb::Result<(), Infallible> {
        if self.tx_free() == 0 {
            return Err(nb::Error::WouldBlock);
        }
        scif(self.ch).ftdr().write_value(Ftdr(byte));
        Ok(())
    }

    /// Write all of `buf`, waiting for FIFO space. Returns once the last byte is in the FIFO;
    /// [`blocking_flush`](Self::blocking_flush) waits for it to leave.
    pub fn blocking_write(&mut self, buf: &[u8]) -> Result<(), Infallible> {
        for &byte in buf {
            while self.nb_write(byte).is_err() {}
        }
        Ok(())
    }

    /// Wait until everything written has left the shift register (TEND).
    pub fn blocking_flush(&mut self) {
        while scif(self.ch).fsr().read().0 & TEND == 0 {}
    }
}

impl embedded_io::ErrorType for UartRx<'_, Blocking> {
    type Error = Error;
}

impl embedded_io::Read for UartRx<'_, Blocking> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        self.blocking_read(buf)
    }
}

impl embedded_io::ReadReady for UartRx<'_, Blocking> {
    fn read_ready(&mut self) -> Result<bool, Error> {
        Ok(RX_ERROR[self.ch].load(Ordering::Acquire) != 0
            || rx_error_flags(self.ch)
            || scif(self.ch).fdr().read().0 & 0x1F != 0)
    }
}

impl embedded_io::ErrorType for UartTx<'_, Blocking> {
    type Error = Infallible;
}

impl embedded_io::Write for UartTx<'_, Blocking> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Infallible> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.tx_free() == 0 {}
        let n = buf.len().min(self.tx_free());
        for &byte in &buf[..n] {
            scif(self.ch).ftdr().write_value(Ftdr(byte));
        }
        Ok(n)
    }

    fn flush(&mut self) -> Result<(), Infallible> {
        self.blocking_flush();
        Ok(())
    }
}

impl embedded_io::WriteReady for UartTx<'_, Blocking> {
    fn write_ready(&mut self) -> Result<bool, Infallible> {
        Ok(self.tx_free() > 0)
    }
}

impl embedded_hal_nb::serial::Error for Error {
    fn kind(&self) -> embedded_hal_nb::serial::ErrorKind {
        use embedded_hal_nb::serial::ErrorKind;
        match self {
            Error::Overrun => ErrorKind::Overrun,
            Error::Framing => ErrorKind::FrameFormat,
            Error::Parity => ErrorKind::Parity,
            Error::Break => ErrorKind::Other,
        }
    }
}

impl embedded_hal_nb::serial::ErrorType for UartRx<'_, Blocking> {
    type Error = Error;
}

impl embedded_hal_nb::serial::Read<u8> for UartRx<'_, Blocking> {
    fn read(&mut self) -> nb::Result<u8, Error> {
        self.nb_read()
    }
}

impl embedded_hal_nb::serial::ErrorType for UartTx<'_, Blocking> {
    type Error = Infallible;
}

impl embedded_hal_nb::serial::Write<u8> for UartTx<'_, Blocking> {
    fn write(&mut self, byte: u8) -> nb::Result<(), Infallible> {
        self.nb_write(byte)
    }

    fn flush(&mut self) -> nb::Result<(), Infallible> {
        if scif(self.ch).fsr().read().0 & TEND == 0 {
            return Err(nb::Error::WouldBlock);
        }
        Ok(())
    }
}

// ── embedded-io-async impls ───────────────────────────────────────────────────


impl embedded_io_async::ErrorType for UartRx<'_, Async> {
    type Error = Error;
}

impl embedded_io_async::Read for UartRx<'_, Async> {
    /// At least one byte, then whatever else has arrived; or a latched receive error, if one is
    /// pending before the first byte. An error latched later waits for the next call.
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        buf[0] = self.read_byte().await?;
        let mut n = 1;
        while n < buf.len() {
            match self.try_read_ring() {
                Some(byte) => {
                    buf[n] = byte;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }
}

impl embedded_io_async::ErrorType for UartTx<'_, Async> {
    type Error = Infallible;
}

impl embedded_io_async::Write for UartTx<'_, Async> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Infallible> {
        UartTx::write(self, buf).await;
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), Infallible> {
        UartTx::flush(self).await;
        Ok(())
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::{NUM_CHANNELS, RXI_BASE, TXI_BASE, scbrr, scif};
    use crate::clocks::TEST_CLOCKS;

    #[test]
    fn num_channels_is_five() {
        assert_eq!(NUM_CHANNELS, 5);
    }

    /// MIDI baud rate (31250 bps).
    ///
    /// SCBRR = round(P1φ / (16 × 31250)) − 1
    ///       = round(66128125 / 500000) − 1
    ///       = round(132.256) − 1 = 132 − 1 = 131
    #[test]
    fn scbrr_midi_31250() {
        assert_eq!(scbrr(TEST_CLOCKS.p1_hz(), 31_250), 131);
    }

    /// 115200 bps (debug serial).
    ///
    /// SCBRR = round(66128125 / 1843200) − 1 = round(35.88) − 1 = 36 − 1 = 35
    #[test]
    fn scbrr_115200() {
        assert_eq!(scbrr(TEST_CLOCKS.p1_hz(), 115_200), 35);
    }

    /// Spot-check that the resulting baud rate error is within ±2% for MIDI.
    #[test]
    fn scbrr_midi_accuracy() {
        let p1 = TEST_CLOCKS.p1_hz();
        let divisor = scbrr(p1, 31_250) as u32 + 1;
        let actual_baud = p1 / (16 * divisor);
        let error_ppm = (actual_baud as i64 - 31_250) * 1_000_000 / 31_250;
        assert!(
            error_ppm.abs() < 20_000,
            "MIDI baud error {error_ppm} ppm exceeds 2%"
        );
    }

    #[test]
    fn config_default_is_115200_8n1() {
        let c = super::Config::default();
        assert_eq!(c.baudrate, 115_200);
        assert_eq!(c.parity, super::Parity::None);
        assert_eq!(c.stop_bits, super::StopBits::One);
    }

    /// SCSMR: CHR (bit 6) clear for 8 data bits; PE (bit 5), O/E (bit 4) and STOP (bit 3) from
    /// the config.
    #[test]
    fn configure_programs_smr_for_parity_and_stop_bits() {
        use crate::mmio::test;
        crate::clocks::init(TEST_CLOCKS);
        let smr = scif(2).smr().addr();
        let mut cfg = super::Config::default();
        cfg.parity = super::Parity::Odd;
        cfg.stop_bits = super::StopBits::Two;
        test::reset();
        unsafe { super::configure(2, &cfg) };
        assert_eq!(test::peek16(smr) & 0x78, (1 << 5) | (1 << 4) | (1 << 3));
        cfg.parity = super::Parity::Even;
        cfg.stop_bits = super::StopBits::One;
        unsafe { super::configure(2, &cfg) };
        assert_eq!(test::peek16(smr) & 0x78, 1 << 5);
    }

    #[test]
    fn uart_error_kinds() {
        use embedded_io::{Error as _, ErrorKind};
        assert_eq!(super::Error::Overrun.kind(), ErrorKind::Other);
        assert_eq!(super::Error::Framing.kind(), ErrorKind::InvalidData);
        assert_eq!(super::Error::Parity.kind(), ErrorKind::InvalidData);
        assert_eq!(super::Error::Break.kind(), ErrorKind::InvalidData);
    }

    /// GIC interrupt IDs follow the pattern: RXI_n = RXI_BASE + n*4.
    /// RZ/A1L HW Manual §A.2: SCIF0 RXI=223, SCIF1 RXI=227, ...
    #[test]
    fn scif_irq_ids() {
        for ch in 0..NUM_CHANNELS as u16 {
            assert_eq!(RXI_BASE + ch * 4, 223 + ch * 4);
            assert_eq!(TXI_BASE + ch * 4, 224 + ch * 4);
        }
    }

    #[test]
    fn the_pac_names_the_same_scif_registers() {
        for ch in 0..NUM_CHANNELS {
            let s = scif(ch);
            let base = 0xE800_7000 + ch * 0x800;
            let got = [
                (s.smr().addr(), s.smr().width()),
                (s.brr().addr(), s.brr().width()),
                (s.scr().addr(), s.scr().width()),
                (s.ftdr().addr(), s.ftdr().width()),
                (s.fsr().addr(), s.fsr().width()),
                (s.frdr().addr(), s.frdr().width()),
                (s.fcr().addr(), s.fcr().width()),
                (s.fdr().addr(), s.fdr().width()),
                (s.sptr().addr(), s.sptr().width()),
                (s.lsr().addr(), s.lsr().width()),
                (s.emr().addr(), s.emr().width()),
            ];
            assert_eq!(
                got,
                [
                    (base, 2),
                    (base + 0x04, 1),
                    (base + 0x08, 2),
                    (base + 0x0C, 1),
                    (base + 0x10, 2),
                    (base + 0x14, 1),
                    (base + 0x18, 2),
                    (base + 0x1C, 2),
                    (base + 0x20, 2),
                    (base + 0x24, 2),
                    (base + 0x28, 2),
                ],
                "SCIF{ch}"
            );
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod typed_tests {
    use super::*;
    use crate::dmac::{self, Dma};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH10, DMA_CH12, DMA_CH13, SCIF0, SCIF1};

    crate::bind_interrupts!(struct Irqs {
        ERI0 => ErrorInterruptHandler<SCIF0>;
        TXI0 => TxInterruptHandler<SCIF0>;
        ERI1 => ErrorInterruptHandler<SCIF1>;
        TXI1 => TxInterruptHandler<SCIF1>;
        DMAINT10 => dmac::InterruptHandler<DMA_CH10>;
        DMAINT12 => dmac::InterruptHandler<DMA_CH12>;
        DMAINT13 => dmac::InterruptHandler<DMA_CH13>;
    });

    /// DMAC channel `ch`'s register at `off`.
    fn chreg(ch: u8, off: usize) -> usize {
        0xE820_0000 + ch as usize * 64 + (if ch >= 8 { 0x200 } else { 0 }) + off
    }

    /// The receive state (rings, read positions, latched errors) is shared between test threads.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// SCIF1 up on its DMA ring, as the PIC link runs, with nothing received yet.
    fn scif1_rx() -> UartRx<'static, Async> {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let rx_dma = Dma::new(unsafe { DMA_CH12::steal() }, Irqs);
        let (rx, _tx) = unsafe {
            Uart::new(
                SCIF1::steal(),
                crate::peripherals::P3_15::steal(),
                crate::peripherals::P1_9::steal(),
                rx_dma,
                None,
                Irqs,
                Config::default(),
            )
        }
        .split();
        while take_error(1).is_some() {}
        test::poke32(chreg(12, 0x1C), rx.ring_base());
        rx
    }

    #[test]
    fn eri_latches_framing_and_overrun() {
        let _serial = serial();
        test::reset();
        let b = 0xE800_8000usize; // SCIF2
        test::poke16(b + 0x10, FER | ER);
        test::poke16(b + 0x24, 1);
        on_eri(2);
        assert_eq!(test::peek16(b + 0x10) & (FER | ER), 0, "SCFSR errors cleared");
        assert_eq!(test::peek16(b + 0x24) & 1, 0, "ORER cleared");
        assert_eq!(take_error(2), Some(Error::Overrun));
        assert_eq!(take_error(2), Some(Error::Framing));
        assert_eq!(take_error(2), None);
    }

    #[test]
    fn errors_are_reported_one_per_read_in_priority_order() {
        let _serial = serial();
        let mut rx = scif1_rx();
        let b = 0xE800_7800usize; // SCIF1
        test::poke16(b + 0x10, PER | BRK | ER);
        on_eri(1);
        assert_eq!(rx.try_read(), Err(Error::Parity));
        assert_eq!(rx.try_read(), Err(Error::Break));
        assert_eq!(rx.try_read(), Ok(None));
    }

    #[test]
    fn try_read_returns_ring_bytes_after_the_error() {
        let _serial = serial();
        let mut rx = scif1_rx();
        let b = 0xE800_7800usize; // SCIF1
        unsafe { rx_ring(1).write(0x5A) };
        test::poke32(chreg(12, 0x1C), rx.ring_base() + 1);
        test::poke16(b + 0x10, FER | ER);
        on_eri(1);
        assert_eq!(rx.try_read(), Err(Error::Framing));
        assert_eq!(rx.try_read(), Ok(Some(0x5A)));
        assert_eq!(rx.try_read(), Ok(None));
    }

    #[test]
    fn an_async_read_returns_a_pending_error_before_any_byte() {
        let _serial = serial();
        let mut rx = scif1_rx();
        let b = 0xE800_7800usize; // SCIF1
        unsafe { rx_ring(1).write(0x33) };
        test::poke32(chreg(12, 0x1C), rx.ring_base() + 1);
        test::poke16(b + 0x10, PER | ER);
        on_eri(1);
        let mut buf = [0u8; 4];
        let r = embassy_futures::block_on(embedded_io_async::Read::read(&mut rx, &mut buf));
        assert_eq!(r, Err(Error::Parity));
        let r = embassy_futures::block_on(embedded_io_async::Read::read(&mut rx, &mut buf));
        assert_eq!(r, Ok(1));
        assert_eq!(buf[0], 0x33);
    }

    #[test]
    fn a_uart_is_configured_bound_and_moves_both_directions_by_dma() {
        let _serial = serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let rx_dma = Dma::new(unsafe { DMA_CH13::steal() }, Irqs);
        let tx_dma = Dma::new(unsafe { DMA_CH10::steal() }, Irqs);
        let (rx, mut tx) = unsafe {
            Uart::new(
                SCIF0::steal(),
                crate::peripherals::P6_15::steal(),
                crate::peripherals::P6_14::steal(),
                rx_dma,
                Some(tx_dma),
                Irqs,
                Config { baudrate: 31_250, ..Config::default() },
            )
        }
        .split();

        // TxD0 (P6_15) and RxD0 (P6_14) on alternative 5: PFCAE6 set, PFCE6/PFC6 clear, the
        // function driving direction (PIPC6).
        let pins = (1u16 << 15) | (1 << 14);
        assert_eq!(test::peek16(0xFCFE_3A18) & pins, pins, "PFCAE6");
        assert_eq!(test::peek16(0xFCFE_3618) & pins, 0, "PFCE6");
        assert_eq!(test::peek16(0xFCFE_3518) & pins, 0, "PFC6");
        assert_eq!(test::peek16(0xFCFE_7218) & pins, pins, "PIPC6");

        let b = 0xE800_7000usize; // SCIF0
        assert_eq!(test::peek8(b + 0x04), 131, "31 250 bps");
        assert_eq!(
            test::peek16(b + 0x28),
            0x0080,
            "double-speed baud generator"
        );
        assert_eq!(
            test::peek16(b + 0x08),
            TIE | RIE | RE | TE,
            "both requests go to the DMAC"
        );
        assert!(crate::gic::handler(222).is_some(), "ERI0 bound");
        assert!(crate::gic::handler(224).is_some(), "TXI0 bound");
        // DMARS5 holds channel 10 (low half: SCIF0 transmit); DMARS6 channel 13 (high: receive).
        assert_eq!(test::peek32(0xFCFE_1014) & 0xFFFF, 0x61);
        assert_eq!(test::peek32(0xFCFE_1018) >> 16, 0x62);
        // The receive descriptor (NXLA): SCFRDR into the ring, linked back to itself. Read through
        // its own pointer: NXLA holds the DMAC's 32-bit address of it.
        let desc = unsafe { core::ptr::addr_of!(DMA_RX_DESC[0]) as *const u32 };
        assert_eq!(test::peek32(chreg(13, 0x38)), desc as u32);
        unsafe {
            assert_eq!(*desc.add(1), (b + 0x14) as u32);
            assert_eq!(*desc.add(2), rx.ring_base());
            assert_eq!(*desc.add(7), desc as u32);
        }
        assert_eq!(RX_READ[0].load(Ordering::Relaxed), 0, "the ring starts empty");
        // The ring raises no completion interrupt, so its DMAINT stays masked at the GIC as it was
        // before typed DMA; the transmit channel's is how a write learns it finished.
        // GICD_ICENABLER1 (0xE820_1184): DMAINT13 = 54 is bit 22, DMAINT10 = 51 is bit 19.
        assert_ne!(test::peek32(0xE820_1184) & (1 << 22), 0, "receive DMAINT masked");
        assert_eq!(test::peek32(0xE820_1184) & (1 << 19), 0, "transmit DMAINT left enabled");

        // A write is one DMA transfer from the staging buffer; the DMAC's TC flag (CHSTAT bit 6),
        // set here as the hardware would, completes it.
        test::poke32(chreg(10, 0x24), 1 << 6);
        embassy_futures::block_on(tx.write(b"hello"));
        assert_eq!(test::peek32(chreg(10, 0x08)), 5, "N0TB: the byte count");
        // N0SA holds the DMAC's 32-bit address of the staging buffer; read it through its pointer.
        let staged = unsafe { core::ptr::addr_of!(DMA_TX_BUF[0].0[0]) };
        assert_eq!(test::peek32(chreg(10, 0x00)), staged as u32);
        assert_eq!(unsafe { core::slice::from_raw_parts(staged, 5) }, b"hello");

        // A flush waits for the last byte to leave the shift register (SCFSR.TEND), not just for
        // the DMA to hand it to the FIFO: a baud change after it must not garble the tail.
        let scfsr_reads = || {
            test::log()
                .iter()
                .filter(|a| !a.write && a.addr == b + 0x10)
                .count()
        };
        let before = scfsr_reads();
        test::poke16(b + 0x10, 1 << 6);
        embassy_futures::block_on(embedded_io_async::Write::flush(&mut tx)).unwrap();
        assert!(scfsr_reads() > before, "flush checked TEND");

        // A baud change keeps both DMA requests enabled.
        tx.set_config(&Config { baudrate: 200_000, ..Config::default() });
        assert_eq!(
            test::peek8(b + 0x04),
            scbrr(crate::clocks::TEST_CLOCKS.p1_hz(), 200_000)
        );
        assert_eq!(test::peek16(b + 0x08), TIE | RIE | RE | TE);
    }

    #[test]
    fn a_stolen_receiver_shares_the_owners_read_position_across_the_ring_wrap() {
        let _serial = serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let rx_dma = Dma::new(unsafe { DMA_CH12::steal() }, Irqs);
        let (mut rx, _tx) = unsafe {
            Uart::new(
                SCIF1::steal(),
                crate::peripherals::P3_15::steal(),
                crate::peripherals::P1_9::steal(),
                rx_dma,
                None,
                Irqs,
                Config { baudrate: 31_250, ..Config::default() },
            )
        }
        .split();
        let ring = rx_ring(1);
        let crda = chreg(12, 0x1C);

        // Nothing received: the DMAC's write position is still the ring's start.
        test::poke32(crda, rx.ring_base());
        assert_eq!(rx.try_read(), Ok(None));

        // The DMAC fills the ring up to its last byte...
        for i in 0..DMA_RX_BUF_SIZE {
            unsafe { ring.add(i).write(i as u8) };
        }
        test::poke32(crda, rx.ring_base() + DMA_RX_BUF_SIZE as u32 - 1);
        for i in 0..DMA_RX_BUF_SIZE - 1 {
            assert_eq!(rx.try_read(), Ok(Some(i as u8)));
        }
        assert_eq!(rx.try_read(), Ok(None));

        // ...then writes the last byte and wraps round to overwrite the first.
        unsafe { ring.write(0xAA) };
        test::poke32(crda, rx.ring_base() + 1);
        assert_eq!(rx.try_read(), Ok(Some(0xFF)));
        assert_eq!(rx.try_read(), Ok(Some(0xAA)));
        assert_eq!(rx.try_read(), Ok(None));

        // A stolen receiver (a fault path's, while the owner is suspended) reads from the same
        // position: what it consumes, the owner doesn't deliver again.
        unsafe {
            ring.add(1).write(0x11);
            ring.add(2).write(0x22);
        }
        test::poke32(crda, rx.ring_base() + 3);
        let mut stolen = unsafe { UartRx::steal::<SCIF1, DMA_CH12>() }.expect("SCIF1's ring runs");
        assert_eq!(stolen.try_read(), Ok(Some(0x11)));
        assert_eq!(rx.try_read(), Ok(Some(0x22)));
        assert_eq!(stolen.try_read(), Ok(None));
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod blocking_tests {
    use super::*;
    use crate::mmio::test;

    const B: usize = 0xE800_8800; // SCIF3
    const SCSCR: usize = B + 0x08;
    const SCFSR: usize = B + 0x10;
    const SCFRDR: usize = B + 0x14;
    const SCFDR: usize = B + 0x1C;
    const SCLSR: usize = B + 0x24;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn scif3() -> (UartRx<'static, Blocking>, UartTx<'static, Blocking>) {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        while take_error(3).is_some() {}
        unsafe {
            Uart::new_blocking(
                crate::peripherals::SCIF3::steal(),
                crate::peripherals::P3_9::steal(),
                crate::peripherals::P3_10::steal(),
                Config::default(),
            )
        }
        .split()
    }

    #[test]
    fn new_blocking_enables_only_te_re() {
        let _serial = serial();
        let _uart = scif3();
        assert_eq!(test::peek16(SCSCR), TE | RE);
    }

    #[test]
    fn nb_read_would_block_on_an_empty_fifo() {
        let _serial = serial();
        let (mut rx, _tx) = scif3();
        test::poke16(SCFDR, 0);
        assert_eq!(rx.nb_read(), Err(nb::Error::WouldBlock));
    }

    #[test]
    fn nb_read_returns_a_byte_and_clears_rdf_dr() {
        let _serial = serial();
        let (mut rx, _tx) = scif3();
        test::poke16(SCFDR, 1);
        test::poke8(SCFRDR, 0x5A);
        test::poke16(SCFSR, RDF | DR);
        assert_eq!(rx.nb_read(), Ok(0x5A));
        assert_eq!(test::peek16(SCFSR) & (RDF | DR), 0);
    }

    #[test]
    fn nb_read_reports_a_framing_error_from_scfsr() {
        let _serial = serial();
        let (mut rx, _tx) = scif3();
        test::poke16(SCFDR, 1);
        test::poke16(SCFSR, FER | ER | RDF);
        assert_eq!(rx.nb_read(), Err(nb::Error::Other(Error::Framing)));
        assert_eq!(test::peek16(SCFSR) & (FER | ER), 0);
        test::poke16(SCLSR, 1);
        assert_eq!(rx.nb_read(), Err(nb::Error::Other(Error::Overrun)));
        assert_eq!(test::peek16(SCLSR) & 1, 0);
    }

    #[test]
    fn nb_write_would_block_on_a_full_fifo() {
        let _serial = serial();
        let (_rx, mut tx) = scif3();
        test::poke16(SCFDR, 16 << 8);
        assert_eq!(tx.nb_write(0x41), Err(nb::Error::WouldBlock));
        test::poke16(SCFDR, 15 << 8);
        assert_eq!(tx.nb_write(0x41), Ok(()));
        assert_eq!(test::peek8(B + 0x0C), 0x41);
    }

    #[test]
    fn blocking_read_returns_what_the_fifo_holds() {
        let _serial = serial();
        let (mut rx, _tx) = scif3();
        test::poke16(SCFDR, 1);
        test::poke8(SCFRDR, 0x7E);
        let mut buf = [0u8; 1];
        assert_eq!(embedded_io::Read::read(&mut rx, &mut buf), Ok(1));
        assert_eq!(buf, [0x7E]);
        assert_eq!(embedded_io::ReadReady::read_ready(&mut rx), Ok(true));
    }

    #[test]
    fn nb_error_kinds() {
        use embedded_hal_nb::serial::{Error as _, ErrorKind};
        assert_eq!(Error::Overrun.kind(), ErrorKind::Overrun);
        assert_eq!(Error::Framing.kind(), ErrorKind::FrameFormat);
        assert_eq!(Error::Parity.kind(), ErrorKind::Parity);
        assert_eq!(Error::Break.kind(), ErrorKind::Other);
    }
}
