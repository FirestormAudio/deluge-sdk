//! UF2 block wire format (<https://github.com/microsoft/uf2>).
//!
//! A UF2 file is a sequence of 512-byte self-describing blocks. This module
//! holds the *pure* logic for both directions:
//!
//! * **Reading** — [`classify_block`] validates a 512-byte block and decides
//!   whether it should be programmed, ignored (FAT metadata / foreign image),
//!   or rejected as malformed. The caller does the actual flash write.
//! * **Writing** — [`build_block`] / [`build_uf2`] render a flat image into
//!   blocks (used by the host `elf2uf2` and by the read-back "dump" path).
//! * **Erase tracking** — [`EraseMap`] implements erase-on-first-touch over the
//!   256 KB flash sectors so out-of-order / resent blocks are idempotent.
//!
//! All flash geometry is passed in via [`Slot`] so this crate never hardcodes a
//! board address; the firmware builds a `Slot` from `rza1l_hal::spibsc`.

// --- UF2 block field offsets -----------------------------------------------

/// Offset of the first magic word.
pub const OFF_MAGIC0: usize = 0x00;
/// Offset of the second magic word.
pub const OFF_MAGIC1: usize = 0x04;
/// Offset of the flags word.
pub const OFF_FLAGS: usize = 0x08;
/// Offset of the absolute target address.
pub const OFF_TARGET: usize = 0x0C;
/// Offset of the payload-size word.
pub const OFF_PAYLOAD_SIZE: usize = 0x10;
/// Offset of the block-sequence number.
pub const OFF_BLOCK_NO: usize = 0x14;
/// Offset of the total-block-count word.
pub const OFF_NUM_BLOCKS: usize = 0x18;
/// Offset of the familyID / file-size word.
pub const OFF_FAMILY: usize = 0x1C;
/// Offset of the payload data area.
pub const OFF_DATA: usize = 0x20;
/// Offset of the trailing magic word.
pub const OFF_MAGIC_END: usize = 0x1FC;

// --- UF2 magics + flags ----------------------------------------------------

/// First magic ("UF2\n").
pub const MAGIC_START0: u32 = 0x0A32_4655;
/// Second magic.
pub const MAGIC_START1: u32 = 0x9E5D_5157;
/// Trailing magic.
pub const MAGIC_END: u32 = 0x0AB1_6F30;
/// Flag: the word at [`OFF_FAMILY`] is a familyID (not a file size).
pub const FLAG_FAMILY_PRESENT: u32 = 0x0000_2000;
/// Flag: block is metadata only — never program it.
pub const FLAG_NOT_MAIN_FLASH: u32 = 0x0000_0001;

/// Custom UF2 familyID for Deluge SSB firmware images. RZ/A1L has no registered
/// family, so our generator stamps this id and we refuse any other.
pub const FAMILY_DELUGE: u32 = 0x6E27_5A1C;

/// Maximum payload a UF2 block may carry.
pub const MAX_PAYLOAD: usize = 476;
/// Payload bytes per block when *building* blocks (one NOR page). Smaller than
/// [`MAX_PAYLOAD`] so each block maps to exactly one flash page.
pub const DUMP_PAYLOAD: u32 = 256;

/// One UF2 block is always 512 bytes.
pub const BLOCK_SIZE: usize = 512;

/// Flash geometry of the firmware slot the programmer targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Slot {
    /// Base of the whole SPI-flash memory-mapped window (`SPI_FLASH_BASE`).
    pub flash_base: u32,
    /// Memory-mapped base address of the slot (`FLASH_SLOT_ADDR`).
    pub addr: u32,
    /// Length of the slot in bytes (`FLASH_SLOT_LEN`).
    pub len: u32,
    /// Erase-sector size in bytes (`SECTOR_SIZE`).
    pub sector_size: u32,
}

impl Slot {
    /// Flash-relative offset of the slot base (`addr - flash_base`).
    #[inline]
    pub const fn offset(&self) -> u32 {
        self.addr - self.flash_base
    }
    /// Exclusive upper bound of the slot's memory-mapped window.
    #[inline]
    pub const fn end(&self) -> u32 {
        self.addr + self.len
    }
    /// Number of erase sectors in the slot.
    #[inline]
    pub const fn sectors(&self) -> u32 {
        self.len / self.sector_size
    }
}

/// A validated, programmable UF2 block (the payload targets the slot).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValidBlock {
    /// Absolute target address of the payload.
    pub target: u32,
    /// Flash-relative offset of the payload (`target - flash_base`).
    pub flash_off: u32,
    /// Byte range of the payload within the 512-byte block.
    pub payload: core::ops::Range<usize>,
    /// This block's sequence number.
    pub block_no: u32,
    /// Total blocks in the image (0 if the generator left it unset).
    pub num_blocks: u32,
}

impl ValidBlock {
    /// Whether this is the final block of the image.
    #[inline]
    pub fn is_last(&self) -> bool {
        self.num_blocks != 0 && self.block_no + 1 == self.num_blocks
    }
}

/// Result of [`classify_block`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Block {
    /// Not a UF2 block for us (bad size/magic, wrong family, foreign target,
    /// or a metadata-only block) — ignore silently.
    Ignored,
    /// A valid UF2 record but malformed (e.g. payload size out of range).
    Malformed,
    /// A programmable block targeting the slot.
    Valid(ValidBlock),
}

#[inline]
fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Classify a 512-byte host sector as a UF2 block for the given [`Slot`].
///
/// This is the read-side decision that gates every flash write the on-device
/// programmer performs; it never touches hardware.
pub fn classify_block(data: &[u8], slot: &Slot) -> Block {
    if data.len() < BLOCK_SIZE {
        return Block::Ignored;
    }
    if le_u32(data, OFF_MAGIC0) != MAGIC_START0
        || le_u32(data, OFF_MAGIC1) != MAGIC_START1
        || le_u32(data, OFF_MAGIC_END) != MAGIC_END
    {
        return Block::Ignored;
    }

    let flags = le_u32(data, OFF_FLAGS);
    if flags & FLAG_NOT_MAIN_FLASH != 0 {
        return Block::Ignored;
    }
    // Require a matching familyID so we never program a foreign image.
    if flags & FLAG_FAMILY_PRESENT == 0 || le_u32(data, OFF_FAMILY) != FAMILY_DELUGE {
        return Block::Ignored;
    }

    let target = le_u32(data, OFF_TARGET);
    let payload = le_u32(data, OFF_PAYLOAD_SIZE) as usize;
    if payload == 0 || payload > MAX_PAYLOAD {
        return Block::Malformed;
    }

    // Must lie entirely within the slot's memory-mapped window.
    if target < slot.addr || target as u64 + payload as u64 > slot.end() as u64 {
        return Block::Ignored;
    }

    Block::Valid(ValidBlock {
        target,
        flash_off: target - slot.flash_base,
        payload: OFF_DATA..OFF_DATA + payload,
        block_no: le_u32(data, OFF_BLOCK_NO),
        num_blocks: le_u32(data, OFF_NUM_BLOCKS),
    })
}

/// Erase-on-first-touch tracker for the slot's flash sectors.
///
/// `N` is the bitmap size in bytes; size it with [`Slot::sectors`]`.div_ceil(8)`.
/// The on-device programmer's bitmap is `const`-sized from the spibsc geometry.
pub struct EraseMap<const N: usize> {
    erased: [u8; N],
}

impl<const N: usize> EraseMap<N> {
    /// A fresh map with nothing erased.
    pub const fn new() -> Self {
        Self { erased: [0; N] }
    }

    /// Mark sector `idx` as erased; returns `true` if this was the first touch
    /// (i.e. the caller should perform the physical erase now).
    pub fn take(&mut self, idx: u32) -> bool {
        let (byte, bit) = (idx as usize / 8, idx as usize % 8);
        if self.erased[byte] & (1 << bit) == 0 {
            self.erased[byte] |= 1 << bit;
            true
        } else {
            false
        }
    }

    /// Whether sector `idx` has already been erased this session.
    pub fn is_erased(&self, idx: u32) -> bool {
        let (byte, bit) = (idx as usize / 8, idx as usize % 8);
        self.erased[byte] & (1 << bit) != 0
    }
}

impl<const N: usize> Default for EraseMap<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Inclusive `[first, last]` slot-relative sector indices touched by writing
/// `len` bytes at flash-relative offset `off`.
pub fn sectors_for(slot: &Slot, off: u32, len: u32) -> (u32, u32) {
    let rel = off - slot.offset();
    let first = rel / slot.sector_size;
    let last = (rel + len - 1) / slot.sector_size;
    (first, last)
}

// --- block building (write side) -------------------------------------------

/// Number of [`DUMP_PAYLOAD`]-sized blocks needed for `image_len` bytes.
pub fn block_count(image_len: u32) -> u32 {
    image_len.div_ceil(DUMP_PAYLOAD)
}

/// Render one UF2 block into `out` (must be ≥ 512 bytes): header for block
/// `index` of an image of `total` blocks targeting `target`, plus `payload`.
///
/// `out` is fully overwritten (the unused payload tail is zeroed).
pub fn build_block(out: &mut [u8], index: u32, total: u32, target: u32, payload: &[u8]) {
    out[..BLOCK_SIZE].fill(0);
    put_u32(out, OFF_MAGIC0, MAGIC_START0);
    put_u32(out, OFF_MAGIC1, MAGIC_START1);
    put_u32(out, OFF_FLAGS, FLAG_FAMILY_PRESENT);
    put_u32(out, OFF_TARGET, target);
    put_u32(out, OFF_PAYLOAD_SIZE, payload.len() as u32);
    put_u32(out, OFF_BLOCK_NO, index);
    put_u32(out, OFF_NUM_BLOCKS, total);
    put_u32(out, OFF_FAMILY, FAMILY_DELUGE);
    out[OFF_DATA..OFF_DATA + payload.len()].copy_from_slice(payload);
    put_u32(out, OFF_MAGIC_END, MAGIC_END);
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    /// The real Deluge geometry (mirrors `rza1l_hal::spibsc`).
    const DELUGE_SLOT: Slot = Slot {
        flash_base: 0x1800_0000,
        addr: 0x1810_0000,
        len: 0x0030_0000,
        sector_size: 0x0004_0000,
    };

    /// Build a full UF2 byte stream from a flat image (write side).
    fn build_uf2(image: &[u8], base: u32) -> Vec<u8> {
        let total = block_count(image.len() as u32);
        let mut out = vec![0u8; total as usize * BLOCK_SIZE];
        for i in 0..total {
            let off = (i * DUMP_PAYLOAD) as usize;
            let end = (off + DUMP_PAYLOAD as usize).min(image.len());
            let blk = &mut out[i as usize * BLOCK_SIZE..(i as usize + 1) * BLOCK_SIZE];
            build_block(blk, i, total, base + off as u32, &image[off..end]);
        }
        out
    }

    // ---- constants pinned to the wire format -------------------------------

    #[test]
    fn wire_constants_are_stable() {
        // These define the on-disk format; changing them breaks every existing
        // .uf2 and must be deliberate. elf2uf2 mirrors the same values.
        assert_eq!(MAGIC_START0, 0x0A32_4655);
        assert_eq!(MAGIC_START1, 0x9E5D_5157);
        assert_eq!(MAGIC_END, 0x0AB1_6F30);
        assert_eq!(FAMILY_DELUGE, 0x6E27_5A1C);
        assert_eq!(FLAG_FAMILY_PRESENT, 0x2000);
        assert_eq!(MAX_PAYLOAD, 476);
        assert_eq!(DUMP_PAYLOAD, 256);
        assert_eq!((OFF_MAGIC0, OFF_TARGET, OFF_DATA, OFF_MAGIC_END), (0, 0x0C, 0x20, 0x1FC));
    }

    #[test]
    fn slot_geometry_helpers() {
        let s = DELUGE_SLOT;
        assert_eq!(s.offset(), 0x0010_0000);
        assert_eq!(s.end(), 0x1840_0000);
        assert_eq!(s.sectors(), 12); // 3 MB / 256 KB
    }

    // ---- classify_block: accept --------------------------------------------

    #[test]
    fn classify_accepts_valid_block() {
        let mut blk = vec![0u8; BLOCK_SIZE];
        build_block(&mut blk, 2, 5, DELUGE_SLOT.addr + 0x200, &[0xAB; 256]);
        match classify_block(&blk, &DELUGE_SLOT) {
            Block::Valid(v) => {
                assert_eq!(v.target, 0x1810_0200);
                assert_eq!(v.flash_off, 0x0010_0200);
                assert_eq!(v.payload, OFF_DATA..OFF_DATA + 256);
                assert_eq!((v.block_no, v.num_blocks), (2, 5));
                assert!(!v.is_last());
            }
            other => panic!("expected Valid, got {other:?}"),
        }
    }

    #[test]
    fn classify_marks_last_block() {
        let mut blk = vec![0u8; BLOCK_SIZE];
        build_block(&mut blk, 4, 5, DELUGE_SLOT.addr, &[0; 16]);
        match classify_block(&blk, &DELUGE_SLOT) {
            Block::Valid(v) => assert!(v.is_last()),
            other => panic!("got {other:?}"),
        }
    }

    // ---- classify_block: ignore --------------------------------------------

    fn good_block() -> Vec<u8> {
        let mut blk = vec![0u8; BLOCK_SIZE];
        build_block(&mut blk, 0, 1, DELUGE_SLOT.addr, &[0x11; 256]);
        blk
    }

    #[test]
    fn classify_ignores_short_block() {
        assert_eq!(classify_block(&[0u8; 100], &DELUGE_SLOT), Block::Ignored);
    }

    #[test]
    fn classify_ignores_bad_magic() {
        let mut blk = good_block();
        put_u32(&mut blk, OFF_MAGIC0, 0xDEAD_BEEF);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Ignored);
        let mut blk = good_block();
        put_u32(&mut blk, OFF_MAGIC_END, 0);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Ignored);
    }

    #[test]
    fn classify_ignores_not_main_flash() {
        let mut blk = good_block();
        put_u32(&mut blk, OFF_FLAGS, FLAG_FAMILY_PRESENT | FLAG_NOT_MAIN_FLASH);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Ignored);
    }

    #[test]
    fn classify_ignores_missing_or_foreign_family() {
        // Family flag clear → ignore (don't program unstamped images).
        let mut blk = good_block();
        put_u32(&mut blk, OFF_FLAGS, 0);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Ignored);
        // Foreign familyID → ignore.
        let mut blk = good_block();
        put_u32(&mut blk, OFF_FAMILY, 0x1234_5678);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Ignored);
    }

    #[test]
    fn classify_ignores_target_outside_slot() {
        // Below the slot.
        let mut blk = vec![0u8; BLOCK_SIZE];
        build_block(&mut blk, 0, 1, DELUGE_SLOT.flash_base, &[0; 256]);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Ignored);
        // Straddling the top edge.
        let mut blk = vec![0u8; BLOCK_SIZE];
        build_block(&mut blk, 0, 1, DELUGE_SLOT.end() - 128, &[0; 256]);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Ignored);
    }

    // ---- classify_block: malformed -----------------------------------------

    #[test]
    fn classify_rejects_bad_payload_size() {
        let mut blk = good_block();
        put_u32(&mut blk, OFF_PAYLOAD_SIZE, 0);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Malformed);
        let mut blk = good_block();
        put_u32(&mut blk, OFF_PAYLOAD_SIZE, (MAX_PAYLOAD + 1) as u32);
        assert_eq!(classify_block(&blk, &DELUGE_SLOT), Block::Malformed);
    }

    // ---- EraseMap ----------------------------------------------------------

    #[test]
    fn erase_map_first_touch_only() {
        let mut m = EraseMap::<2>::new();
        assert!(m.take(0), "first touch erases");
        assert!(!m.take(0), "second touch is a no-op");
        assert!(m.is_erased(0));
        assert!(!m.is_erased(1));
        assert!(m.take(11));
        assert!(m.is_erased(11));
    }

    #[test]
    fn sectors_for_spans_correctly() {
        let s = DELUGE_SLOT;
        // First byte of the slot → sector 0 only.
        assert_eq!(sectors_for(&s, s.offset(), 256), (0, 0));
        // A write crossing the 256 KB boundary touches two sectors.
        assert_eq!(sectors_for(&s, s.offset() + s.sector_size - 1, 2), (0, 1));
        // Into the second sector.
        assert_eq!(sectors_for(&s, s.offset() + s.sector_size, 256), (1, 1));
    }

    // ---- round-trip: build → classify → reassemble -------------------------

    #[test]
    fn build_then_classify_round_trips() {
        // 256*2 + 5 bytes → 3 blocks; reassemble from the classified payloads.
        let image: Vec<u8> = (0..517u32).map(|i| (i * 7) as u8).collect();
        let uf2 = build_uf2(&image, DELUGE_SLOT.addr);
        assert_eq!(uf2.len(), 3 * BLOCK_SIZE);

        let mut rebuilt = vec![0u8; image.len()];
        let mut seen_last = false;
        for blk in uf2.chunks(BLOCK_SIZE) {
            match classify_block(blk, &DELUGE_SLOT) {
                Block::Valid(v) => {
                    let dst = (v.flash_off - DELUGE_SLOT.offset()) as usize;
                    let bytes = &blk[v.payload.clone()];
                    rebuilt[dst..dst + bytes.len()].copy_from_slice(bytes);
                    seen_last |= v.is_last();
                }
                other => panic!("round-trip block rejected: {other:?}"),
            }
        }
        assert!(seen_last, "the final block must report is_last()");
        assert_eq!(rebuilt, image, "reassembled image must equal the original");
    }

    #[test]
    fn out_of_order_and_duplicate_blocks_are_idempotent() {
        // Erase-on-first-touch must not depend on block order or resends.
        let image = vec![0xCDu8; 256 * 3];
        let uf2 = build_uf2(&image, DELUGE_SLOT.addr);
        let mut m = EraseMap::<2>::new();
        let mut erases = 0;

        // Feed blocks reversed and twice over.
        let order = [2u32, 2, 0, 1, 0, 1, 2];
        for &i in &order {
            let blk = &uf2[i as usize * BLOCK_SIZE..(i as usize + 1) * BLOCK_SIZE];
            if let Block::Valid(v) = classify_block(blk, &DELUGE_SLOT) {
                let (first, last) = sectors_for(&DELUGE_SLOT, v.flash_off, 256);
                for s in first..=last {
                    if m.take(s) {
                        erases += 1;
                    }
                }
            } else {
                panic!("valid block rejected");
            }
        }
        // All three 256-byte payloads land in slot sector 0 → erased once.
        assert_eq!(erases, 1);
    }
}
