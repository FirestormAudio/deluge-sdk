//! The Deluge's SCUX USB-rate SRC path: [`rza1l_hal::scux::usb_tx_path`] on the board's FFD2/FFU2
//! DMA channels.

pub use rza1l_hal::scux::usb_tx_path::*;

use rza1l_hal::Peri;
use rza1l_hal::dmac::Dma;

use crate::board::{ScuxFfd2Dma, ScuxFfu2Dma};

/// [`rza1l_hal::scux::usb_tx_path::init`] on the Deluge's FFD2/FFU2 DMA channels.
///
/// # Safety
/// As [`rza1l_hal::scux::usb_tx_path::init`].
pub unsafe fn init(
    ffd: Peri<'static, ScuxFfd2Dma>,
    ffu: Peri<'static, ScuxFfu2Dma>,
    fin_hz: u32,
    fout_hz: u32,
) {
    unsafe {
        rza1l_hal::scux::usb_tx_path::init(
            Dma::new_without_interrupt(ffd),
            Dma::new_without_interrupt(ffu),
            fin_hz,
            fout_hz,
        )
    }
}
