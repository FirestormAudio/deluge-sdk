//! The Deluge's SCUX async SRC path: [`rza1l_hal::scux::src_path`] on the board's FFD1/FFU1
//! DMA channels.

pub use rza1l_hal::scux::src_path::*;

/// [`rza1l_hal::scux::src_path::init`] on the Deluge's FFD1/FFU1 DMA channels.
///
/// # Safety
/// As [`rza1l_hal::scux::src_path::init`].
pub unsafe fn init(fin_hz: u32, fout_hz: u32) {
    unsafe {
        rza1l_hal::scux::src_path::init(
            crate::system::SCUX_FFD1_DMA_CH,
            crate::system::SCUX_FFU1_DMA_CH,
            fin_hz,
            fout_hz,
        )
    }
}
