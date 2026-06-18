//! Upload wire-frame construction and the CRC-32 it carries.

/// Upload wire-frame: `magic | version | flags | len u32 | crc32 u32 | ELF`.
const FRAME_MAGIC: &[u8; 4] = b"DLUP";
const FRAME_VERSION: u8 = 1;

/// Build the upload wire-frame for `elf`:
/// `magic | version | flags | len u32 LE | crc32 u32 LE | <elf>`.
pub(crate) fn build_frame(elf: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + elf.len());
    frame.extend_from_slice(FRAME_MAGIC);
    frame.push(FRAME_VERSION);
    frame.push(0); // flags (reserved)
    frame.extend_from_slice(&(elf.len() as u32).to_le_bytes());
    frame.extend_from_slice(&crc32(elf).to_le_bytes());
    frame.extend_from_slice(elf);
    frame
}

/// CRC-32 (IEEE) — the same checksum the device computes
/// (`deluge_image::crc32`); a test pins the two implementations together.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vectors() {
        // Standard CRC-32/ISO-HDLC check values.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    /// The host framing CRC must agree byte-for-byte with the device's
    /// `deluge_image::crc32`, or an upload would always be rejected.
    #[test]
    fn crc32_agrees_with_device() {
        for sample in [
            b"".as_slice(),
            b"123456789",
            b"\x7FELF\x01\x01\x01",
            &[0u8, 1, 2, 3, 255, 254, 7, 42, 99],
        ] {
            assert_eq!(crc32(sample), deluge_image::crc32(sample));
        }
    }

    #[test]
    fn frame_round_trips() {
        let elf = b"\x7FELF fake image bytes";
        let frame = build_frame(elf);

        assert_eq!(&frame[0..4], FRAME_MAGIC);
        assert_eq!(frame[4], FRAME_VERSION);
        assert_eq!(frame[5], 0, "flags reserved");
        let len = u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]);
        assert_eq!(len as usize, elf.len());
        let crc = u32::from_le_bytes([frame[10], frame[11], frame[12], frame[13]]);
        assert_eq!(crc, crc32(elf));
        assert_eq!(&frame[14..], elf, "payload follows the 14-byte header");
        assert_eq!(frame.len(), 14 + elf.len());
    }
}
