//! The Deluge's SCUX async SRC path: [`rza1l_hal::scux::src_path`] on the board's FFD1/FFU1
//! DMA channels.

pub use rza1l_hal::scux::src_path::*;

use rza1l_hal::Peri;
use rza1l_hal::dmac::Dma;

use crate::board::{ScuxFfd1Dma, ScuxFfu1Dma};

/// [`rza1l_hal::scux::src_path::init`] on the Deluge's FFD1/FFU1 DMA channels.
///
/// # Safety
/// As [`rza1l_hal::scux::src_path::init`].
pub unsafe fn init(
    ffd: Peri<'static, ScuxFfd1Dma>,
    ffu: Peri<'static, ScuxFfu1Dma>,
    fin_hz: u32,
    fout_hz: u32,
) {
    unsafe {
        rza1l_hal::scux::src_path::init(
            Dma::new_without_interrupt(ffd),
            Dma::new_without_interrupt(ffu),
            fin_hz,
            fout_hz,
        )
    }
}
