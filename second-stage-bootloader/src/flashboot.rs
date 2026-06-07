//! Boot-from-flash support.
//!
//! In addition to the SD-card `/APPS/` images, the SSB can launch a firmware
//! image stored directly in the SPI flash chip — the same chip that holds the
//! first-stage bootloader and the SSB itself.  This lets a unit boot with no SD
//! card present, like the original Deluge bootloader.
//!
//! ## Image format
//! The on-flash image is a **raw `.bin`** (e.g. produced by `cargo build-fw-bin`)
//! linked to load and run from SRAM at `0x2002_0000`.  Byte 0 of the image is the
//! vector table / `_start`, and the standard FSB metadata words sit just past the
//! eight vector entries (see `rza1l-hal/src/startup.rs`):
//!
//! | Image offset | Word |
//! |--------------|------|
//! | `+0x20` | `code_start`   — load address of byte 0 |
//! | `+0x24` | `code_end`     — one past the last image byte |
//! | `+0x28` | `code_execute` — entry point |
//! | `+0x2C` | `".BootLoad_ValidProgramTest."` signature |
//!
//! The image lives at the flash slot ([`spibsc::FLASH_SLOT_ADDR`]); we read these
//! words through the memory-mapped window, validate them, and reuse the
//! existing trampoline ([`crate::launcher::launch_via_trampoline`]) to copy
//! `code_start..code_end` from flash into SRAM and jump to `code_execute`.

use rza1l_hal::spibsc;

use crate::elf::SramSegDesc;

/// Offsets of the FSB metadata words from the image base.
const META_CODE_START: u32 = 0x20;
const META_CODE_END: u32 = 0x24;
const META_CODE_EXECUTE: u32 = 0x28;
const META_SIGNATURE: u32 = 0x2C;

/// Signature string the FSB metadata carries (`.asciz` in `startup.rs`).
const SIGNATURE: &[u8] = b".BootLoad_ValidProgramTest.";

/// Lowest legal SRAM load address (retention RAM below this is reserved).
const SRAM_LOAD_ORIGIN: u32 = 0x2002_0000;
/// One past the top of usable on-chip SRAM.
const SRAM_END: u32 = 0x2030_0000;

/// Validated description of the on-flash firmware image.
#[derive(Clone, Copy)]
pub struct FlashImage {
    /// SRAM load address of image byte 0.
    pub code_start: u32,
    /// One past the last image byte.
    pub code_end: u32,
    /// Entry point.
    pub entry: u32,
}

#[inline]
fn read_word(off: u32) -> u32 {
    // Reads go through the cached memory-mapped window; flash is memory-mapped here.
    unsafe { core::ptr::read_volatile((spibsc::FLASH_SLOT_ADDR + off) as *const u32) }
}

#[inline]
fn read_byte(off: u32) -> u8 {
    unsafe { core::ptr::read_volatile((spibsc::FLASH_SLOT_ADDR + off) as *const u8) }
}

/// Inspect the flash slot and return a [`FlashImage`] if it holds a valid,
/// sane firmware image (correct signature and in-range metadata).
pub fn probe() -> Option<FlashImage> {
    // Signature must match exactly.
    for (i, &b) in SIGNATURE.iter().enumerate() {
        if read_byte(META_SIGNATURE + i as u32) != b {
            return None;
        }
    }

    let code_start = read_word(META_CODE_START);
    let code_end = read_word(META_CODE_END);
    let entry = read_word(META_CODE_EXECUTE);

    // Sanity-check the metadata before we trust it for a memcpy + jump.
    if code_start < SRAM_LOAD_ORIGIN || code_start >= SRAM_END {
        return None;
    }
    if code_end <= code_start || code_end > SRAM_END {
        return None;
    }
    if code_end - code_start > spibsc::FLASH_SLOT_LEN {
        return None;
    }
    if entry < code_start || entry >= code_end {
        return None;
    }

    Some(FlashImage {
        code_start,
        code_end,
        entry,
    })
}

impl FlashImage {
    /// Build the single trampoline descriptor that copies the image from the
    /// flash memory-mapped window into SRAM.
    ///
    /// `src` is the **real** SPIBSC window address (`0x1808_0000`), not the
    /// `0x5808_0000` uncached mirror: the trampoline runs with the MMU off, so
    /// only the physical SPIBSC AHB window is valid.  Reads happen with caches
    /// off, so the cached/uncached attribute is irrelevant.
    pub fn desc(&self) -> SramSegDesc {
        SramSegDesc {
            src: spibsc::FLASH_SLOT_ADDR,
            dst: self.code_start,
            filesz: self.code_end - self.code_start,
            zero_extra: 0,
        }
    }
}
