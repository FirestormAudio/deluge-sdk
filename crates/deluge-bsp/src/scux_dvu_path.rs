//! The Deluge's SCUX DVU output path: [`rza1l_hal::scux::dvu_path`] on the board's SSI0 and DMA
//! channels.

pub use rza1l_hal::scux::dvu_path::*;

use rza1l_hal::Peri;
use rza1l_hal::dmac::Dma;

use crate::board::{AudioSsi, ScuxFfd0Dma, SsiRxDma};

/// [`rza1l_hal::scux::dvu_path::init`] on the Deluge's SSI0, its RX DMA channel and FFD0's.
pub fn init(
    ssi: Peri<'static, AudioSsi>,
    rx: Peri<'static, SsiRxDma>,
    ffd0: Peri<'static, ScuxFfd0Dma>,
) {
    // SAFETY: SSI0, its RX channel and FFD0's channel are moved in; the DVU path's SRC0/0, DVU0
    // and FFD0 are its own, and no other path drives them.
    unsafe {
        rza1l_hal::scux::dvu_path::init(
            ssi,
            Dma::new_without_interrupt(rx),
            crate::system::SSICR,
            Dma::new_without_interrupt(ffd0),
        )
    }
}

/// [`rza1l_hal::scux::dvu_path::init_with_src`] on the Deluge's SSI0, its RX DMA channel and
/// FFD0's.
pub fn init_with_src(
    ssi: Peri<'static, AudioSsi>,
    rx: Peri<'static, SsiRxDma>,
    ffd0: Peri<'static, ScuxFfd0Dma>,
    engine_hz: u32,
) {
    // SAFETY: as in `init`.
    unsafe {
        rza1l_hal::scux::dvu_path::init_with_src(
            ssi,
            Dma::new_without_interrupt(rx),
            crate::system::SSICR,
            Dma::new_without_interrupt(ffd0),
            engine_hz,
        )
    }
}
