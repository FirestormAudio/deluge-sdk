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
use deluge_image::fat::{self, Sector};

use crate::uf2::{self, Uf2Programmer};

// ── Live UF2 progress (read by the OLED status task) ───────────────────────────

/// Blocks programmed so far in the current session.
pub static UF2_BLOCKS_DONE: AtomicU32 = AtomicU32::new(0);
/// Total blocks in the image being programmed (0 until the first block).
pub static UF2_NUM_BLOCKS: AtomicU32 = AtomicU32::new(0);
/// Set once the final block of an image has been programmed.
pub static UF2_DONE: AtomicBool = AtomicBool::new(false);

/// Bytes per sector.
const SECTOR: usize = fat::SECTOR;

// ── Device ─────────────────────────────────────────────────────────────────

/// Ghost FAT16 block device exposing `CURRENT.UF2`.
///
/// The pure FAT16 layout (geometry, boot sector, FAT, root dir) lives in
/// [`deluge_image::fat`]; this type adds the hardware: data sectors are filled
/// from flash via [`uf2::render_dump_block`] and writes are routed to the
/// [`Uf2Programmer`].
pub struct GhostFat {
    /// Pure FAT16 volume layout for the current firmware image.
    layout: fat::GhostFat,
    prog: Uf2Programmer,
}

impl GhostFat {
    /// Create a ghost volume that dumps `image_len` bytes of the firmware slot
    /// as `CURRENT.UF2`.
    pub fn new(image_len: u32) -> Self {
        UF2_BLOCKS_DONE.store(0, Ordering::Relaxed);
        UF2_NUM_BLOCKS.store(0, Ordering::Relaxed);
        UF2_DONE.store(false, Ordering::Relaxed);
        Self {
            layout: fat::GhostFat::new(image_len),
            prog: Uf2Programmer::new(),
        }
    }

    /// Synthesize one 512-byte sector for `lba` into `out`.
    fn read_sector(&self, lba: u32, out: &mut [u8]) {
        out[..SECTOR].fill(0);
        match self.layout.classify(lba) {
            Sector::Boot => self.layout.write_boot(out),
            Sector::Fat(i) => self.layout.write_fat(i, out),
            Sector::Root => self.layout.write_root(out),
            // Data sectors carry the live flash image (read here, not in the
            // pure layout) rendered as UF2 blocks.
            Sector::Data(file_sector) => {
                uf2::render_dump_block(file_sector, self.layout.image_len(), out)
            }
            Sector::Unused => {}
        }
    }
}

impl BlockDevice for GhostFat {
    fn block_count(&self) -> u32 {
        self.layout.total_sectors()
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
