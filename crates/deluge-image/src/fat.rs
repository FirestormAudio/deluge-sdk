//! Synthesized ("ghost") FAT16 volume layout backing the UF2 update drive.
//!
//! No real filesystem is stored: every sector is computed on demand. This
//! module owns the *pure* layout — geometry, boot sector (BPB), FAT, and root
//! directory synthesis, plus the LBA → sector-kind dispatch — so a host test
//! can build the whole volume in memory and even mount it. The on-device
//! adapter ([`app-loader`'s `ghostfat`]) fills data sectors from flash and wires
//! this into USB Mass Storage.
//!
//! The volume exposes a single file, `CURRENT.UF2`, whose contents are the
//! firmware-slot image rendered as UF2 blocks (see [`crate::uf2`]).

use crate::uf2;

/// Bytes per sector.
pub const SECTOR: usize = 512;
/// Sectors per cluster (4 KB clusters).
pub const SECTORS_PER_CLUSTER: u32 = 8;
/// Reserved sectors before the first FAT (boot sector only).
pub const RESERVED_SECTORS: u32 = 1;
/// Number of FAT copies.
pub const NUM_FATS: u32 = 2;
/// Root-directory entries (one 512-byte root-dir sector).
pub const ROOT_ENTRIES: u32 = 16;
/// Root-directory sectors.
pub const ROOT_DIR_SECTORS: u32 = (ROOT_ENTRIES * 32) / SECTOR as u32;
/// Total data clusters — ≥ 4085 keeps the volume unambiguously FAT16, and the
/// resulting total stays under 65536 sectors so a 16-bit sector count suffices.
pub const TOTAL_CLUSTERS: u32 = 8000;
/// Sectors per FAT, sized to hold `TOTAL_CLUSTERS + 2` 16-bit entries.
pub const SECTORS_PER_FAT: u32 = ((TOTAL_CLUSTERS + 2) * 2).div_ceil(SECTOR as u32);

/// First FAT sector (LBA).
pub const FAT_START: u32 = RESERVED_SECTORS;
/// First root-directory sector (LBA).
pub const ROOT_START: u32 = FAT_START + NUM_FATS * SECTORS_PER_FAT;
/// First data sector (LBA).
pub const DATA_START: u32 = ROOT_START + ROOT_DIR_SECTORS;
/// Total sectors in the volume.
pub const TOTAL_SECTORS: u32 = DATA_START + TOTAL_CLUSTERS * SECTORS_PER_CLUSTER;

/// FAT16 entries per 512-byte FAT sector.
pub const FAT_ENTRIES_PER_SECTOR: u32 = (SECTOR / 2) as u32;
/// First data cluster number in the FAT.
pub const FIRST_CLUSTER: u32 = 2;

/// What a logical block address maps to in the synthesized volume.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sector {
    /// The boot sector / BIOS parameter block (LBA 0).
    Boot,
    /// FAT sector, given as a 0-based index *within one FAT copy*.
    Fat(u32),
    /// The (single) root-directory sector.
    Root,
    /// A data-region sector, given as a 0-based file sector of `CURRENT.UF2`.
    Data(u32),
    /// A sector with no synthesized content (reads back as zeros).
    Unused,
}

/// Pure FAT16 ghost-volume layout for a firmware image of `image_len` bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GhostFat {
    image_len: u32,
    dump_blocks: u32,
    dump_clusters: u32,
}

impl GhostFat {
    /// Layout for dumping `image_len` bytes of the firmware slot as
    /// `CURRENT.UF2` (one UF2 block per flash page).
    pub fn new(image_len: u32) -> Self {
        let dump_blocks = uf2::block_count(image_len);
        let dump_clusters = dump_blocks.div_ceil(SECTORS_PER_CLUSTER);
        Self {
            image_len,
            dump_blocks,
            dump_clusters,
        }
    }

    /// Firmware image length in bytes.
    #[inline]
    pub fn image_len(&self) -> u32 {
        self.image_len
    }

    /// `CURRENT.UF2` size in 512-byte blocks (0 if the slot is empty).
    #[inline]
    pub fn dump_blocks(&self) -> u32 {
        self.dump_blocks
    }

    /// Total sectors in the volume (the USB MSC `block_count`).
    #[inline]
    pub fn total_sectors(&self) -> u32 {
        TOTAL_SECTORS
    }

    /// `CURRENT.UF2` size in bytes.
    #[inline]
    pub fn dump_size(&self) -> u32 {
        self.dump_blocks * SECTOR as u32
    }

    /// Map a logical block address to the kind of sector to synthesize.
    pub fn classify(&self, lba: u32) -> Sector {
        if lba == 0 {
            Sector::Boot
        } else if (FAT_START..ROOT_START).contains(&lba) {
            // Two identical FAT copies follow the reserved region.
            Sector::Fat((lba - FAT_START) % SECTORS_PER_FAT)
        } else if (ROOT_START..DATA_START).contains(&lba) {
            if lba - ROOT_START == 0 {
                Sector::Root
            } else {
                Sector::Unused
            }
        } else if lba >= DATA_START {
            let file_sector = lba - DATA_START;
            if file_sector < self.dump_blocks {
                Sector::Data(file_sector)
            } else {
                Sector::Unused
            }
        } else {
            Sector::Unused
        }
    }

    /// Write the boot sector (BIOS parameter block) into `out` (≥ 512 bytes,
    /// pre-zeroed by the caller).
    pub fn write_boot(&self, out: &mut [u8]) {
        out[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]); // jump
        out[3..11].copy_from_slice(b"MSDOS5.0");
        out[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
        out[13] = SECTORS_PER_CLUSTER as u8;
        out[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
        out[16] = NUM_FATS as u8;
        out[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
        out[19..21].copy_from_slice(&(TOTAL_SECTORS as u16).to_le_bytes()); // < 65536
        out[21] = 0xF8; // fixed-disk media descriptor
        out[22..24].copy_from_slice(&(SECTORS_PER_FAT as u16).to_le_bytes());
        out[24..26].copy_from_slice(&1u16.to_le_bytes()); // sectors per track
        out[26..28].copy_from_slice(&1u16.to_le_bytes()); // heads
        out[28..32].copy_from_slice(&0u32.to_le_bytes()); // hidden sectors
        out[32..36].copy_from_slice(&0u32.to_le_bytes()); // total sectors (32-bit unused)
        out[36] = 0x80; // drive number
        out[38] = 0x29; // extended boot signature
        out[39..43].copy_from_slice(&0xDE10_0B07u32.to_le_bytes()); // volume id
        out[43..54].copy_from_slice(b"DELUGE SSB ");
        out[54..62].copy_from_slice(b"FAT16   ");
        out[510] = 0x55;
        out[511] = 0xAA;
    }

    /// Write FAT sector `fat_sector` (0-based within a FAT copy) into `out`
    /// (≥ 512 bytes, pre-zeroed by the caller).
    pub fn write_fat(&self, fat_sector: u32, out: &mut [u8]) {
        let base = fat_sector * FAT_ENTRIES_PER_SECTOR;
        let last_dump_cluster = FIRST_CLUSTER + self.dump_clusters; // exclusive
        for i in 0..FAT_ENTRIES_PER_SECTOR {
            let cluster = base + i;
            let entry: u16 = if cluster == 0 {
                0xFFF8 // media descriptor in entry 0
            } else if cluster == 1 {
                0xFFFF // reserved / end marker
            } else if self.dump_clusters > 0
                && cluster >= FIRST_CLUSTER
                && cluster < last_dump_cluster
            {
                if cluster + 1 == last_dump_cluster {
                    0xFFFF // end of CURRENT.UF2 chain
                } else {
                    (cluster + 1) as u16
                }
            } else {
                0x0000 // free
            };
            let o = (i * 2) as usize;
            out[o..o + 2].copy_from_slice(&entry.to_le_bytes());
        }
    }

    /// Write the root-directory sector into `out` (≥ 512 bytes, pre-zeroed).
    pub fn write_root(&self, out: &mut [u8]) {
        // Entry 0: volume label.
        out[0..11].copy_from_slice(b"DELUGE SSB ");
        out[11] = 0x08; // volume-label attribute

        // Entry 1: CURRENT.UF2 (only if there is an image to dump).
        if self.dump_blocks > 0 {
            let e = &mut out[32..64];
            e[0..11].copy_from_slice(b"CURRENT UF2");
            e[11] = 0x20; // archive
            let first = FIRST_CLUSTER as u16;
            e[26..28].copy_from_slice(&first.to_le_bytes()); // first cluster low
            e[28..32].copy_from_slice(&self.dump_size().to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    fn le16(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    fn le32(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }

    // ---- geometry invariants ----------------------------------------------

    #[test]
    fn geometry_is_unambiguously_fat16() {
        // ≥ 4085 data clusters → FAT16 (below is FAT12, above 65524 is FAT32).
        assert!((4085..65525).contains(&TOTAL_CLUSTERS));
        // 16-bit total-sector field in the BPB must suffice.
        assert!(TOTAL_SECTORS < 65536, "total sectors must fit a u16");
        // Layout is contiguous: reserved | FATs | root | data.
        assert_eq!(FAT_START, RESERVED_SECTORS);
        assert_eq!(ROOT_START, RESERVED_SECTORS + NUM_FATS * SECTORS_PER_FAT);
        assert_eq!(DATA_START, ROOT_START + ROOT_DIR_SECTORS);
        // One FAT must address every cluster (+2 reserved entries).
        assert!(SECTORS_PER_FAT * FAT_ENTRIES_PER_SECTOR >= TOTAL_CLUSTERS + 2);
    }

    // ---- classify ----------------------------------------------------------

    #[test]
    fn classify_covers_each_region() {
        let g = GhostFat::new(1024); // a few data sectors
        assert_eq!(g.classify(0), Sector::Boot);
        assert_eq!(g.classify(FAT_START), Sector::Fat(0));
        // Second FAT copy maps onto the same 0-based index.
        assert_eq!(g.classify(FAT_START + SECTORS_PER_FAT), Sector::Fat(0));
        assert_eq!(g.classify(ROOT_START), Sector::Root);
        assert_eq!(g.classify(DATA_START), Sector::Data(0));
        assert_eq!(g.classify(DATA_START + g.dump_blocks() - 1), Sector::Data(g.dump_blocks() - 1));
        assert_eq!(g.classify(DATA_START + g.dump_blocks()), Sector::Unused); // past the file
    }

    #[test]
    fn empty_slot_has_no_file() {
        let g = GhostFat::new(0);
        assert_eq!(g.dump_blocks(), 0);
        assert_eq!(g.classify(DATA_START), Sector::Unused);
        // Root dir then carries only the volume label, no dir entry.
        let mut root = vec![0u8; SECTOR];
        g.write_root(&mut root);
        assert_eq!(&root[0..11], b"DELUGE SSB ");
        assert!(root[32..64].iter().all(|&b| b == 0), "no CURRENT.UF2 entry");
    }

    // ---- boot sector -------------------------------------------------------

    #[test]
    fn boot_sector_bpb_fields() {
        let g = GhostFat::new(4096);
        let mut s = vec![0u8; SECTOR];
        g.write_boot(&mut s);
        assert_eq!(le16(&s, 11), SECTOR as u16); // bytes/sector
        assert_eq!(s[13], SECTORS_PER_CLUSTER as u8);
        assert_eq!(le16(&s, 14), RESERVED_SECTORS as u16);
        assert_eq!(s[16], NUM_FATS as u8);
        assert_eq!(le16(&s, 17), ROOT_ENTRIES as u16);
        assert_eq!(le16(&s, 19) as u32, TOTAL_SECTORS);
        assert_eq!(s[21], 0xF8);
        assert_eq!(le16(&s, 22), SECTORS_PER_FAT as u16);
        assert_eq!(&s[54..62], b"FAT16   ");
        assert_eq!((s[510], s[511]), (0x55, 0xAA)); // boot signature
    }

    // ---- FAT ---------------------------------------------------------------

    #[test]
    fn fat_reserved_and_chain_entries() {
        // 18 UF2 blocks (18 × 256 B) → 3 clusters, i.e. a real multi-link chain.
        let g = GhostFat::new(18 * uf2::DUMP_PAYLOAD);
        assert!(g.dump_clusters >= 2, "test needs a multi-cluster chain");
        let mut fat = vec![0u8; SECTOR];
        g.write_fat(0, &mut fat);

        assert_eq!(le16(&fat, 0), 0xFFF8, "entry 0 = media descriptor");
        assert_eq!(le16(&fat, 2), 0xFFFF, "entry 1 = reserved");

        let last = FIRST_CLUSTER + g.dump_clusters; // exclusive
        for c in FIRST_CLUSTER..last {
            let want = if c + 1 == last { 0xFFFF } else { (c + 1) as u16 };
            assert_eq!(le16(&fat, c as usize * 2), want, "cluster {c} link");
        }
        // The cluster just past the chain is free.
        assert_eq!(le16(&fat, last as usize * 2), 0x0000);
    }

    // ---- root dir ----------------------------------------------------------

    #[test]
    fn root_dir_lists_current_uf2() {
        let g = GhostFat::new(1000);
        let mut root = vec![0u8; SECTOR];
        g.write_root(&mut root);
        assert_eq!(&root[0..11], b"DELUGE SSB ");
        assert_eq!(root[11], 0x08); // volume label

        let e = &root[32..64];
        assert_eq!(&e[0..11], b"CURRENT UF2");
        assert_eq!(e[11], 0x20); // archive
        assert_eq!(le16(e, 26) as u32, FIRST_CLUSTER); // first cluster
        assert_eq!(le32(e, 28), g.dump_size()); // file size in bytes
    }

    // ---- full-volume assembly ---------------------------------------------

    /// Build the whole synthesized volume into a byte image, filling data
    /// sectors with UF2 blocks of a fake firmware image.
    fn build_volume(g: &GhostFat, firmware: &[u8]) -> Vec<u8> {
        let mut vol = vec![0u8; g.total_sectors() as usize * SECTOR];
        for lba in 0..g.total_sectors() {
            let out = &mut vol[lba as usize * SECTOR..(lba as usize + 1) * SECTOR];
            match g.classify(lba) {
                Sector::Boot => g.write_boot(out),
                Sector::Fat(i) => g.write_fat(i, out),
                Sector::Root => g.write_root(out),
                Sector::Data(fs) => {
                    // Mirror render_dump_block, but read the fake image, not flash.
                    let off = fs * uf2::DUMP_PAYLOAD;
                    let total = uf2::block_count(g.image_len());
                    let n = uf2::DUMP_PAYLOAD.min(g.image_len().saturating_sub(off)) as usize;
                    let mut page = [0u8; uf2::DUMP_PAYLOAD as usize];
                    let src = off as usize;
                    page[..n].copy_from_slice(&firmware[src..src + n]);
                    uf2::build_block(out, fs, total, 0x1810_0000 + off, &page[..n]);
                }
                Sector::Unused => {}
            }
        }
        vol
    }

    #[test]
    fn assembled_volume_is_self_consistent() {
        let firmware: Vec<u8> = (0..1000u32).map(|i| (i * 13) as u8).collect();
        let g = GhostFat::new(firmware.len() as u32);
        let vol = build_volume(&g, &firmware);

        // The two FAT copies are byte-identical.
        let fat0 = &vol[FAT_START as usize * SECTOR..(FAT_START + SECTORS_PER_FAT) as usize * SECTOR];
        let fat1 = &vol[(FAT_START + SECTORS_PER_FAT) as usize * SECTOR
            ..(FAT_START + 2 * SECTORS_PER_FAT) as usize * SECTOR];
        assert_eq!(fat0, fat1, "FAT copies must match");

        // Reassemble CURRENT.UF2 from the data region and decode it back to the
        // firmware image via the UF2 reader — proves the dir size, cluster
        // placement and block payloads all line up.
        let dump_bytes = g.dump_size() as usize;
        let data = &vol[DATA_START as usize * SECTOR..DATA_START as usize * SECTOR + dump_bytes];
        let slot = uf2::Slot {
            flash_base: 0x1800_0000,
            addr: 0x1810_0000,
            len: 0x0030_0000,
            sector_size: 0x0004_0000,
        };
        let mut rebuilt = vec![0u8; firmware.len()];
        for blk in data.chunks(SECTOR) {
            if let uf2::Block::Valid(v) = uf2::classify_block(blk, &slot) {
                let dst = (v.target - slot.addr) as usize;
                let bytes = &blk[v.payload.clone()];
                rebuilt[dst..dst + bytes.len()].copy_from_slice(bytes);
            }
        }
        assert_eq!(rebuilt, firmware);
    }

    /// Mount the synthesized volume with an independent FAT implementation and
    /// read `CURRENT.UF2` back — proves a real host OS would accept the BPB,
    /// follow the FAT cluster chain, and see the right file (§5.2 of the plan).
    #[test]
    fn real_fat_driver_mounts_and_reads_current_uf2() {
        use std::io::{Cursor, Read};

        let firmware: Vec<u8> = (0..3000u32).map(|i| (i * 31 + 7) as u8).collect();
        let g = GhostFat::new(firmware.len() as u32);
        let vol = build_volume(&g, &firmware);

        let fs = fatfs::FileSystem::new(Cursor::new(vol), fatfs::FsOptions::new())
            .expect("volume must mount as FAT");
        let root = fs.root_dir();

        // The root directory lists exactly one regular file, CURRENT.UF2.
        let names: Vec<String> = root
            .iter()
            .map(|e| e.unwrap())
            .filter(|e| !e.is_dir())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, ["CURRENT.UF2"], "root must list CURRENT.UF2");

        // Its bytes are the UF2 dump; decode them back to the firmware image.
        let mut uf2_bytes = Vec::new();
        root.open_file("CURRENT.UF2")
            .unwrap()
            .read_to_end(&mut uf2_bytes)
            .unwrap();
        assert_eq!(uf2_bytes.len() as u32, g.dump_size());

        let slot = uf2::Slot {
            flash_base: 0x1800_0000,
            addr: 0x1810_0000,
            len: 0x0030_0000,
            sector_size: 0x0004_0000,
        };
        let mut rebuilt = vec![0u8; firmware.len()];
        for blk in uf2_bytes.chunks(SECTOR) {
            if let uf2::Block::Valid(v) = uf2::classify_block(blk, &slot) {
                let dst = (v.target - slot.addr) as usize;
                let bytes = &blk[v.payload.clone()];
                rebuilt[dst..dst + bytes.len()].copy_from_slice(bytes);
            }
        }
        assert_eq!(rebuilt, firmware, "round-trip through a real FAT mount");
    }
}
