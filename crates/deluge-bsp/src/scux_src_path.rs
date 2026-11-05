//! The sample-playback (SRC) path: SCUX channel 1 (FFD1 → SRC1 → FFU1), converting `fin_hz` to `fout_hz` between two rings:
//! write to [`with_tx`], read the converted stream from [`with_rx`].

use core::cell::RefCell;

use critical_section::Mutex;
use rza1_hal::Peri;
use rza1_hal::dmac::{BlockDma, BlockInterruptHandler};
use rza1_hal::ring_buffer::{ReadableRing, RingBuf, RingDesc, WritableRing};
use rza1_hal::scux::{AudioInfo, Ch1, Channel, Control, Dvu, Src};

use crate::audio::{Ring, lend, lend_descs, with_slot};
use crate::board::{ScuxFfd1Dma, ScuxFfu1Dma};

rza1_hal::bind_interrupts!(struct Irqs {
    DMAINT3 => BlockInterruptHandler<ScuxFfd1Dma>;
    DMAINT5 => BlockInterruptHandler<ScuxFfu1Dma>;
});

/// Input ring: 1024 stereo frames in 8 blocks.
pub const TX_WORDS: usize = 2048;
/// Output ring: 2048 stereo frames in 16 blocks.
pub const RX_WORDS: usize = 4096;

// SAFETY: lent once, by `init`.
static mut TX_BUF: Ring<TX_WORDS> = Ring([0; TX_WORDS]);
static mut RX_BUF: Ring<RX_WORDS> = Ring([0; RX_WORDS]);
// SAFETY: lent once, by `init`; one descriptor per block.
static mut TX_DESCS: [RingDesc; 8] = [RingDesc::NEW; 8];
static mut RX_DESCS: [RingDesc; 16] = [RingDesc::NEW; 16];

static CONTROL: Mutex<RefCell<Option<Control<'static>>>> = Mutex::new(RefCell::new(None));
static TX: Mutex<RefCell<Option<WritableRing<'static>>>> = Mutex::new(RefCell::new(None));
static RX: Mutex<RefCell<Option<ReadableRing<'static>>>> = Mutex::new(RefCell::new(None));

/// Start the path: async SRC from `fin_hz` to `fout_hz`, no volume processing.
pub fn init(
    ch1: Channel<'static, Ch1>,
    ffd: Peri<'static, ScuxFfd1Dma>,
    ffu: Peri<'static, ScuxFfu1Dma>,
    fin_hz: u32,
    fout_hz: u32,
) {
    // SAFETY: each buffer is lent once, here.
    let (tx_buf, rx_buf) = unsafe {
        (
            lend(core::ptr::addr_of_mut!(TX_BUF)),
            lend(core::ptr::addr_of_mut!(RX_BUF)),
        )
    };
    // SAFETY: each table is lent once, here.
    let (tx_descs, rx_descs) = unsafe {
        (
            lend_descs(core::ptr::addr_of_mut!(TX_DESCS)),
            lend_descs(core::ptr::addr_of_mut!(RX_DESCS)),
        )
    };
    let path = ch1.into_ffu_path(
        AudioInfo::STEREO_24,
        RingBuf {
            dma: BlockDma::new(ffd, Irqs),
            buf: tx_buf,
            blocks: tx_descs.len(),
            descs: tx_descs,
        },
        Src::Async {
            input_hz: fin_hz,
            output_hz: fout_hz,
        },
        Dvu::Bypass,
        RingBuf {
            dma: BlockDma::new(ffu, Irqs),
            buf: rx_buf,
            blocks: rx_descs.len(),
            descs: rx_descs,
        },
    );
    let (control, tx, rx) = path.split();
    critical_section::with(|cs| {
        *CONTROL.borrow(cs).borrow_mut() = Some(control);
        *TX.borrow(cs).borrow_mut() = Some(tx);
        *RX.borrow(cs).borrow_mut() = rx;
    });
}

/// Run `f` on the input ring. `None` before [`init`].
pub fn with_tx<R>(f: impl FnOnce(&mut WritableRing<'static>) -> R) -> Option<R> {
    with_slot(&TX, f)
}

/// Run `f` on the converted output ring. `None` before [`init`].
pub fn with_rx<R>(f: impl FnOnce(&mut ReadableRing<'static>) -> R) -> Option<R> {
    with_slot(&RX, f)
}

/// Change the input rate the SRC converts from. Nothing before [`init`].
pub fn update_input_rate(fin_hz: u32) {
    with_slot(&CONTROL, |c| c.set_input_rate(fin_hz));
}
