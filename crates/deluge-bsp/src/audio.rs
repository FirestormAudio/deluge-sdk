//! The codec's audio: SSI0 straight to the codec ([`init`]), or out through SCUX channel 0 with
//! DVU volume and an optional rate converter ([`init_with_scux`], [`init_with_scux_src`]).
//!
//! The rings live here, lent to the drivers through their uncached mirror. An interrupt handler
//! or a task reaches them through [`with_tx`] / [`with_rx`], which run a closure on the ring in a
//! critical section: keep it short. Both return `None` before an `init` (or when re-entered).

use core::cell::{Cell, RefCell};

use critical_section::Mutex;
use rza1_hal::Peri;
use rza1_hal::dma_buf::UncachedMut;
use rza1_hal::dmac::{BlockDma, BlockInterruptHandler};
use rza1_hal::gpio::{Alternate, Flex, Level, Output};
use rza1_hal::ostm;
use rza1_hal::ring_buffer::{BlockSignal, ReadableRing, RingBuf, RingDesc, RingDescs, WritableRing};
use rza1_hal::scux::{AudioInfo, Ch0, Channel, Dvu, Src};
use rza1_hal::ssi::{Ssi, SsiRx, SsiTx};

use crate::board::{AudioPins, AudioSsi, ScuxFfd0Dma, SsiRxDma, SsiTxDma};
use crate::scux_dvu_path;

rza1_hal::bind_interrupts!(struct Irqs {
    DMAINT0 => BlockInterruptHandler<ScuxFfd0Dma>;
    DMAINT6 => BlockInterruptHandler<SsiTxDma>;
    DMAINT7 => BlockInterruptHandler<SsiRxDma>;
});

/// Codec power-on delay after the SSI clocks start, as the C firmware waits.
const CODEC_POWER_DELAY_MS: u32 = 5;

/// The engine's sample rate: the SSI's frame rate.
const SSI_HZ: u32 = 44_100;

/// SSI transmit ring: 1024 stereo frames in 8 blocks of 128.
pub const SSI_TX_WORDS: usize = 2048;
/// SSI receive ring: 2048 stereo frames in 16 blocks of 128.
pub const SSI_RX_WORDS: usize = 4096;
/// SCUX DVU path ring: 2048 stereo frames in 16 blocks of 128.
pub const DVU_WORDS: usize = 4096;

/// A cache-line-aligned ring.
#[repr(C, align(32))]
pub(crate) struct Ring<const N: usize>(pub(crate) [u32; N]);

// SAFETY (all three): lent once, by the `init` that runs, and touched only through that loan.
static mut SSI_TX_BUF: Ring<SSI_TX_WORDS> = Ring([0; SSI_TX_WORDS]);
static mut SSI_RX_BUF: Ring<SSI_RX_WORDS> = Ring([0; SSI_RX_WORDS]);
static mut DVU_BUF: Ring<DVU_WORDS> = Ring([0; DVU_WORDS]);
// SAFETY (all three): as the rings; one descriptor per block.
static mut SSI_TX_DESCS: [RingDesc; 8] = [RingDesc::NEW; 8];
static mut SSI_RX_DESCS: [RingDesc; 16] = [RingDesc::NEW; 16];
static mut DVU_DESCS: [RingDesc; 16] = [RingDesc::NEW; 16];

/// `buf` through its uncached mirror, as a DMA ring. Its cached alias is cleaned and invalidated
/// first, defensively: no line of it may be dirty when the DMAC owns the memory (rza1-rt zeroes
/// the BSS with the D-cache off, so none should be).
///
/// # Safety
/// Called once per buffer; nothing else touches it afterwards.
pub(crate) unsafe fn lend<const N: usize>(buf: *mut Ring<N>) -> UncachedMut<'static, u32> {
    let start = buf as usize;
    #[cfg(target_os = "none")]
    // SAFETY: the buffer's own lines; nothing has used it yet.
    unsafe {
        rza1_hal::cache::dma_clean_inv_range(start, start + N * 4);
    }
    #[cfg(target_os = "none")]
    let start = start + rza1_hal::UNCACHED_MIRROR_OFFSET;
    // SAFETY: the caller's contract; the mirror is the same memory, uncached.
    let words = unsafe { core::slice::from_raw_parts_mut(start as *mut u32, N) };
    UncachedMut::new(words).expect("an audio ring's mirror is uncached memory")
}

/// `table` through its uncached mirror, as a ring's descriptors, after the same clean and
/// invalidate as [`lend`].
///
/// # Safety
/// Called once per table; nothing else touches it afterwards.
pub(crate) unsafe fn lend_descs<const N: usize>(table: *mut [RingDesc; N]) -> RingDescs<'static> {
    let start = table as usize;
    #[cfg(target_os = "none")]
    // SAFETY: the table's own lines; nothing has used it yet.
    unsafe {
        rza1_hal::cache::dma_clean_inv_range(start, start + core::mem::size_of::<[RingDesc; N]>());
    }
    #[cfg(target_os = "none")]
    let start = start + rza1_hal::UNCACHED_MIRROR_OFFSET;
    // SAFETY: the caller's contract; the mirror is the same memory, uncached.
    let descs = unsafe { core::slice::from_raw_parts_mut(start as *mut RingDesc, N) };
    RingDescs::new(descs).expect("a descriptor table's mirror is uncached memory")
}

/// Channel `ch` with its block interrupt bound, over `buf` in one block per descriptor of
/// `descs`.
fn ring<C: rza1_hal::dmac::Channel>(
    ch: Peri<'static, C>,
    buf: UncachedMut<'static, u32>,
    descs: RingDescs<'static>,
) -> RingBuf<'static>
where
    Irqs: rza1_hal::interrupt::Binding<C::Interrupt, BlockInterruptHandler<C>>,
{
    let blocks = descs.len();
    RingBuf { dma: BlockDma::new(ch, Irqs), buf, descs, blocks }
}

/// The transmit ring: the SSI's own, or the SCUX DVU path's FFD.
enum Tx {
    Ssi(SsiTx<'static>),
    Scux(WritableRing<'static>),
}

impl Tx {
    fn ring(&mut self) -> &mut WritableRing<'static> {
        match self {
            Tx::Ssi(tx) => tx,
            Tx::Scux(tx) => tx,
        }
    }
}

static TX: Mutex<RefCell<Option<Tx>>> = Mutex::new(RefCell::new(None));
static RX: Mutex<RefCell<Option<SsiRx<'static>>>> = Mutex::new(RefCell::new(None));
static RX_SIGNAL: Mutex<Cell<Option<BlockSignal>>> = Mutex::new(Cell::new(None));

/// Run `f` on what `slot` holds, in a critical section; `None` if it's empty or already in use
/// (a nested call).
pub(crate) fn with_slot<T, R>(
    slot: &Mutex<RefCell<Option<T>>>,
    f: impl FnOnce(&mut T) -> R,
) -> Option<R> {
    critical_section::with(|cs| {
        let mut held = slot.borrow(cs).try_borrow_mut().ok()?;
        held.as_mut().map(f)
    })
}

/// Run `f` on the transmit ring (what the codec plays): the SSI's, or with SCUX the DVU path's.
/// `None` before an `init`.
pub fn with_tx<R>(f: impl FnOnce(&mut WritableRing<'static>) -> R) -> Option<R> {
    with_slot(&TX, |tx| f(tx.ring()))
}

/// Run `f` on the SSI receive ring (what the codec records). `None` before an `init`.
pub fn with_rx<R>(f: impl FnOnce(&mut ReadableRing<'static>) -> R) -> Option<R> {
    with_slot(&RX, |rx| f(rx))
}

/// Fill the transmit ring with dither, 256 words per critical section (the USB interrupt
/// stays serviceable between chunks).
/// Nothing before an `init`.
pub fn fill_tx_with_dither() {
    const CHUNK: usize = 256;
    let len = with_tx(|tx| tx.len()).unwrap_or(0);
    let mut lfsr: u32 = 0xACE1;
    for start in (0..len).step_by(CHUNK) {
        with_tx(|tx| {
            for w in &mut tx.ring_mut()[start..(start + CHUNK).min(len)] {
                *w = crate::sample_fmt::dither_sample(&mut lfsr) as u32;
            }
        });
    }
}

/// Waits for the receive ring's next 128-frame block, without holding it. `None` before an
/// `init`.
pub fn rx_block_signal() -> Option<BlockSignal> {
    critical_section::with(|cs| RX_SIGNAL.borrow(cs).get())
}

fn install(tx: Tx, rx: SsiRx<'static>) {
    critical_section::with(|cs| {
        RX_SIGNAL.borrow(cs).set(Some(rx.block_signal()));
        *TX.borrow(cs).borrow_mut() = Some(tx);
        *RX.borrow(cs).borrow_mut() = Some(rx);
    });
}

/// Put AUDIO_XOUT (P7_11, the codec's master clock) on its function, and hold the codec off.
fn codec_off(xout: Peri<'static, rza1_hal::peripherals::P7_11>, power: Peri<'static, rza1_hal::peripherals::P6_12>) -> Output<'static> {
    Flex::new(xout).set_as_alternate(Alternate::Alt6);
    Output::new(power, Level::Low)
}

/// Power the codec on once its clocks run.
fn codec_on(mut power: Output<'static>) {
    ostm::delay_ms(CODEC_POWER_DELAY_MS);
    power.set_high();
    log::debug!("audio: CODEC_POWER asserted, codec active");
}

/// Bring up the codec with output through SCUX channel 0's DVU (unity volume, no rate
/// conversion): SSI0 receives into its ring; the DVU path's FFD ring ([`with_tx`]) plays.
pub fn init_with_scux(
    ssi: Peri<'static, AudioSsi>,
    pins: AudioPins,
    rx: Peri<'static, SsiRxDma>,
    ffd0: Peri<'static, ScuxFfd0Dma>,
    ch0: Channel<'static, Ch0>,
) {
    bring_up_scux(ssi, pins, rx, ffd0, ch0, Src::Bypass);
}

/// [`init_with_scux`], converting `engine_hz` to the codec's 44.1 kHz in SCUX.
pub fn init_with_scux_src(
    ssi: Peri<'static, AudioSsi>,
    pins: AudioPins,
    rx: Peri<'static, SsiRxDma>,
    ffd0: Peri<'static, ScuxFfd0Dma>,
    ch0: Channel<'static, Ch0>,
    engine_hz: u32,
) {
    bring_up_scux(ssi, pins, rx, ffd0, ch0, Src::Async { input_hz: engine_hz, output_hz: SSI_HZ });
}

fn bring_up_scux(
    ssi: Peri<'static, AudioSsi>,
    pins: AudioPins,
    rx: Peri<'static, SsiRxDma>,
    ffd0: Peri<'static, ScuxFfd0Dma>,
    ch0: Channel<'static, Ch0>,
    src: Src,
) {
    let power = codec_off(pins.xout, pins.codec_power);
    // SAFETY: each buffer and table is lent once, here.
    let (rx_buf, dvu_buf) = unsafe {
        (lend(core::ptr::addr_of_mut!(SSI_RX_BUF)), lend(core::ptr::addr_of_mut!(DVU_BUF)))
    };
    let (rx_descs, dvu_descs) = unsafe {
        (
            lend_descs(core::ptr::addr_of_mut!(SSI_RX_DESCS)),
            lend_descs(core::ptr::addr_of_mut!(DVU_DESCS)),
        )
    };
    let (out, rx) = Ssi::new_with_scux_tx(
        ssi,
        pins.sck,
        pins.ws,
        pins.tx,
        pins.rx,
        ring(rx, rx_buf, rx_descs),
        crate::system::codec_ssi_config(),
    );
    let dvu = Dvu::Volume { volume: 0x0010_0000, ramp: None, zero_cross_mute: false };
    let dvu_ring = ring(ffd0, dvu_buf, dvu_descs);
    let path = ch0.into_ssi_path(AudioInfo::STEREO_24, dvu_ring, src, dvu, out);
    let (control, tx, _) = path.split();
    scux_dvu_path::install(control);
    install(Tx::Scux(tx), rx);
    log::debug!("audio: SSI0 receive + SCUX DVU path streaming");
    codec_on(power);
}

/// Bring up the codec straight on SSI0: transmit and receive rings, no SCUX.
pub fn init(
    ssi: Peri<'static, AudioSsi>,
    pins: AudioPins,
    tx: Peri<'static, SsiTxDma>,
    rx: Peri<'static, SsiRxDma>,
) {
    let power = codec_off(pins.xout, pins.codec_power);
    // SAFETY: each buffer and table is lent once, here.
    let (tx_buf, rx_buf) = unsafe {
        (lend(core::ptr::addr_of_mut!(SSI_TX_BUF)), lend(core::ptr::addr_of_mut!(SSI_RX_BUF)))
    };
    let (tx_descs, rx_descs) = unsafe {
        (
            lend_descs(core::ptr::addr_of_mut!(SSI_TX_DESCS)),
            lend_descs(core::ptr::addr_of_mut!(SSI_RX_DESCS)),
        )
    };
    let (tx, rx) = Ssi::new(
        ssi,
        pins.sck,
        pins.ws,
        pins.tx,
        pins.rx,
        ring(tx, tx_buf, tx_descs),
        ring(rx, rx_buf, rx_descs),
        crate::system::codec_ssi_config(),
    );
    install(Tx::Ssi(tx), rx);
    log::debug!("audio: SSI0 full duplex streaming");
    codec_on(power);
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use critical_section::Mutex;

    #[test]
    fn with_tx_before_init_is_none() {
        assert!(with_tx(|_| ()).is_none());
        assert!(with_rx(|_| ()).is_none());
        assert!(rx_block_signal().is_none());
    }

    #[test]
    fn with_slot_reentered_is_none() {
        static SLOT: Mutex<RefCell<Option<u32>>> = Mutex::new(RefCell::new(Some(7)));
        let outer = with_slot(&SLOT, |v| {
            *v += 1;
            with_slot(&SLOT, |_| ()).is_none()
        });
        assert_eq!(outer, Some(true), "the nested call sees the slot taken");
        assert_eq!(with_slot(&SLOT, |v| *v), Some(8));
    }

    #[test]
    fn codec_ssi_config_is_the_bench_value() {
        assert_eq!(rza1_hal::ssi::ssicr(&crate::system::codec_ssi_config()), 0x002B_C020);
    }
}
