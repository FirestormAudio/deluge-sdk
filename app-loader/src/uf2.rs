//! UF2 block parsing + flash programming.
//!
//! When the host drags a `.uf2` file onto the [`crate::ghostfat`] drive, the OS
//! writes its 512-byte blocks to the synthesized volume.  Each block is a
//! self-describing [UF2] record carrying up to 476 bytes of payload and the
//! absolute target address to write it to.  [`Uf2Programmer::write_block`]
//! validates a block and, if it targets our firmware slot, erases (on first
//! touch) and programs the payload into SPI flash.
//!
//! Blocks may arrive in any order and the host may resend them, so the
//! programmer is order-independent: erase state is tracked per flash sector
//! (256 KB on the S25FL512) in a bitmap, and a sector is erased only the first
//! time any of its bytes are touched.
//!
//! The pure wire-format logic (validation, geometry, block building, the erase
//! bitmap) lives in the host-testable [`deluge_image::uf2`] crate; this module
//! is the thin hardware adapter that drives `spibsc` erase/program around it.
//!
//! [UF2]: https://github.com/microsoft/uf2

use deluge_image::uf2::{self as wire, Block, EraseMap, Slot};
use rza1l_hal::spibsc;

/// Payload bytes per UF2 block when dumping flash (one NOR page per block).
pub use deluge_image::uf2::DUMP_PAYLOAD;

/// Flash geometry of our firmware slot, built from the `spibsc` constants so the
/// wire-format crate never hardcodes a board address.
const SLOT: Slot = Slot {
    flash_base: spibsc::SPI_FLASH_BASE,
    addr: spibsc::FLASH_SLOT_ADDR,
    len: spibsc::FLASH_SLOT_LEN,
    sector_size: spibsc::SECTOR_SIZE,
};

/// Number of flash sectors (256 KB each) in the app slot.
const SLOT_SECTORS: usize = (spibsc::FLASH_SLOT_LEN / spibsc::SECTOR_SIZE) as usize;
/// Erase-tracking bitmap size (one bit per slot sector).
const ERASE_BITMAP_BYTES: usize = SLOT_SECTORS.div_ceil(8);

/// Outcome of feeding one host sector to [`Uf2Programmer::write_block`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Sector is not a UF2 block for us — ignore (e.g. FAT metadata).
    Ignored,
    /// Block programmed successfully.
    Programmed,
    /// Final block of the image programmed (`block_no + 1 == num_blocks`).
    Done,
    /// Block was a valid UF2 record but could not be programmed.
    Error,
}

/// Stateful UF2 → flash programmer for a single update session.
pub struct Uf2Programmer {
    /// Per-sector erase-on-first-touch tracker.
    erased: EraseMap<ERASE_BITMAP_BYTES>,
    /// Count of successfully programmed blocks (for progress).
    blocks_done: u32,
    /// Total blocks in the image (from the last valid block seen).
    num_blocks: u32,
}

impl Uf2Programmer {
    pub const fn new() -> Self {
        Self {
            erased: EraseMap::new(),
            blocks_done: 0,
            num_blocks: 0,
        }
    }

    /// Programmed-blocks / total-blocks progress (total is 0 until first block).
    pub fn progress(&self) -> (u32, u32) {
        (self.blocks_done, self.num_blocks)
    }

    /// Erase the flash sectors (256 KB) spanned by writing `len` bytes at
    /// flash-relative offset `off` that have not yet been erased this session.
    fn erase_for(&mut self, off: u32, len: u32) {
        let (first, last) = wire::sectors_for(&SLOT, off, len);
        for sector in first..=last {
            if self.erased.take(sector) {
                let sector_off = spibsc::FLASH_SLOT_OFFSET + sector * spibsc::SECTOR_SIZE;
                unsafe { spibsc::erase_sector(sector_off) };
            }
        }
    }

    /// Validate one 512-byte host sector as a UF2 block and, if it targets the
    /// firmware slot, erase-on-first-touch and program its payload.
    pub fn write_block(&mut self, data: &[u8]) -> Outcome {
        let block = match wire::classify_block(data, &SLOT) {
            Block::Ignored => return Outcome::Ignored,
            Block::Malformed => return Outcome::Error,
            Block::Valid(b) => b,
        };

        let payload = &data[block.payload.clone()];
        self.erase_for(block.flash_off, payload.len() as u32);
        unsafe { spibsc::program(block.flash_off, payload) };

        self.num_blocks = block.num_blocks;
        self.blocks_done = self.blocks_done.saturating_add(1);

        if block.is_last() {
            Outcome::Done
        } else {
            Outcome::Programmed
        }
    }
}

impl Default for Uf2Programmer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Dump (read-back) helpers — used by ghostfat to synthesize CURRENT.UF2
// ---------------------------------------------------------------------------

/// Number of UF2 blocks needed to dump `image_len` bytes.
pub fn dump_block_count(image_len: u32) -> u32 {
    wire::block_count(image_len)
}

/// Render UF2 block `index` of a flash dump of `image_len` bytes (starting at
/// the slot base) into `out` (must be ≥ 512 bytes).  Reads flash via the
/// memory-mapped window.
pub fn render_dump_block(index: u32, image_len: u32, out: &mut [u8]) {
    let total = dump_block_count(image_len);
    let off = index * DUMP_PAYLOAD;
    let addr = spibsc::FLASH_SLOT_ADDR + off;
    let payload_len = DUMP_PAYLOAD.min(image_len.saturating_sub(off)) as usize;

    // Copy the page payload from the memory-mapped flash window into a staging
    // buffer, then hand it to the shared block builder.
    let mut page = [0u8; DUMP_PAYLOAD as usize];
    for (i, b) in page[..payload_len].iter_mut().enumerate() {
        *b = unsafe { core::ptr::read_volatile((addr as *const u8).add(i)) };
    }
    wire::build_block(out, index, total, addr, &page[..payload_len]);
}
