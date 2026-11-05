//! The boot-slot format: the header page stage-0 checks before it enters an
//! XIP boot slot, and the slot geometry it is frozen with.
//!
//! Stage-0 is never field-updated, so everything here is frozen with it: a
//! change to the layout, a constant or an error code needs a new `VERSION` and
//! a stage-0 that understands it.
//!
//! ## Slot layout
//!
//! Slot A is flash `0x010000`, slot B `0x080000`, each [`SLOT_LEN`] long. The
//! first [`HEADER_LEN`] bytes are the header page; the body (an XIP image linked
//! to run at slot + `0x100`) follows it.
//!
//! ## Header page (all fields little-endian; every other byte is `0xFF`)
//!
//! | Offset | Field          | Value                                                        |
//! |--------|----------------|--------------------------------------------------------------|
//! | `0x00` | `magic` u32    | [`MAGIC`] (`b"DSLT"`)                                        |
//! | `0x04` | `version` u16  | [`VERSION`]                                                  |
//! | `0x06` | `variant` u16  | [`Variant`]: `0` OLED, `1` 7SEG                              |
//! | `0x08` | `sequence` u32 | higher is newer                                              |
//! | `0x0C` | `body_len` u32 | `1..=MAX_BODY_LEN`                                           |
//! | `0x10` | `body_crc` u32 | [`crc32`] of the body                                        |
//! | `0x14` | `entry` u32    | offset from the slot start; 4-aligned, the whole word inside the body |
//! | `0x18` | reserved       | `0xFF` in this version; covered by `header_crc`              |
//! | `0xF8` | `header_crc`   | [`crc32`] of bytes `0x00..0xF8`, reserved bytes included     |
//! | `0xFC` | `confirm` u32  | [`CONFIRM_PENDING`] or [`CONFIRMED`]; outside both CRCs, so the slot can confirm itself by programming one word |
//!
//! An erased page reads all `0xFF` and fails the magic check; a torn one fails
//! the header CRC.
//!
//! The header CRC covers the reserved bytes so that later fields can be added
//! there under the same [`VERSION`], protected, as long as a stage-0 that does
//! not know them may ignore them. `VERSION` changes only for a layout this
//! stage-0 cannot read, and it accepts exactly `VERSION`.
//!
//! Every length and offset read from a header is checked in `u64` before it is
//! used: the device is 32-bit and a `usize` sum of two header words can wrap.

use crate::crc::crc32;

/// Length of the header page at the start of each slot.
pub const HEADER_LEN: usize = 0x100;
/// `b"DSLT"` read as a little-endian `u32`.
pub const MAGIC: u32 = 0x544C_5344;
/// The header layout version this module reads and writes.
pub const VERSION: u16 = 1;
/// Flash offset of boot slot A.
pub const SLOT_A_OFFSET: u32 = 0x01_0000;
/// Flash offset of boot slot B.
pub const SLOT_B_OFFSET: u32 = 0x08_0000;
/// Length of each boot slot, header included.
pub const SLOT_LEN: u32 = 0x7_0000;
/// The longest body that fits in a slot after its header page.
pub const MAX_BODY_LEN: u32 = SLOT_LEN - HEADER_LEN as u32;
/// The confirm word of a slot that has not yet confirmed a good boot (erased flash).
pub const CONFIRM_PENDING: u32 = 0xFFFF_FFFF;
/// The confirm word of a slot that has booted and confirmed itself (`b"MFNC"` LE).
pub const CONFIRMED: u32 = 0x434E_464D;
/// The retention-RAM word stage-0 writes, just before entering a slot, to tell
/// it which slot it is and why it was chosen. The slot reads it, then clears it.
pub const BOOT_INFO_ADDR: usize = 0x2000_0000;
/// The tag in the top half of the boot-info word; see [`boot_info`].
pub const BOOT_INFO_TAG: u32 = 0x5330_0000;

const CONFIRM_OFFSET: usize = 0xFC;
const HEADER_CRC_OFFSET: usize = 0xF8;

/// Which front panel the slot's image is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Variant {
    /// The OLED Deluge.
    Oled = 0,
    /// The seven-segment Deluge.
    SevenSeg = 1,
}

/// A parsed header that passed every check [`parse`] makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotHeader {
    pub variant: Variant,
    pub sequence: u32,
    pub body_len: u32,
    pub body_crc: u32,
    /// Offset of the entry point from the slot start (not from the body).
    pub entry_offset: u32,
    /// Whether the confirm word reads [`CONFIRMED`]; anything else is pending.
    pub confirmed: bool,
}

/// Why a slot was refused. [`SlotError::code`] is what stage-0 reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotError {
    /// The magic is wrong: an erased or foreign page.
    BadMagic,
    /// A header version this stage-0 does not read.
    BadVersion,
    /// The header CRC does not match: a torn or corrupted page.
    HeaderCrc,
    /// A variant number that is not a [`Variant`].
    BadVariant,
    /// `body_len` is zero or does not fit in the slot.
    BodyLen,
    /// The entry is misaligned, or its instruction word is not wholly inside the body.
    Entry,
    /// The data given is shorter than the header page, or than header + body.
    Truncated,
    /// The body CRC does not match.
    BodyCrc,
}

impl SlotError {
    /// The fault code stage-0 reports (and blinks): `1` for [`SlotError::BadMagic`]
    /// through `8` for [`SlotError::BodyCrc`], in declaration order. Frozen.
    pub const fn code(self) -> u32 {
        self as u32 + 1
    }
}

/// The boot-info word for `slot` (`0` = A) entered for `reason` (`0` = it was
/// the only valid slot): `0x5330_xxyy`.
pub const fn boot_info(slot: u8, reason: u8) -> u32 {
    BOOT_INFO_TAG | (slot as u32) << 8 | reason as u32
}

fn word(page: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([page[at], page[at + 1], page[at + 2], page[at + 3]])
}

fn half(page: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([page[at], page[at + 1]])
}

fn check_body_len(body_len: u32) -> Result<(), SlotError> {
    if body_len == 0 || body_len > MAX_BODY_LEN {
        return Err(SlotError::BodyLen);
    }
    Ok(())
}

fn check_entry(entry_offset: u32, body_len: u32) -> Result<(), SlotError> {
    // The whole entry instruction must be inside the CRC-checked body.
    let end = HEADER_LEN as u64 + body_len as u64;
    let entry = entry_offset as u64;
    if entry % 4 != 0 || entry < HEADER_LEN as u64 || entry + 4 > end {
        return Err(SlotError::Entry);
    }
    Ok(())
}

/// Parse and check a header page: everything except the body CRC.
///
/// `page` must be at least [`HEADER_LEN`] bytes; a longer slice (a whole slot)
/// is fine and only its first page is read.
pub fn parse(page: &[u8]) -> Result<SlotHeader, SlotError> {
    if page.len() < HEADER_LEN {
        return Err(SlotError::Truncated);
    }
    if word(page, 0x00) != MAGIC {
        return Err(SlotError::BadMagic);
    }
    if word(page, HEADER_CRC_OFFSET) != crc32(&page[..HEADER_CRC_OFFSET]) {
        return Err(SlotError::HeaderCrc);
    }
    if half(page, 0x04) != VERSION {
        return Err(SlotError::BadVersion);
    }
    let variant = match half(page, 0x06) {
        0 => Variant::Oled,
        1 => Variant::SevenSeg,
        _ => return Err(SlotError::BadVariant),
    };
    let body_len = word(page, 0x0C);
    check_body_len(body_len)?;
    let entry_offset = word(page, 0x14);
    check_entry(entry_offset, body_len)?;
    Ok(SlotHeader {
        variant,
        sequence: word(page, 0x08),
        body_len,
        body_crc: word(page, 0x10),
        entry_offset,
        confirmed: word(page, CONFIRM_OFFSET) == CONFIRMED,
    })
}

/// Fully check a slot: [`parse`] its header, then the body's CRC.
///
/// `slot` starts at the header page and may run past the body (a whole slot
/// read through the XIP window); only `HEADER_LEN + body_len` bytes are read.
pub fn validate(slot: &[u8]) -> Result<SlotHeader, SlotError> {
    let header = parse(slot)?;
    // `body_len <= MAX_BODY_LEN` (checked by `parse`), so this cannot overflow.
    let end = HEADER_LEN + header.body_len as usize;
    let body = slot.get(HEADER_LEN..end).ok_or(SlotError::Truncated)?;
    if crc32(body) != header.body_crc {
        return Err(SlotError::BodyCrc);
    }
    Ok(header)
}

/// Build the header page for `body`, with the confirm word left pending: the
/// inverse of [`parse`], refusing anything `parse` would reject.
pub fn encode_header(
    body: &[u8],
    variant: Variant,
    sequence: u32,
    entry_offset: u32,
) -> Result<[u8; HEADER_LEN], SlotError> {
    let body_len = u32::try_from(body.len()).map_err(|_| SlotError::BodyLen)?;
    check_body_len(body_len)?;
    check_entry(entry_offset, body_len)?;
    let mut page = [0xFF; HEADER_LEN];
    page[0x00..0x04].copy_from_slice(&MAGIC.to_le_bytes());
    page[0x04..0x06].copy_from_slice(&VERSION.to_le_bytes());
    page[0x06..0x08].copy_from_slice(&(variant as u16).to_le_bytes());
    page[0x08..0x0C].copy_from_slice(&sequence.to_le_bytes());
    page[0x0C..0x10].copy_from_slice(&body_len.to_le_bytes());
    page[0x10..0x14].copy_from_slice(&crc32(body).to_le_bytes());
    page[0x14..0x18].copy_from_slice(&entry_offset.to_le_bytes());
    let crc = crc32(&page[..HEADER_CRC_OFFSET]);
    page[HEADER_CRC_OFFSET..HEADER_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc::crc32;

    fn image(body: &[u8]) -> Vec<u8> {
        let mut img = encode_header(body, Variant::Oled, 7, 0x100)
            .unwrap()
            .to_vec();
        img.extend_from_slice(body);
        img
    }

    /// Rewrite the header CRC after a test mutates a covered field.
    fn recrc(img: &mut [u8]) {
        let crc = crc32(&img[..0xF8]);
        img[0xF8..0xFC].copy_from_slice(&crc.to_le_bytes());
    }

    fn put(img: &mut [u8], at: usize, v: u32) {
        img[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    const BODY: &[u8] = b"\x01\x02\x03\x04";

    #[test]
    fn round_trips() {
        assert_eq!(
            validate(&image(BODY)),
            Ok(SlotHeader {
                variant: Variant::Oled,
                sequence: 7,
                body_len: 4,
                body_crc: crc32(BODY),
                entry_offset: 0x100,
                confirmed: false,
            })
        );
    }

    #[test]
    fn header_layout_is_frozen() {
        let img = image(BODY);
        assert_eq!(&img[0..4], b"DSLT");
        assert_eq!(&img[4..6], &[1, 0]);
        assert!(img[0x18..0xF8].iter().all(|&b| b == 0xFF));
        assert_eq!(&img[0xF8..0xFC], &crc32(&img[..0xF8]).to_le_bytes());
        assert_eq!(&img[0xFC..0x100], &CONFIRM_PENDING.to_le_bytes());
    }

    #[test]
    fn header_crc_covers_reserved_bytes() {
        let mut img = image(BODY);
        img[0x80] = 0x00;
        assert_eq!(parse(&img), Err(SlotError::HeaderCrc));
    }

    #[test]
    fn rejects_erased_page() {
        assert_eq!(parse(&[0xFF; 256]), Err(SlotError::BadMagic));
    }

    #[test]
    fn rejects_short_page() {
        assert_eq!(parse(&image(BODY)[..0xFF]), Err(SlotError::Truncated));
    }

    #[test]
    fn rejects_header_crc_mismatch() {
        let mut img = image(BODY);
        img[0x08] ^= 1;
        assert_eq!(parse(&img), Err(SlotError::HeaderCrc));
    }

    #[test]
    fn rejects_bad_version() {
        let mut img = image(BODY);
        img[4..6].copy_from_slice(&2u16.to_le_bytes());
        recrc(&mut img);
        assert_eq!(parse(&img), Err(SlotError::BadVersion));
    }

    #[test]
    fn rejects_bad_variant() {
        let mut img = image(BODY);
        img[6..8].copy_from_slice(&9u16.to_le_bytes());
        recrc(&mut img);
        assert_eq!(parse(&img), Err(SlotError::BadVariant));
    }

    #[test]
    fn rejects_body_len_overflow() {
        for len in [0, MAX_BODY_LEN + 1, u32::MAX] {
            let mut img = image(BODY);
            put(&mut img, 0x0C, len);
            recrc(&mut img);
            assert_eq!(parse(&img), Err(SlotError::BodyLen), "body_len {len:#x}");
        }
        assert_eq!(
            encode_header(&[], Variant::Oled, 0, 0x100),
            Err(SlotError::BodyLen)
        );
    }

    #[test]
    fn rejects_entry_past_body() {
        for entry in [0xFC, 0x100 + BODY.len() as u32, u32::MAX - 3] {
            let mut img = image(BODY);
            put(&mut img, 0x14, entry);
            recrc(&mut img);
            assert_eq!(parse(&img), Err(SlotError::Entry), "entry {entry:#x}");
        }
        assert_eq!(
            encode_header(BODY, Variant::Oled, 0, 0x104),
            Err(SlotError::Entry)
        );
    }

    #[test]
    fn rejects_entry_word_past_body() {
        // The whole entry instruction must lie in the CRC-checked body.
        assert_eq!(
            encode_header(&[0; 2], Variant::Oled, 0, 0x100),
            Err(SlotError::Entry)
        );
        assert_eq!(
            encode_header(&[0; 6], Variant::Oled, 0, 0x104),
            Err(SlotError::Entry)
        );
        let mut img = image(&[0; 6]);
        put(&mut img, 0x14, 0x104);
        recrc(&mut img);
        assert_eq!(parse(&img), Err(SlotError::Entry));
    }

    #[test]
    fn rejects_misaligned_entry() {
        let mut img = image(&[0; 8]);
        put(&mut img, 0x14, 0x101);
        recrc(&mut img);
        assert_eq!(parse(&img), Err(SlotError::Entry));
    }

    #[test]
    fn rejects_truncated_slot() {
        let img = image(BODY);
        assert_eq!(validate(&img[..img.len() - 1]), Err(SlotError::Truncated));
    }

    #[test]
    fn rejects_body_crc_mismatch() {
        let mut img = image(BODY);
        img[0x101] ^= 0x80;
        assert_eq!(validate(&img), Err(SlotError::BodyCrc));
    }

    #[test]
    fn confirm_word_is_outside_crcs() {
        let mut img = image(BODY);
        put(&mut img, 0xFC, CONFIRMED);
        assert_eq!(validate(&img).map(|h| h.confirmed), Ok(true));
    }

    #[test]
    fn error_codes_are_frozen() {
        use SlotError::*;
        let all = [
            BadMagic, BadVersion, HeaderCrc, BadVariant, BodyLen, Entry, Truncated, BodyCrc,
        ];
        for (i, e) in all.into_iter().enumerate() {
            assert_eq!(e.code(), i as u32 + 1);
        }
    }

    #[test]
    fn boot_info_packs() {
        assert_eq!(boot_info(1, 2), 0x5330_0102);
    }
}
