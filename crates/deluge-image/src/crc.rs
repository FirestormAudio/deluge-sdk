//! CRC-32 (IEEE 802.3) — the single shared implementation both ends of the
//! dev-upload protocol, the boot-slot and app headers, and the on-flash settings
//! record agree on.
//!
//! Reflected, `0xEDB88320` polynomial, `0xFFFFFFFF` init and final XOR — i.e. the
//! same CRC `gzip`/`zlib`/`PNG` use, so the host tool can compute it with any
//! off-the-shelf routine and get the same value. Byte-at-a-time over a 256-entry
//! table ([`IEEE_TABLE`], built at compile time). [`crc32`] and [`Crc32::update`]
//! read a shared `static` copy; a caller that needs the table somewhere particular
//! (stage-0 keeps one in RAM, since a lookup through the uncached flash window
//! costs a flash read) holds its own copy and uses [`crc32_with`] /
//! [`Crc32::update_with`].

/// A CRC-32 lookup table: entry `i` is the CRC register after shifting byte `i` through.
///
/// Only [`IEEE_TABLE`] exists: a caller copies it (to place it somewhere particular) but can't
/// build another, so [`crc32_with`] always computes the CRC this crate defines.
///
/// ```compile_fail
/// let forged = deluge_image::crc::CrcTable([0; 256]);
/// ```
pub struct CrcTable([u32; 256]);

const fn build_table() -> CrcTable {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    CrcTable(table)
}

/// The IEEE (`0xEDB88320`) table.
pub const IEEE_TABLE: CrcTable = build_table();

/// The copy [`crc32`] and [`Crc32::update`] read.
static TABLE: CrcTable = IEEE_TABLE;

/// Incremental CRC-32 (IEEE) accumulator, so a stream can be checksummed without
/// buffering it: `let mut c = Crc32::new(); c.update(a); c.update(b);` then
/// `c.finalize()` equals `crc32(a followed by b)`.
#[derive(Clone, Copy)]
pub struct Crc32(u32);

impl Crc32 {
    /// Start a fresh CRC accumulation.
    pub fn new() -> Self {
        Crc32(0xFFFF_FFFF)
    }

    /// Fold `data` into the running CRC.
    pub fn update(&mut self, data: &[u8]) {
        self.update_with(&TABLE, data);
    }

    /// [`Crc32::update`] with the caller's copy of [`IEEE_TABLE`].
    pub fn update_with(&mut self, table: &CrcTable, data: &[u8]) {
        let mut crc = self.0;
        for &byte in data {
            crc = table.0[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        self.0 = crc;
    }

    /// Finish and return the CRC-32 value (does not consume the accumulator).
    pub fn finalize(&self) -> u32 {
        !self.0
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_with(&TABLE, data)
}

/// [`crc32`] with the caller's copy of [`IEEE_TABLE`].
pub fn crc32_with(table: &CrcTable, data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update_with(table, data);
    c.finalize()
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn known_vectors() {
        // Standard CRC-32/ISO-HDLC check values (cross-checked against zlib).
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn single_byte_difference_changes_crc() {
        assert_ne!(crc32(b"DLUP\x01"), crc32(b"DLUP\x00"));
    }

    #[test]
    fn streaming_matches_one_shot() {
        use super::Crc32;
        let data = b"The quick brown fox jumps over the lazy dog";
        let mut c = Crc32::new();
        c.update(&data[..10]);
        c.update(&data[10..20]);
        c.update(&data[20..]);
        assert_eq!(c.finalize(), crc32(data));
        assert_eq!(c.finalize(), 0x414F_A339);
        assert_eq!(Crc32::new().finalize(), 0x0000_0000);
    }

    /// The bit-at-a-time CRC the table replaces, kept as the reference.
    fn bitwise(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 + 7) as u8).collect()
    }

    #[test]
    fn table_matches_known_entries() {
        use super::IEEE_TABLE;
        assert_eq!(IEEE_TABLE.0[0], 0);
        assert_eq!(IEEE_TABLE.0[1], 0x7707_3096);
        assert_eq!(IEEE_TABLE.0[255], 0x2D02_EF8D);
    }

    #[test]
    fn table_matches_bitwise_reference() {
        for len in 0..=300 {
            let d = pattern(len);
            assert_eq!(crc32(&d), bitwise(&d), "len {len}");
        }
    }

    #[test]
    fn with_table_matches_default() {
        use super::{IEEE_TABLE, crc32_with};
        for len in [0, 1, 7, 64, 300] {
            let d = pattern(len);
            assert_eq!(crc32_with(&IEEE_TABLE, &d), crc32(&d));
        }
    }
}
