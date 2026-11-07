//! The boot-slot format: the header page stage-0 checks before it enters an
//! XIP boot slot, and the slot geometry it is frozen with.
//!
//! Stage-0 is never field-updated, so everything here is frozen with it: a
//! change to the layout, a constant or an error code needs a new `VERSION` and
//! a stage-0 that understands it.
//!
//! ## Slot layout
//!
//! On the Deluge, slot A is flash `0x010000`, slot B `0x080000`, each [`SLOT_LEN`]
//! long ([`CLASSIC`]); another board passes its own [`SlotLayout`] to the `_with`
//! functions. The first [`HEADER_LEN`] bytes are the header page; the body (an XIP
//! image linked to run at slot + `0x100`) follows it.
//!
//! ## Header page (all fields little-endian; every other byte is `0xFF`)
//!
//! | Offset | Field          | Value                                                        |
//! |--------|----------------|--------------------------------------------------------------|
//! | `0x00` | `magic` u32    | [`MAGIC`] (`b"DSLT"`)                                        |
//! | `0x04` | `version` u16  | [`VERSION`]                                                  |
//! | `0x06` | `variant` u16  | the variant code: `0` OLED, `1` 7SEG Deluge; see [`VARIANT_OLED`] |
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

/// Where a board's two boot slots lie in its flash. The header format is the same on every
/// board; the geometry is the board's, frozen with its stage-0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotLayout {
    /// Flash offsets of slots A and B.
    pub slot_offsets: [u32; 2],
    /// Length of each slot, header page included.
    pub slot_len: u32,
}

impl SlotLayout {
    /// The longest body that fits in a slot after its header page.
    pub const fn max_body_len(self) -> u32 {
        self.slot_len - HEADER_LEN as u32
    }
}

/// The Deluge's slots: A at [`SLOT_A_OFFSET`], B at [`SLOT_B_OFFSET`], [`SLOT_LEN`] each.
pub const CLASSIC: SlotLayout = SlotLayout {
    slot_offsets: [SLOT_A_OFFSET, SLOT_B_OFFSET],
    slot_len: SLOT_LEN,
};

/// Variant codes: which build a slot carries. `0` and `1` are the Deluge's front panels; another
/// board documents its code where its own stage-0 lives. A stage-0 accepts only its own codes.
pub const VARIANT_OLED: u16 = 0;
/// The seven-segment Deluge.
pub const VARIANT_SEVEN_SEG: u16 = 1;
/// The codes the Deluge's stage-0 accepts.
const CLASSIC_VARIANTS: &[u16] = &[VARIANT_OLED, VARIANT_SEVEN_SEG];
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

/// Which front panel a Deluge slot's image is built for: the Deluge's variant codes by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Variant {
    /// The OLED Deluge.
    Oled = VARIANT_OLED,
    /// The seven-segment Deluge.
    SevenSeg = VARIANT_SEVEN_SEG,
}

impl Variant {
    /// Its variant code.
    pub const fn code(self) -> u16 {
        self as u16
    }
}

/// A parsed header that passed every check [`parse`] makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotHeader {
    /// The variant code, one the caller accepted.
    pub variant: u16,
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
    /// A variant code the caller doesn't accept.
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

/// The boot-info word for `slot` (`0` = A) entered for `reason` (a [`Reason`] as `u8`; `0` = it
/// was the only valid slot): `0x5330_xxyy`.
pub const fn boot_info(slot: u8, reason: u8) -> u32 {
    BOOT_INFO_TAG | (slot as u32) << 8 | reason as u32
}

fn word(page: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([page[at], page[at + 1], page[at + 2], page[at + 3]])
}

fn half(page: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([page[at], page[at + 1]])
}

fn check_body_len(body_len: u32, layout: SlotLayout) -> Result<(), SlotError> {
    if body_len == 0 || body_len > layout.max_body_len() {
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
    parse_with(page, CLASSIC, CLASSIC_VARIANTS)
}

/// [`parse`] for a board with its own slot `layout`, accepting only the variant codes in
/// `accepts`.
pub fn parse_with(
    page: &[u8],
    layout: SlotLayout,
    accepts: &[u16],
) -> Result<SlotHeader, SlotError> {
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
    let variant = half(page, 0x06);
    if !accepts.contains(&variant) {
        return Err(SlotError::BadVariant);
    }
    let body_len = word(page, 0x0C);
    check_body_len(body_len, layout)?;
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
    validate_with(slot, CLASSIC, CLASSIC_VARIANTS)
}

/// [`validate`] for a board with its own slot `layout` and variant codes (see [`parse_with`]).
pub fn validate_with(
    slot: &[u8],
    layout: SlotLayout,
    accepts: &[u16],
) -> Result<SlotHeader, SlotError> {
    let header = parse_with(slot, layout, accepts)?;
    // `body_len <= layout.max_body_len()` (checked by `parse_with`), so this cannot overflow.
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
    encode_header_with(body, variant.code(), sequence, entry_offset, CLASSIC)
}

/// [`encode_header`] for a board with its own slot `layout` and variant code.
pub fn encode_header_with(
    body: &[u8],
    variant: u16,
    sequence: u32,
    entry_offset: u32,
    layout: SlotLayout,
) -> Result<[u8; HEADER_LEN], SlotError> {
    let body_len = u32::try_from(body.len()).map_err(|_| SlotError::BodyLen)?;
    check_body_len(body_len, layout)?;
    check_entry(entry_offset, body_len)?;
    let mut page = [0xFF; HEADER_LEN];
    page[0x00..0x04].copy_from_slice(&MAGIC.to_le_bytes());
    page[0x04..0x06].copy_from_slice(&VERSION.to_le_bytes());
    page[0x06..0x08].copy_from_slice(&variant.to_le_bytes());
    page[0x08..0x0C].copy_from_slice(&sequence.to_le_bytes());
    page[0x0C..0x10].copy_from_slice(&body_len.to_le_bytes());
    page[0x10..0x14].copy_from_slice(&crc32(body).to_le_bytes());
    page[0x14..0x18].copy_from_slice(&entry_offset.to_le_bytes());
    let crc = crc32(&page[..HEADER_CRC_OFFSET]);
    page[HEADER_CRC_OFFSET..HEADER_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    Ok(page)
}

/// Which slot stage-0 enters, and why ([`select`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Enter `slot` (`0` = A) for `reason`.
    Boot { slot: u8, reason: Reason },
    /// Neither slot is valid.
    NoSlot,
}

/// Why [`select`] chose a slot; its value is the boot-info word's low byte. Frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    /// The only valid slot, confirmed.
    OnlyValid = 0,
    /// The newer of two valid slots, confirmed.
    Newest = 1,
    /// The candidate is pending and gets a try (the watchdog didn't fire).
    PendingTry = 2,
    /// The candidate is pending and the watchdog killed its last boot: the other slot.
    RolledBack = 3,
    /// As `RolledBack`, but there is no other valid slot: the pending slot is tried again.
    PendingRetry = 4,
}

/// Pick the slot to enter from both slots' checks (`a`, `b`: [`validate_with`]'s results) and
/// whether the watchdog overflowed on the last boot (`wovf`).
///
/// The candidate is the valid slot with the higher sequence (A on a tie). A confirmed candidate
/// boots (the watchdog flag is ignored: an app may use the watchdog itself). A pending candidate
/// boots unless the watchdog killed its last boot; then the other valid slot boots, or, with none,
/// the pending one again: it is the only image, and a one-off hang must not park the unit.
pub fn select(
    a: Result<SlotHeader, SlotError>,
    b: Result<SlotHeader, SlotError>,
    wovf: bool,
) -> Choice {
    let (cand, other) = match (a, b) {
        (Err(_), Err(_)) => return Choice::NoSlot,
        (Ok(h), Err(_)) => ((0, h), None),
        (Err(_), Ok(h)) => ((1, h), None),
        (Ok(ha), Ok(hb)) if hb.sequence > ha.sequence => ((1, hb), Some(0)),
        (Ok(ha), Ok(_)) => ((0, ha), Some(1)),
    };
    let (slot, header) = cand;
    let boot = |slot, reason| Choice::Boot { slot, reason };
    match (header.confirmed, wovf, other) {
        (true, _, None) => boot(slot, Reason::OnlyValid),
        (true, _, Some(_)) => boot(slot, Reason::Newest),
        (false, false, _) => boot(slot, Reason::PendingTry),
        (false, true, Some(o)) => boot(o, Reason::RolledBack),
        (false, true, None) => boot(slot, Reason::PendingRetry),
    }
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
                variant: VARIANT_OLED,
                sequence: 7,
                body_len: 4,
                body_crc: crc32(BODY),
                entry_offset: 0x100,
                confirmed: false,
            })
        );
    }

    fn hdr(sequence: u32, confirmed: bool) -> Result<SlotHeader, SlotError> {
        Ok(SlotHeader {
            variant: 0,
            sequence,
            body_len: 4,
            body_crc: 0,
            entry_offset: 0x100,
            confirmed,
        })
    }

    const BAD: Result<SlotHeader, SlotError> = Err(SlotError::BodyCrc);

    fn boot(slot: u8, reason: Reason) -> Choice {
        Choice::Boot { slot, reason }
    }

    #[test]
    fn the_newer_confirmed_slot_boots() {
        assert_eq!(
            select(hdr(1, true), hdr(2, true), false),
            boot(1, Reason::Newest)
        );
        assert_eq!(
            select(hdr(3, true), hdr(2, true), true),
            boot(0, Reason::Newest)
        );
        assert_eq!(select(hdr(1, true), BAD, false), boot(0, Reason::OnlyValid));
    }

    #[test]
    fn a_corrupt_newer_slot_falls_back() {
        // B would be newer, but it doesn't validate: A is all there is.
        assert_eq!(select(hdr(1, true), BAD, false), boot(0, Reason::OnlyValid));
        assert_eq!(select(BAD, hdr(4, true), false), boot(1, Reason::OnlyValid));
    }

    #[test]
    fn a_pending_slot_is_tried() {
        assert_eq!(
            select(hdr(1, true), hdr(2, false), false),
            boot(1, Reason::PendingTry)
        );
    }

    #[test]
    fn a_pending_slot_the_watchdog_killed_rolls_back() {
        assert_eq!(
            select(hdr(1, true), hdr(2, false), true),
            boot(0, Reason::RolledBack)
        );
    }

    #[test]
    fn a_lone_pending_slot_is_retried() {
        assert_eq!(
            select(hdr(2, false), BAD, true),
            boot(0, Reason::PendingRetry)
        );
        assert_eq!(
            select(BAD, hdr(2, false), false),
            boot(1, Reason::PendingTry)
        );
    }

    #[test]
    fn equal_sequences_prefer_a() {
        assert_eq!(
            select(hdr(5, true), hdr(5, true), false),
            boot(0, Reason::Newest)
        );
    }

    #[test]
    fn both_pending_picks_the_newer() {
        assert_eq!(
            select(hdr(5, false), hdr(6, false), false),
            boot(1, Reason::PendingTry)
        );
        // The newer one hung: the older, pending too, gets its try.
        assert_eq!(
            select(hdr(5, false), hdr(6, false), true),
            boot(0, Reason::RolledBack)
        );
    }

    #[test]
    fn wovf_ignored_for_a_confirmed_candidate() {
        assert_eq!(
            select(hdr(1, false), hdr(2, true), true),
            boot(1, Reason::Newest)
        );
    }

    #[test]
    fn no_valid_slot_is_no_slot() {
        assert_eq!(select(BAD, Err(SlotError::BadMagic), true), Choice::NoSlot);
    }

    #[test]
    fn classic_images_parse_as_before() {
        let img = image(BODY);
        let classic = parse_with(&img, CLASSIC, &[VARIANT_OLED, VARIANT_SEVEN_SEG]);
        assert_eq!(parse(&img), classic);
        assert_eq!(validate(&img), validate_with(&img, CLASSIC, &[0, 1]));
        assert_eq!(CLASSIC.slot_offsets, [SLOT_A_OFFSET, SLOT_B_OFFSET]);
        assert_eq!(CLASSIC.slot_len, SLOT_LEN);
        assert_eq!(CLASSIC.max_body_len(), MAX_BODY_LEN);
    }

    #[test]
    fn another_layouts_bound_applies() {
        let small = SlotLayout {
            slot_offsets: [0x1000, 0x2000],
            slot_len: 0x1000,
        };
        let body = [0u8; 0x1000 - HEADER_LEN + 4];
        assert_eq!(
            encode_header_with(&body, 0x100, 1, 0x100, small),
            Err(SlotError::BodyLen)
        );
        let mut img = image(BODY);
        put(&mut img, 0x0C, 0x1000 - HEADER_LEN as u32 + 1);
        recrc(&mut img);
        assert_eq!(parse_with(&img, small, &[0]), Err(SlotError::BodyLen));
    }

    #[test]
    fn an_unaccepted_variant_is_bad_variant() {
        let mut img = image(BODY);
        img[6..8].copy_from_slice(&7u16.to_le_bytes());
        recrc(&mut img);
        assert_eq!(
            parse_with(&img, CLASSIC, &[0, 1]),
            Err(SlotError::BadVariant)
        );
        let mut img = image(BODY);
        img[6..8].copy_from_slice(&1u16.to_le_bytes());
        recrc(&mut img);
        assert_eq!(
            parse_with(&img, CLASSIC, &[0x100]),
            Err(SlotError::BadVariant)
        );
    }

    #[test]
    fn encode_with_round_trips() {
        let layout = SlotLayout {
            slot_offsets: [0x2_0000, 0x4_0000],
            slot_len: 0x2_0000,
        };
        let page = encode_header_with(BODY, 0x100, 9, 0x100, layout).unwrap();
        let mut img = page.to_vec();
        img.extend_from_slice(BODY);
        let h = validate_with(&img, layout, &[0x100]).unwrap();
        assert_eq!((h.variant, h.sequence, h.body_len), (0x100, 9, 4));
        assert!(!h.confirmed);
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
