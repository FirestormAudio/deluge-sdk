//! MSC task: exposes the inserted SD card as a USB Mass Storage block device.
//!
//! The Bulk-Only Transport / SCSI protocol lives in [`usb_classes::bot`] and the SD-card
//! [`BlockDevice`](usb_classes::bot::BlockDevice) backing in [`deluge_bsp::usb::sd_block`]; this
//! file only initialises the card and starts the transport loop.
//!
//! Throughput counters for the OLED display are re-exported from the BOT module.

use log::{info, warn};

use deluge_bsp::sd;
use deluge_bsp::usb::sd_block::SdBlock;
use rza1_hal::usb::{Rusb1EndpointIn, Rusb1EndpointOut};
use usb_classes::bot;

pub use usb_classes::bot::{RX_BYTES, TX_BYTES};

/// Bulk-Only Transport / SCSI command loop bridged to the SD card.
#[embassy_executor::task]
pub(crate) async fn msc_task(ep_in: Rusb1EndpointIn, ep_out: Rusb1EndpointOut) {
    info!("MSC: initialising SD card");
    match sd::init().await {
        Ok(()) => info!("MSC: SD ready ({} sectors)", sd::total_sectors()),
        Err(e) => warn!("MSC: SD init failed ({:?}) — will retry on demand", e),
    }

    bot::run(SdBlock, ep_in, ep_out).await
}
