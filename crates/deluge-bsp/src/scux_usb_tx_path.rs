//! The Deluge's SCUX USB-rate SRC path: [`rza1l_hal::scux::usb_tx_path`] on the board's FFD2/FFU2
//! DMA channels.

pub use rza1l_hal::scux::usb_tx_path::*;

/// [`rza1l_hal::scux::usb_tx_path::init`] on the Deluge's FFD2/FFU2 DMA channels.
///
/// # Safety
/// As [`rza1l_hal::scux::usb_tx_path::init`].
pub unsafe fn init(fin_hz: u32, fout_hz: u32) {
    unsafe {
        rza1l_hal::scux::usb_tx_path::init(
            crate::system::SCUX_FFD2_DMA_CH,
            crate::system::SCUX_FFU2_DMA_CH,
            fin_hz,
            fout_hz,
        )
    }
}
