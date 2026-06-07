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
//! [UF2]: https://github.com/microsoft/uf2

use rza1l_hal::spibsc;

/// UF2 block magics.
const UF2_MAGIC_START0: u32 = 0x0A32_4655; // "UF2\n"
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;

/// Flag: bit set when word at 0x1C is a familyID (not a file size).
const UF2_FLAG_FAMILY_PRESENT: u32 = 0x0000_2000;
/// Flag: block is not for flashing (metadata only) — ignore.
const UF2_FLAG_NOT_MAIN_FLASH: u32 = 0x0000_0001;

/// Custom UF2 familyID for Deluge SSB firmware images.  RZ/A1L has no registered
/// family, so the matching `.uf2` generator (see `tools/`) must stamp this id.
/// Blocks with a *different* family present are ignored (not ours); blocks with
/// no family are also ignored to avoid programming foreign images.
pub const UF2_FAMILY_DELUGE: u32 = 0x6E27_5A1C;

/// UF2 block field offsets.
const OFF_MAGIC0: usize = 0x00;
const OFF_MAGIC1: usize = 0x04;
const OFF_FLAGS: usize = 0x08;
const OFF_TARGET: usize = 0x0C;
const OFF_PAYLOAD_SIZE: usize = 0x10;
const OFF_BLOCK_NO: usize = 0x14;
const OFF_NUM_BLOCKS: usize = 0x18;
const OFF_FAMILY: usize = 0x1C;
const OFF_DATA: usize = 0x20;
const OFF_MAGIC_END: usize = 0x1FC;
/// Maximum payload bytes a UF2 block may carry.
const UF2_MAX_PAYLOAD: usize = 476;

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

#[inline]
fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Stateful UF2 → flash programmer for a single update session.
pub struct Uf2Programmer {
    /// Bit set once the corresponding slot sector has been erased.
    erased: [u8; ERASE_BITMAP_BYTES],
    /// Count of successfully programmed blocks (for progress).
    blocks_done: u32,
    /// Total blocks in the image (from the last valid block seen).
    num_blocks: u32,
}

impl Uf2Programmer {
    pub const fn new() -> Self {
        Self {
            erased: [0; ERASE_BITMAP_BYTES],
            blocks_done: 0,
            num_blocks: 0,
        }
    }

    /// Programmed-blocks / total-blocks progress (total is 0 until first block).
    pub fn progress(&self) -> (u32, u32) {
        (self.blocks_done, self.num_blocks)
    }

    /// Erase the flash sectors (256 KB) spanned by `[off, off+len)`
    /// (flash-relative) that have not yet been erased this session.
    fn erase_for(&mut self, off: u32, len: u32) {
        let first = (off - spibsc::FLASH_SLOT_OFFSET) / spibsc::SECTOR_SIZE;
        let last = (off + len - 1 - spibsc::FLASH_SLOT_OFFSET) / spibsc::SECTOR_SIZE;
        for sector in first..=last {
            let idx = sector as usize;
            let (byte, bit) = (idx / 8, idx % 8);
            if self.erased[byte] & (1 << bit) == 0 {
                let sector_off = spibsc::FLASH_SLOT_OFFSET + sector * spibsc::SECTOR_SIZE;
                unsafe { spibsc::erase_sector(sector_off) };
                self.erased[byte] |= 1 << bit;
            }
        }
    }

    /// Validate one 512-byte host sector as a UF2 block and, if it targets the
    /// firmware slot, erase-on-first-touch and program its payload.
    pub fn write_block(&mut self, data: &[u8]) -> Outcome {
        if data.len() < 512 {
            return Outcome::Ignored;
        }
        // Magic check — anything else is FAT metadata or a non-UF2 write.
        if le_u32(data, OFF_MAGIC0) != UF2_MAGIC_START0
            || le_u32(data, OFF_MAGIC1) != UF2_MAGIC_START1
            || le_u32(data, OFF_MAGIC_END) != UF2_MAGIC_END
        {
            return Outcome::Ignored;
        }

        let flags = le_u32(data, OFF_FLAGS);
        if flags & UF2_FLAG_NOT_MAIN_FLASH != 0 {
            return Outcome::Ignored;
        }
        // Require a matching familyID so we never program a foreign image.
        if flags & UF2_FLAG_FAMILY_PRESENT == 0 || le_u32(data, OFF_FAMILY) != UF2_FAMILY_DELUGE {
            return Outcome::Ignored;
        }

        let target = le_u32(data, OFF_TARGET);
        let payload = le_u32(data, OFF_PAYLOAD_SIZE) as usize;
        if payload == 0 || payload > UF2_MAX_PAYLOAD {
            return Outcome::Error;
        }

        // Must lie entirely within the firmware slot's memory-mapped window.
        let slot_lo = spibsc::FLASH_SLOT_ADDR;
        let slot_hi = spibsc::FLASH_SLOT_ADDR + spibsc::FLASH_SLOT_LEN;
        if target < slot_lo || target as u64 + payload as u64 > slot_hi as u64 {
            return Outcome::Ignored;
        }

        let off = target - spibsc::SPI_FLASH_BASE; // flash-relative
        self.erase_for(off, payload as u32);
        unsafe { spibsc::program(off, &data[OFF_DATA..OFF_DATA + payload]) };

        self.num_blocks = le_u32(data, OFF_NUM_BLOCKS);
        let block_no = le_u32(data, OFF_BLOCK_NO);
        self.blocks_done = self.blocks_done.saturating_add(1);

        if self.num_blocks != 0 && block_no + 1 == self.num_blocks {
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

/// Payload bytes per UF2 block when dumping flash (one NOR page per block).
pub const DUMP_PAYLOAD: u32 = 256;

/// Number of UF2 blocks needed to dump `image_len` bytes.
pub fn dump_block_count(image_len: u32) -> u32 {
    image_len.div_ceil(DUMP_PAYLOAD)
}

/// Render UF2 block `index` of a flash dump of `image_len` bytes (starting at
/// the slot base) into `out` (must be ≥ 512 bytes).  Reads flash via the
/// memory-mapped window.
pub fn render_dump_block(index: u32, image_len: u32, out: &mut [u8]) {
    let total = dump_block_count(image_len);
    out[..512].fill(0);

    let addr = spibsc::FLASH_SLOT_ADDR + index * DUMP_PAYLOAD;
    let off = index * DUMP_PAYLOAD;
    let payload = DUMP_PAYLOAD.min(image_len.saturating_sub(off));

    out[OFF_MAGIC0..OFF_MAGIC0 + 4].copy_from_slice(&UF2_MAGIC_START0.to_le_bytes());
    out[OFF_MAGIC1..OFF_MAGIC1 + 4].copy_from_slice(&UF2_MAGIC_START1.to_le_bytes());
    out[OFF_FLAGS..OFF_FLAGS + 4].copy_from_slice(&UF2_FLAG_FAMILY_PRESENT.to_le_bytes());
    out[OFF_TARGET..OFF_TARGET + 4].copy_from_slice(&addr.to_le_bytes());
    out[OFF_PAYLOAD_SIZE..OFF_PAYLOAD_SIZE + 4].copy_from_slice(&payload.to_le_bytes());
    out[OFF_BLOCK_NO..OFF_BLOCK_NO + 4].copy_from_slice(&index.to_le_bytes());
    out[OFF_NUM_BLOCKS..OFF_NUM_BLOCKS + 4].copy_from_slice(&total.to_le_bytes());
    out[OFF_FAMILY..OFF_FAMILY + 4].copy_from_slice(&UF2_FAMILY_DELUGE.to_le_bytes());

    // Copy the page payload straight from the memory-mapped flash window.
    for i in 0..payload as usize {
        out[OFF_DATA + i] = unsafe { core::ptr::read_volatile((addr as *const u8).add(i)) };
    }

    out[OFF_MAGIC_END..OFF_MAGIC_END + 4].copy_from_slice(&UF2_MAGIC_END.to_le_bytes());
}
