//! Persistent app-loader settings record — the pure encode/decode for the
//! dev-mode flag the SSB stores in the SPI-flash settings sector.
//!
//! The hardware read/write (flash erase/program through the memory-mapped
//! window) lives in the on-device `app-loader::settings` wrapper; the *format* —
//! magic, version, flags, CRC — lives here so it is host-testable and has a
//! single definition, exactly like the ELF/FSB helpers in [`crate::elf`].
//!
//! ## On-flash layout (one 256 B page; the rest of the sector stays erased)
//!
//! | Offset | Field      | Notes                                            |
//! |--------|------------|--------------------------------------------------|
//! | 0..4   | `magic`    | `b"DSET"`                                         |
//! | 4      | `version`  | record version (`1`)                             |
//! | 5      | `flags`    | bit 0 = dev_mode; other bits reserved (0)        |
//! | 6..8   | reserved   | 0                                                 |
//! | 8..12  | `crc32`    | CRC-32 (IEEE) of bytes `0..8`, little-endian      |
//!
//! Erased flash reads `0xFF`, so a blank sector fails the magic check and the
//! device falls back to [`Settings::default`] (dev mode off).

use crate::crc::crc32;

/// Persistent loader settings.  Room to grow: add a field here plus a flag bit
/// (or a new versioned field) in [`encode`]/[`decode`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Settings {
    /// When `true`, the loader listens for USB uploads and disables the
    /// auto-boot countdown.  Default `false` — a stock unit never accepts
    /// firmware over USB.
    pub dev_mode: bool,
}

/// Magic identifying a valid settings record.
pub const MAGIC: [u8; 4] = *b"DSET";
/// Current record version.
pub const VERSION: u8 = 1;
/// `flags` bit: dev mode enabled.
const FLAG_DEV_MODE: u8 = 1 << 0;
/// Bytes covered by the CRC (everything before the CRC word).
const PAYLOAD_LEN: usize = 8;
/// Total encoded record length.
pub const RECORD_LEN: usize = 12;

/// Encode `s` into its fixed-size on-flash record (magic + version + flags +
/// reserved + CRC-32 of the payload).
pub fn encode(s: &Settings) -> [u8; RECORD_LEN] {
    let mut buf = [0u8; RECORD_LEN];
    buf[0..4].copy_from_slice(&MAGIC);
    buf[4] = VERSION;
    buf[5] = if s.dev_mode { FLAG_DEV_MODE } else { 0 };
    // buf[6..8] reserved, already zero.
    let crc = crc32(&buf[..PAYLOAD_LEN]);
    buf[8..12].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// Decode a settings record, or `None` if `buf` is not a valid one (too short,
/// wrong magic/version, or a CRC mismatch — including blank `0xFF` flash).
pub fn decode(buf: &[u8]) -> Option<Settings> {
    if buf.len() < RECORD_LEN {
        return None;
    }
    if buf[0..4] != MAGIC || buf[4] != VERSION {
        return None;
    }
    let stored = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    if crc32(&buf[..PAYLOAD_LEN]) != stored {
        return None;
    }
    Some(Settings {
        dev_mode: buf[5] & FLAG_DEV_MODE != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_both_states() {
        for dev_mode in [false, true] {
            let s = Settings { dev_mode };
            assert_eq!(decode(&encode(&s)), Some(s));
        }
    }

    #[test]
    fn blank_or_zeroed_flash_is_rejected() {
        // Erased flash reads all 0xFF; a never-written sector may read 0x00.
        assert_eq!(decode(&[0xFFu8; RECORD_LEN]), None);
        assert_eq!(decode(&[0x00u8; RECORD_LEN]), None);
    }

    #[test]
    fn rejects_corruption_and_bad_version() {
        // Flip a payload bit without fixing the CRC.
        let mut rec = encode(&Settings { dev_mode: true });
        rec[5] ^= 0x02;
        assert_eq!(decode(&rec), None);

        // Truncated record.
        let rec = encode(&Settings::default());
        assert_eq!(decode(&rec[..RECORD_LEN - 1]), None);

        // Unknown version.
        let mut rec = encode(&Settings::default());
        rec[4] = 0xFF;
        assert_eq!(decode(&rec), None);
    }

    #[test]
    fn default_is_dev_mode_off() {
        assert!(!Settings::default().dev_mode);
    }
}
