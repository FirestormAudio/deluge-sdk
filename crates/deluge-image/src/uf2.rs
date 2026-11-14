//! UF2 blocks: the file format `USB UPDATE` takes over its drag-and-drop drive.
//!
//! A `.uf2` file is a run of 512-byte blocks, each carrying up to 476 bytes of payload and where
//! it goes (all fields little-endian):
//!
//! | Offset | Field |
//! |--------|-------|
//! | `0x000` | [`MAGIC_START0`] |
//! | `0x004` | [`MAGIC_START1`] |
//! | `0x008` | flags: [`FLAG_FAMILY`] required, [`FLAG_NOT_MAIN_FLASH`] refused |
//! | `0x00C` | `targetAddr`: here, the byte offset into the image (base 0) |
//! | `0x010` | `payloadSize` (`≤ 476`) |
//! | `0x014` | `blockNo` |
//! | `0x018` | `numBlocks` |
//! | `0x01C` | the family ID: which board, and app or loader ([`Families`]) |
//! | `0x020` | 476 data bytes |
//! | `0x1FC` | [`MAGIC_END`] |
//!
//! The family ID decides the destination (the app slot, or a boot slot chosen at write time), so
//! `targetAddr` is an offset into the image, not a flash address.

/// A UF2 block's length: one 512-byte sector.
pub const BLOCK_LEN: usize = 512;
/// The payload [`encode`] puts in each block.
pub const PAYLOAD_LEN: usize = 256;
/// The first start magic, `"UF2\n"`.
pub const MAGIC_START0: u32 = 0x0A32_4655;
/// The second start magic.
pub const MAGIC_START1: u32 = 0x9E5D_5157;
/// The end magic.
pub const MAGIC_END: u32 = 0x0AB1_6F30;
/// Flag: the block isn't for the main flash (a comment or a file container); never accepted.
pub const FLAG_NOT_MAIN_FLASH: u32 = 0x0000_0001;
/// Flag: the family ID field is valid; required.
pub const FLAG_FAMILY: u32 = 0x0000_2000;
/// The most blocks one file may have: a 4 MiB body and its 64-byte DAPP header at
/// [`PAYLOAD_LEN`], with room to spare.
pub const MAX_BLOCKS: u32 = 0x4100;

/// The largest payload a block can carry.
const MAX_PAYLOAD: usize = 476;
const DATA_OFF: usize = 0x20;
const END_OFF: usize = 0x1FC;

/// A board's two UF2 family IDs: one for an app (the flash app slot) and one for a loader (a
/// boot slot).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Families {
    pub app: u32,
    pub loader: u32,
}

/// The OLED Deluge's family IDs.
pub const DELUGE_OLED: Families = Families {
    app: 0xA799_ECAC,
    loader: 0x51B0_2BFD,
};

/// The 7-segment Deluge's family IDs.
pub const DELUGE_SEVEN_SEG: Families = Families {
    app: 0xC6E8_31B1,
    loader: 0x9851_DF9D,
};

/// A decoded block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block<'a> {
    /// Where the payload goes: a byte offset into the image.
    pub target: u32,
    pub block_no: u32,
    pub num_blocks: u32,
    pub family: u32,
    pub payload: &'a [u8],
}

fn word(b: &[u8; BLOCK_LEN], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Decode `sector` as a UF2 block; `None` unless every magic is right, the family flag is set,
/// the not-main-flash flag is clear, and the payload fits.
pub fn decode(sector: &[u8; BLOCK_LEN]) -> Option<Block<'_>> {
    let flags = word(sector, 8);
    let size = word(sector, 16) as usize;
    let ok = word(sector, 0) == MAGIC_START0
        && word(sector, 4) == MAGIC_START1
        && word(sector, END_OFF) == MAGIC_END
        && flags & FLAG_FAMILY != 0
        && flags & FLAG_NOT_MAIN_FLASH == 0
        && size <= MAX_PAYLOAD;
    ok.then(|| Block {
        target: word(sector, 12),
        block_no: word(sector, 20),
        num_blocks: word(sector, 24),
        family: word(sector, 28),
        payload: &sector[DATA_OFF..DATA_OFF + size],
    })
}

/// `image` as UF2 blocks of [`PAYLOAD_LEN`] bytes under `family`, `targetAddr` counting from 0;
/// the last payload is short, the rest of each data area zero.
pub fn encode(image: &[u8], family: u32) -> impl Iterator<Item = [u8; BLOCK_LEN]> + '_ {
    let num_blocks = image.len().div_ceil(PAYLOAD_LEN) as u32;
    image
        .chunks(PAYLOAD_LEN)
        .enumerate()
        .map(move |(i, chunk)| {
            let mut b = [0u8; BLOCK_LEN];
            let mut put = |off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
            put(0, MAGIC_START0);
            put(4, MAGIC_START1);
            put(8, FLAG_FAMILY);
            put(12, (i * PAYLOAD_LEN) as u32);
            put(16, chunk.len() as u32);
            put(20, i as u32);
            put(24, num_blocks);
            put(28, family);
            put(END_OFF, MAGIC_END);
            b[DATA_OFF..DATA_OFF + chunk.len()].copy_from_slice(chunk);
            b
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 + 3) as u8).collect()
    }

    fn blocks(image: &[u8], family: u32) -> Vec<[u8; BLOCK_LEN]> {
        encode(image, family).collect()
    }

    fn word(b: &[u8; BLOCK_LEN], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
    }

    fn set_word(b: &mut [u8; BLOCK_LEN], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn round_trip() {
        let image = img(1000);
        let bs = blocks(&image, DELUGE_OLED.app);
        assert_eq!(bs.len(), 4);
        let mut back = Vec::new();
        for (i, b) in bs.iter().enumerate() {
            let d = decode(b).expect("decodes");
            assert_eq!(d.block_no, i as u32);
            assert_eq!(d.num_blocks, 4);
            assert_eq!(d.family, DELUGE_OLED.app);
            assert_eq!(d.target, (i * PAYLOAD_LEN) as u32);
            back.extend_from_slice(d.payload);
        }
        assert_eq!(back, image);
    }

    #[test]
    fn rejects_bad_magic_end() {
        let mut b = blocks(&img(10), DELUGE_OLED.app)[0];
        b[BLOCK_LEN - 1] ^= 0xFF;
        assert!(decode(&b).is_none());
    }

    #[test]
    fn rejects_bad_start_magic() {
        let mut b = blocks(&img(10), DELUGE_OLED.app)[0];
        b[4] ^= 0x01;
        assert!(decode(&b).is_none());
    }

    #[test]
    fn rejects_no_family_flag() {
        let mut b = blocks(&img(10), DELUGE_OLED.app)[0];
        let flags = word(&b, 8);
        set_word(&mut b, 8, flags & !FLAG_FAMILY);
        assert!(decode(&b).is_none());
    }

    #[test]
    fn rejects_not_main_flash() {
        let mut b = blocks(&img(10), DELUGE_OLED.app)[0];
        let flags = word(&b, 8);
        set_word(&mut b, 8, flags | FLAG_NOT_MAIN_FLASH);
        assert!(decode(&b).is_none());
    }

    #[test]
    fn rejects_oversize_payload() {
        let mut b = blocks(&img(10), DELUGE_OLED.app)[0];
        set_word(&mut b, 16, 477);
        assert!(decode(&b).is_none());
    }

    #[test]
    fn families_are_distinct() {
        let ids = [
            DELUGE_OLED.app,
            DELUGE_OLED.loader,
            DELUGE_SEVEN_SEG.app,
            DELUGE_SEVEN_SEG.loader,
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn a_block_is_room_for_a_whole_app() {
        // A 4 MiB body plus its 64-byte DAPP header, in 256-byte payloads.
        assert!(MAX_BLOCKS as usize * PAYLOAD_LEN >= (4 << 20) + 64);
    }
}
