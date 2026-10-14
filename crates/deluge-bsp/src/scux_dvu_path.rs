//! The Deluge's SCUX DVU output path: [`rza1l_hal::scux::dvu_path`] bound to the board's SSI0
//! configuration and DMA channel.

pub use rza1l_hal::scux::dvu_path::*;

/// [`rza1l_hal::scux::dvu_path::init`] with the Deluge's SSI0 config and FFD0 DMA channel.
///
/// # Safety
/// As [`rza1l_hal::scux::dvu_path::init`].
pub unsafe fn init() {
    unsafe {
        rza1l_hal::scux::dvu_path::init(&crate::system::SSI_CONFIG, crate::system::SCUX_FFD0_DMA_CH)
    }
}

/// [`rza1l_hal::scux::dvu_path::init_with_src`] with the Deluge's SSI0 config and FFD0 DMA channel.
///
/// # Safety
/// As [`rza1l_hal::scux::dvu_path::init_with_src`].
pub unsafe fn init_with_src(engine_hz: u32) {
    unsafe {
        rza1l_hal::scux::dvu_path::init_with_src(
            &crate::system::SSI_CONFIG,
            crate::system::SCUX_FFD0_DMA_CH,
            engine_hz,
        )
    }
}
