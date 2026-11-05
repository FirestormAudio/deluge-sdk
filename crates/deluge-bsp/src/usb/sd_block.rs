//! The SD card as a USB Mass Storage medium: a [`BlockDevice`] over [`crate::sd`], for the
//! board-agnostic Bulk-Only Transport engine in `usb_classes::bot`.

use usb_classes::bot::{BlockDevice, Inquiry};

/// [`BlockDevice`] backed by the inserted SD card ([`crate::sd`]).  Shared by the
/// MSC firmware and the bootloader's DATA TRANSFER mode.
pub struct SdBlock;

impl BlockDevice for SdBlock {
    fn block_count(&self) -> u32 {
        crate::sd::total_sectors()
    }

    fn is_ready(&self) -> bool {
        crate::sd::is_ready()
    }

    async fn ensure_ready(&mut self) -> bool {
        if !crate::sd::is_ready() {
            let _ = crate::sd::init().await;
        }
        crate::sd::is_ready()
    }

    async fn read(&mut self, lba: u32, count: u32, buf: &mut [u8]) -> Result<(), ()> {
        crate::sd::read_sectors(lba, count, buf)
            .await
            .map_err(|_| ())
    }

    async fn write(&mut self, lba: u32, count: u32, buf: &[u8]) -> Result<(), ()> {
        crate::sd::write_sectors(lba, count, buf)
            .await
            .map_err(|_| ())
    }

    fn inquiry(&self) -> Inquiry {
        Inquiry {
            vendor: *b"Synthstm",
            product: *b"Deluge SD Card  ",
            revision: *b"1.00",
        }
    }

    fn is_write_protected(&self) -> bool {
        crate::sd::is_write_protected()
    }
}
