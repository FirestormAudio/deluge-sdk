//! A header for a flat image a loader copies into RAM and runs: an app stored in the flash app
//! slot, or the loader itself inside its boot slot.
//!
//! ## Header (all fields little-endian; every other byte is `0xFF`)
//!
//! | Offset | Field          | Value |
//! |--------|----------------|-------|
//! | `0x00` | `magic` u32    | [`APP_MAGIC`] (`b"DAPP"`) |
//! | `0x04` | `version` u16  | [`APP_VERSION`] |
//! | `0x06` | `variant` u16  | the variant code (as [`crate::slot`]'s) |
//! | `0x08` | `load_addr` u32 | where the body is copied |
//! | `0x0C` | `entry` u32    | where to branch: 4-aligned, the whole word inside the copied body |
//! | `0x10` | `body_len` u32 | `1..=max_body`, the caller's bound |
//! | `0x14` | `body_crc` u32 | [`crc32`] of the body |
//! | `0x18` | reserved       | `0xFF`; covered by `header_crc` |
//! | `0x3C` | `header_crc`   | [`crc32`] of bytes `0x00..0x3C` |
//!
//! The body follows the header directly. Images are never executed in place: the header says
//! where the body runs, and the loader copies it there.

use crate::crc::crc32;

/// `b"DAPP"` read as a little-endian `u32`.
pub const APP_MAGIC: u32 = 0x5050_4144;
/// The header version this crate writes and accepts.
pub const APP_VERSION: u16 = 1;
/// The header's length; the body starts here.
pub const APP_HEADER_LEN: usize = 0x40;

const HEADER_CRC_OFFSET: usize = 0x3C;

/// A header that passed every check [`validate_app`] makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppHeader {
    /// The variant code, one the caller accepted.
    pub variant: u16,
    pub load_addr: u32,
    pub entry: u32,
    pub body_len: u32,
    pub body_crc: u32,
}

/// Why an app image was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppError {
    /// The magic is wrong: an erased or foreign image.
    BadMagic,
    /// A header version this crate does not read.
    BadVersion,
    /// The header CRC does not match.
    HeaderCrc,
    /// A variant code the caller doesn't accept.
    BadVariant,
    /// `body_len` is zero or over the caller's bound.
    BodyLen,
    /// The entry is misaligned, or its instruction word is not wholly inside the copied body.
    Entry,
    /// The data given is shorter than the header, or than header + body.
    Truncated,
    /// The body CRC does not match.
    BodyCrc,
}

impl AppError {
    /// A code for a blink or a status word: `1` for [`AppError::BadMagic`] through `8` for
    /// [`AppError::BodyCrc`], in declaration order.
    pub const fn code(self) -> u32 {
        self as u32 + 1
    }
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The header for `body`, to run at `load_addr` from `entry`.
pub fn encode_app_header(
    body: &[u8],
    variant: u16,
    load_addr: u32,
    entry: u32,
) -> [u8; APP_HEADER_LEN] {
    let mut h = [0xFF; APP_HEADER_LEN];
    h[0..4].copy_from_slice(&APP_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&APP_VERSION.to_le_bytes());
    h[6..8].copy_from_slice(&variant.to_le_bytes());
    h[8..12].copy_from_slice(&load_addr.to_le_bytes());
    h[12..16].copy_from_slice(&entry.to_le_bytes());
    h[16..20].copy_from_slice(&(body.len() as u32).to_le_bytes());
    h[20..24].copy_from_slice(&crc32(body).to_le_bytes());
    let crc = crc32(&h[..HEADER_CRC_OFFSET]);
    h[HEADER_CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
    h
}

/// Check an app image's header (the first [`APP_HEADER_LEN`] bytes of `header`): its fields, that
/// its variant is in `accepts`, that the body is `1..=max_body` bytes and the entry inside it.
/// The body isn't read: for a caller whose image is already CRC-checked as a whole (a shim inside
/// a boot slot stage-0 checked). Lengths and addresses are checked in `u64`, so no header word
/// can wrap a sum.
pub fn validate_app_header(
    header: &[u8],
    accepts: &[u16],
    max_body: u32,
) -> Result<AppHeader, AppError> {
    let h = header.get(..APP_HEADER_LEN).ok_or(AppError::Truncated)?;
    if word(h, 0) != APP_MAGIC {
        return Err(AppError::BadMagic);
    }
    if crc32(&h[..HEADER_CRC_OFFSET]) != word(h, HEADER_CRC_OFFSET) {
        return Err(AppError::HeaderCrc);
    }
    if u16::from_le_bytes([h[4], h[5]]) != APP_VERSION {
        return Err(AppError::BadVersion);
    }
    let header = AppHeader {
        variant: u16::from_le_bytes([h[6], h[7]]),
        load_addr: word(h, 8),
        entry: word(h, 12),
        body_len: word(h, 16),
        body_crc: word(h, 20),
    };
    if !accepts.contains(&header.variant) {
        return Err(AppError::BadVariant);
    }
    if header.body_len == 0 || header.body_len > max_body {
        return Err(AppError::BodyLen);
    }
    let (load, entry, len) = (
        u64::from(header.load_addr),
        u64::from(header.entry),
        u64::from(header.body_len),
    );
    if entry % 4 != 0 || entry < load || entry + 4 > load + len {
        return Err(AppError::Entry);
    }
    Ok(header)
}

/// Check `image` (header then body): [`validate_app_header`], then that the body is present and
/// its CRC matches.
pub fn validate_app(image: &[u8], accepts: &[u16], max_body: u32) -> Result<AppHeader, AppError> {
    let header = validate_app_header(image, accepts, max_body)?;
    let body = image
        .get(APP_HEADER_LEN..APP_HEADER_LEN + header.body_len as usize)
        .ok_or(AppError::Truncated)?;
    if crc32(body) != header.body_crc {
        return Err(AppError::BodyCrc);
    }
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOAD: u32 = 0x2002_0000;

    fn image(body: &[u8], variant: u16, entry: u32) -> Vec<u8> {
        let mut v = encode_app_header(body, variant, LOAD, entry).to_vec();
        v.extend_from_slice(body);
        v
    }

    fn body() -> Vec<u8> {
        (0..64u8).collect()
    }

    #[test]
    fn round_trip() {
        let img = image(&body(), 7, LOAD + 8);
        let h = validate_app(&img, &[7], 0x1000).unwrap();
        assert_eq!(
            h,
            AppHeader {
                variant: 7,
                load_addr: LOAD,
                entry: LOAD + 8,
                body_len: 64,
                body_crc: crate::crc32(&body()),
            }
        );
    }

    #[test]
    fn bad_magic() {
        let mut img = image(&body(), 7, LOAD);
        img[0] ^= 1;
        assert_eq!(validate_app(&img, &[7], 0x1000), Err(AppError::BadMagic));
    }

    /// Re-seal the header CRC after editing a field, so the field's own check is what fails.
    fn reseal(img: &mut [u8]) {
        let crc = crate::crc32(&img[..0x3C]);
        img[0x3C..0x40].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn bad_version() {
        let mut img = image(&body(), 7, LOAD);
        img[4] = 9;
        reseal(&mut img);
        assert_eq!(validate_app(&img, &[7], 0x1000), Err(AppError::BadVersion));
    }

    #[test]
    fn header_crc() {
        let mut img = image(&body(), 7, LOAD);
        img[0x20] = 0;
        assert_eq!(validate_app(&img, &[7], 0x1000), Err(AppError::HeaderCrc));
    }

    #[test]
    fn foreign_variant_refused() {
        let img = image(&body(), 7, LOAD);
        assert_eq!(
            validate_app(&img, &[0, 1], 0x1000),
            Err(AppError::BadVariant)
        );
    }

    #[test]
    fn body_len_zero_or_over_max() {
        let img = image(&body(), 7, LOAD);
        assert_eq!(validate_app(&img, &[7], 63), Err(AppError::BodyLen));
        let mut empty = image(&[], 7, LOAD);
        reseal(&mut empty);
        assert_eq!(validate_app(&empty, &[7], 0x1000), Err(AppError::BodyLen));
    }

    #[test]
    fn entry_outside_body_refused() {
        for entry in [LOAD - 4, LOAD + 64, LOAD + 62, LOAD + 2] {
            let img = image(&body(), 7, entry);
            assert_eq!(
                validate_app(&img, &[7], 0x1000),
                Err(AppError::Entry),
                "{entry:#x}"
            );
        }
    }

    #[test]
    fn truncated() {
        let img = image(&body(), 7, LOAD);
        assert_eq!(
            validate_app(&img[..APP_HEADER_LEN - 1], &[7], 0x1000),
            Err(AppError::Truncated)
        );
        assert_eq!(
            validate_app(&img[..img.len() - 1], &[7], 0x1000),
            Err(AppError::Truncated)
        );
    }

    #[test]
    fn body_crc() {
        let mut img = image(&body(), 7, LOAD);
        let last = img.len() - 1;
        img[last] ^= 0xFF;
        assert_eq!(validate_app(&img, &[7], 0x1000), Err(AppError::BodyCrc));
    }

    /// The header alone, for a caller whose body is already CRC-checked (a shim inside a slot
    /// stage-0 checked whole): every header check, no body read.
    #[test]
    fn header_only_skips_the_body() {
        let mut img = image(&body(), 7, LOAD + 8);
        let last = img.len() - 1;
        img[last] ^= 0xFF;
        assert_eq!(validate_app(&img, &[7], 0x1000), Err(AppError::BodyCrc));
        let h = validate_app_header(&img[..APP_HEADER_LEN], &[7], 0x1000).unwrap();
        assert_eq!((h.load_addr, h.entry, h.body_len), (LOAD, LOAD + 8, 64));
        assert_eq!(
            validate_app_header(&img[..APP_HEADER_LEN], &[0], 0x1000),
            Err(AppError::BadVariant)
        );
        assert_eq!(
            validate_app_header(&img[..APP_HEADER_LEN - 1], &[7], 0x1000),
            Err(AppError::Truncated)
        );
    }

    #[test]
    fn reserved_bytes_are_ff() {
        let img = image(&body(), 7, LOAD);
        assert!(img[0x18..0x3C].iter().all(|&b| b == 0xFF));
        assert_eq!(&img[..4], b"DAPP");
    }
}
