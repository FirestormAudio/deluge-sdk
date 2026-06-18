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
//! The image lives at the flash slot ([`flash::SLOT_ADDR`]); we read these
//! words through the memory-mapped window, validate them, and reuse the
//! existing trampoline ([`crate::launcher::launch_via_trampoline`]) to copy
//! `code_start..code_end` from flash into SRAM and jump to `code_execute`.

use deluge_bsp::flash;
use deluge_image::elf::{FsbError, validate_fsb_metadata};

use crate::elf::{FlashStage, SramSegDesc};

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
    unsafe { core::ptr::read_volatile((flash::SLOT_ADDR + off) as *const u32) }
}

#[inline]
fn read_byte(off: u32) -> u8 {
    unsafe { core::ptr::read_volatile((flash::SLOT_ADDR + off) as *const u8) }
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
    if !(SRAM_LOAD_ORIGIN..SRAM_END).contains(&code_start) {
        return None;
    }
    if code_end <= code_start || code_end > SRAM_END {
        return None;
    }
    if code_end - code_start > flash::SLOT_LEN {
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
    /// `src` is the **real** SPIBSC window address (`0x1810_0000`), not the
    /// `0x5810_0000` uncached mirror: the trampoline runs with the MMU off, so
    /// only the physical SPIBSC AHB window is valid.  Reads happen with caches
    /// off, so the cached/uncached attribute is irrelevant.
    pub fn desc(&self) -> SramSegDesc {
        SramSegDesc {
            src: flash::SLOT_ADDR,
            dst: self.code_start,
            filesz: self.code_end - self.code_start,
            zero_extra: 0,
        }
    }
}

/// Program a flattened firmware image (staged in SDRAM by
/// [`crate::elf::flatten_to_flash_staging`]) into the flash app slot.
///
/// The image's FSB metadata is validated **before** any erase, so an image the
/// boot path would reject is refused without touching the slot — it can never
/// brick an existing stored image. After a successful return the next
/// [`probe`] sees the new image (`FlashMap::program` flushes the read cache).
///
/// # Safety
/// Erases and programs the flash app slot; no code may run from flash during the
/// call. Safe here because the SSB executes from SRAM. Reads the staging window
/// via `stage.ptr` for `stage.len` bytes, which the caller must keep valid.
pub async unsafe fn store_image_to_slot<F, Fut>(
    stage: &FlashStage,
    mut on_progress: F,
) -> Result<(), FsbError>
where
    F: FnMut(u32, u32) -> Fut,
    Fut: core::future::Future<Output = ()>,
{
    let image = unsafe { core::slice::from_raw_parts(stage.ptr, stage.len) };

    // Reject anything that would overrun the slot *before* erasing.  The
    // per-sector/page guards in `spibsc` already refuse writes past the slot, but
    // they do so silently (a too-large image would erase + program up to the slot
    // boundary and drop the rest), leaving a truncated, unbootable image with no
    // error surfaced.  Catch the misfit here so the slot is never touched and the
    // caller sees a clear failure.  (The SD flatten path also checks this against
    // the staging buffer; this is the choke point that owns the slot invariant.)
    if stage.len as u32 > flash::SLOT_LEN {
        return Err(FsbError::TooLargeForSlot);
    }

    // Validate before erasing so a bad image cannot brick the slot.
    let meta = validate_fsb_metadata(image, stage.code_start)?;

    // The flat image (`stage.len`) is normally *shorter* than the span the FSB
    // copies (`code_end - code_start`, 64 KB-rounded with any BSS tail). `probe`
    // refuses to boot an image whose copy span overruns the slot, so reject one
    // here too — otherwise a pathological image with a small body but a huge
    // `code_end` would flash "OK" yet never appear as BOOT FLASH.
    if meta.code_end - meta.code_start > flash::SLOT_LEN {
        return Err(FsbError::TooLargeForSlot);
    }

    let len = stage.len as u32;
    on_progress(0, len).await;

    // Erase every 64 KB block the image spans.
    unsafe { flash::MAP.erase_range(flash::SLOT_OFFSET, len) };

    // Program in chunks so the OLED can show progress. `FlashMap::program`
    // internally splits each call into ≤256-byte page writes and restores
    // memory-mapped read mode (flushing the read cache) when it returns.
    const CHUNK: usize = 0x1000; // 4 KB
    let mut off = 0usize;
    while off < image.len() {
        let n = CHUNK.min(image.len() - off);
        unsafe { flash::MAP.program(flash::SLOT_OFFSET + off as u32, &image[off..off + n]) };
        off += n;
        on_progress(off as u32, len).await;
    }

    // Read the whole image back and compare. The SPIBSC layer recovers from a
    // rejected erase/program (stuck WIP → Clear-Status) *without surfacing an
    // error*, so a write that silently dropped part of the image would otherwise
    // flash "OK" yet store a corrupt, unbootable image. Read through the
    // **uncached** SPIBSC mirror (`+0x4000_0000`, mapped non-cached): `program`
    // already flushed the SPIBSC read cache, but the CPU's L1/L2 may still hold
    // stale lines for the slot from an earlier `probe`, and this is also exactly
    // the physical flash the boot trampoline copies from.
    const UNCACHED_MIRROR: u32 = 0x4000_0000;
    for (i, &want) in image.iter().enumerate() {
        let addr = flash::SLOT_ADDR + UNCACHED_MIRROR + i as u32;
        let got = unsafe { core::ptr::read_volatile(addr as *const u8) };
        if got != want {
            return Err(FsbError::VerifyFailed(i as u32));
        }
    }

    Ok(())
}
