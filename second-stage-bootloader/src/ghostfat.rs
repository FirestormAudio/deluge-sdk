//! Synthesized ("ghost") FAT16 volume backing the UF2 update drive.
//!
//! When the SSB is in UF2 mode it presents this volume over USB Mass Storage.
//! No real filesystem is stored: reads are synthesized on the fly and writes are
//! routed to the [`crate::uf2`] flash programmer.
//!
//! * Reading lists a single file `CURRENT.UF2` whose contents are the current
//!   firmware-slot image rendered as UF2 blocks (drag it off to back up).
//! * Writing a `.uf2` file streams its blocks straight into the programmer,
//!   which erases-on-first-touch and programs the firmware slot.  FAT/dir
//!   metadata writes carry no UF2 magic and are harmlessly ignored.
//!
//! The geometry is fixed and sized so the cluster count is unambiguously FAT16
//! and there is room for both `CURRENT.UF2` and an incoming image.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use deluge_bsp::usb::bot::{BlockDevice, Inquiry};

use crate::uf2::{self, Uf2Programmer};

// ── Live UF2 progress (read by the OLED status task) ───────────────────────────

/// Blocks programmed so far in the current session.
pub static UF2_BLOCKS_DONE: AtomicU32 = AtomicU32::new(0);
/// Total blocks in the image being programmed (0 until the first block).
pub static UF2_NUM_BLOCKS: AtomicU32 = AtomicU32::new(0);
/// Set once the final block of an image has been programmed.
pub static UF2_DONE: AtomicBool = AtomicBool::new(false);

// ── FAT16 geometry ─────────────────────────────────────────────────────────

const SECTOR: usize = 512;
const SECTORS_PER_CLUSTER: u32 = 8; // 4 KB clusters
const RESERVED_SECTORS: u32 = 1; // boot sector only
const NUM_FATS: u32 = 2;
const ROOT_ENTRIES: u32 = 16; // 1 root-dir sector
const ROOT_DIR_SECTORS: u32 = (ROOT_ENTRIES * 32) / SECTOR as u32;
/// Total data clusters — ≥ 4085 keeps the volume unambiguously FAT16, and the
/// resulting total stays under 65536 sectors so a 16-bit sector count suffices.
const TOTAL_CLUSTERS: u32 = 8000;
const SECTORS_PER_FAT: u32 = ((TOTAL_CLUSTERS + 2) * 2).div_ceil(SECTOR as u32);

const FAT_START: u32 = RESERVED_SECTORS;
const ROOT_START: u32 = FAT_START + NUM_FATS * SECTORS_PER_FAT;
const DATA_START: u32 = ROOT_START + ROOT_DIR_SECTORS;
const TOTAL_SECTORS: u32 = DATA_START + TOTAL_CLUSTERS * SECTORS_PER_CLUSTER;

/// FAT16 entries per 512-byte FAT sector.
const FAT_ENTRIES_PER_SECTOR: u32 = (SECTOR / 2) as u32;
/// First data cluster number in FAT.
const FIRST_CLUSTER: u32 = 2;

// ── Device ─────────────────────────────────────────────────────────────────

/// Ghost FAT16 block device exposing `CURRENT.UF2`.
pub struct GhostFat {
    /// Byte length of the current firmware image (`code_end - code_start`); 0 if
    /// the slot is empty.
    image_len: u32,
    /// `CURRENT.UF2` length in 512-byte blocks (one UF2 block per page).
    dump_blocks: u32,
    /// Clusters occupied by `CURRENT.UF2`.
    dump_clusters: u32,
    prog: Uf2Programmer,
}

impl GhostFat {
    /// Create a ghost volume that dumps `image_len` bytes of the firmware slot
    /// as `CURRENT.UF2`.
    pub fn new(image_len: u32) -> Self {
        let dump_blocks = uf2::dump_block_count(image_len);
        let dump_clusters = dump_blocks.div_ceil(SECTORS_PER_CLUSTER);
        UF2_BLOCKS_DONE.store(0, Ordering::Relaxed);
        UF2_NUM_BLOCKS.store(0, Ordering::Relaxed);
        UF2_DONE.store(false, Ordering::Relaxed);
        Self {
            image_len,
            dump_blocks,
            dump_clusters,
            prog: Uf2Programmer::new(),
        }
    }

    /// `CURRENT.UF2` size in bytes.
    fn dump_size(&self) -> u32 {
        self.dump_blocks * SECTOR as u32
    }

    /// Synthesize one 512-byte sector for `lba` into `out`.
    fn read_sector(&self, lba: u32, out: &mut [u8]) {
        out[..SECTOR].fill(0);
        if lba == 0 {
            self.boot_sector(out);
        } else if (FAT_START..ROOT_START).contains(&lba) {
            // Two identical FAT copies follow the reserved region.
            let fat_sector = (lba - FAT_START) % SECTORS_PER_FAT;
            self.fat_sector(fat_sector, out);
        } else if (ROOT_START..DATA_START).contains(&lba) {
            if lba - ROOT_START == 0 {
                self.root_dir(out);
            }
        } else if lba >= DATA_START {
            let file_sector = lba - DATA_START;
            if file_sector < self.dump_blocks {
                uf2::render_dump_block(file_sector, self.image_len, out);
            }
        }
    }

    fn boot_sector(&self, out: &mut [u8]) {
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

    fn fat_sector(&self, fat_sector: u32, out: &mut [u8]) {
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

    fn root_dir(&self, out: &mut [u8]) {
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

impl BlockDevice for GhostFat {
    fn block_count(&self) -> u32 {
        TOTAL_SECTORS
    }

    fn is_ready(&self) -> bool {
        true
    }

    async fn ensure_ready(&mut self) -> bool {
        true
    }

    async fn read(&mut self, lba: u32, count: u32, buf: &mut [u8]) -> Result<(), ()> {
        for i in 0..count as usize {
            let off = i * SECTOR;
            self.read_sector(lba + i as u32, &mut buf[off..off + SECTOR]);
        }
        Ok(())
    }

    async fn write(&mut self, _lba: u32, count: u32, buf: &[u8]) -> Result<(), ()> {
        // Feed every sector to the programmer; non-UF2 sectors are ignored.
        for i in 0..count as usize {
            let off = i * SECTOR;
            let outcome = self.prog.write_block(&buf[off..off + SECTOR]);
            match outcome {
                uf2::Outcome::Programmed | uf2::Outcome::Done => {
                    let (done, total) = self.prog.progress();
                    UF2_BLOCKS_DONE.store(done, Ordering::Relaxed);
                    UF2_NUM_BLOCKS.store(total, Ordering::Relaxed);
                    if outcome == uf2::Outcome::Done {
                        UF2_DONE.store(true, Ordering::Relaxed);
                    }
                }
                uf2::Outcome::Ignored | uf2::Outcome::Error => {}
            }
        }
        Ok(())
    }

    fn inquiry(&self) -> Inquiry {
        Inquiry {
            vendor: *b"Synthstm",
            product: *b"Deluge UF2 Boot ",
            revision: *b"1.00",
        }
    }
}
