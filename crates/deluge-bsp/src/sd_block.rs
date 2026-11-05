//! The SD card as an async 512-byte block device (`block-device-driver`'s [`BlockDevice`]), for
//! an async filesystem such as embedded-fatfs: each transfer is [`sd::read_sectors`] or
//! [`sd::write_sectors`], awaited, so the executor runs while it waits on the DMA.
//!
//! [`sd::init`] must have brought the card up first.
//!
//! Not to be confused with `usb::sd_block` (feature `usb-msc`), the card as a USB Mass Storage
//! medium.

use aligned::{A4, Aligned};
use block_device_driver::BlockDevice;

use crate::sd::{self, SdError};

/// The SD card, a sector at a time. Zero-sized: the card's state is [`sd`]'s.
///
/// Any 4-byte aligned blocks will do: the driver moves multi-sector transfers through its own
/// DMA bounce buffer.
pub struct SdBlockDevice;

/// `blocks` as the bytes they hold.
fn bytes(blocks: &[Aligned<A4, [u8; 512]>]) -> &[u8] {
    // SAFETY: `Aligned<A4, [u8; 512]>` is the 512-byte array with an alignment marker of size
    // zero, so the slice is `blocks.len() * 512` contiguous bytes, borrowed as long as `blocks`.
    unsafe { core::slice::from_raw_parts(blocks.as_ptr().cast(), blocks.len() * 512) }
}

/// `blocks` as the bytes they hold, to write.
fn bytes_mut(blocks: &mut [Aligned<A4, [u8; 512]>]) -> &mut [u8] {
    // SAFETY: as `bytes`, from the exclusive borrow.
    unsafe { core::slice::from_raw_parts_mut(blocks.as_mut_ptr().cast(), blocks.len() * 512) }
}

impl BlockDevice<512> for SdBlockDevice {
    type Error = SdError;
    type Align = A4;

    async fn read(
        &mut self,
        block_address: u32,
        data: &mut [Aligned<A4, [u8; 512]>],
    ) -> Result<(), SdError> {
        let count = data.len() as u32;
        sd::read_sectors(block_address, count, bytes_mut(data)).await
    }

    async fn write(
        &mut self,
        block_address: u32,
        data: &[Aligned<A4, [u8; 512]>],
    ) -> Result<(), SdError> {
        sd::write_sectors(block_address, data.len() as u32, bytes(data)).await
    }

    async fn size(&mut self) -> Result<u64, SdError> {
        Ok(sd::total_sectors() as u64 * 512)
    }
}
