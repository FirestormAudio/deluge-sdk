//! Boot-from-flash support.
//!
//! In addition to the SD-card `/APPS/` images, the app loader can launch a
//! firmware image stored directly in the SPI flash chip — the same chip that
//! holds the first-stage bootloader and the loader itself.  This lets a unit boot with no SD
//! card present, like the original Deluge bootloader.
//!
//! ## Image format
//! The app slot ([`flash::SLOT_OFFSET`]) holds a DAPP image ([`deluge_image::app`]): a 64-byte
//! header (load address, entry, body length, body CRC, the OLED variant) and the body right after
//! it, an SRAM-linked flat image. [`probe`] checks the header when the menu is built; the whole
//! image, body CRC included, is checked at launch ([`launch_with_progress`]), and the trampoline
//! ([`super::launcher::launch_via_trampoline`]) copies the body from the flash window to its load
//! address and jumps to the entry. The body must zero its own `.bss`: only its bytes are copied.

use deluge_bsp::flash;
use deluge_image::app::{APP_HEADER_LEN, AppError, AppHeader, BodyCheck, validate_app_header};
use deluge_image::slot::VARIANT_OLED;
use deluge_loader::stage::{StageError, bootable_header, flash_app_move};

use super::elf::SramSegDesc;

/// Lowest legal SRAM load address (retention RAM below this is reserved).
const SRAM_LOAD_ORIGIN: u32 = 0x2002_0000;
/// One past the top of usable on-chip SRAM.
const SRAM_END: u32 = 0x2030_0000;
/// The largest body the slot holds.
pub const MAX_BODY: u32 = flash::SLOT_LEN - APP_HEADER_LEN as u32;

/// The slot's first `len` bytes, through the memory-mapped window.
fn window(len: usize) -> &'static [u8] {
    // SAFETY: the app slot is inside the always-mapped SPIBSC read window, and `len` never
    // exceeds the slot (callers bound it by a checked header).
    unsafe { core::slice::from_raw_parts(flash::SLOT_ADDR as *const u8, len) }
}

/// The flash app's header, if the slot holds one that loads inside SRAM (the body is checked at
/// launch).
pub fn probe() -> Option<AppHeader> {
    bootable_header(
        window(APP_HEADER_LEN),
        VARIANT_OLED,
        MAX_BODY,
        SRAM_LOAD_ORIGIN..SRAM_END,
    )
}

/// Why the flash app can't be launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashAppError {
    /// The image failed a check (its header or its body CRC).
    Image(AppError),
    /// It loads outside the SRAM apps live in.
    BadLoadAddress(StageError),
}

/// Body bytes checked between progress reports.
const PROGRESS_CHUNK: usize = 64 * 1024;

/// The flash app, checked whole (header and body CRC) through the window: the trampoline's copy
/// straight from the flash window, and the entry. `on_progress(done, total)` (body bytes) is
/// awaited after each [`PROGRESS_CHUNK`] of the body CRC.
///
/// The copy's source is the **real** SPIBSC window address (`0x1810_0000`), not the uncached
/// mirror: the trampoline runs with the MMU off, so only the physical window is valid.
pub async fn launch_with_progress<F, Fut>(
    h: &AppHeader,
    mut on_progress: F,
) -> Result<(SramSegDesc, u32), FlashAppError>
where
    F: FnMut(u32, u32) -> Fut,
    Fut: core::future::Future<Output = ()>,
{
    let whole = window(APP_HEADER_LEN + h.body_len as usize);
    let h = validate_app_header(whole, &[VARIANT_OLED], MAX_BODY).map_err(FlashAppError::Image)?;
    let mut check = BodyCheck::new(h);
    let h = loop {
        let step = check.step(whole, PROGRESS_CHUNK);
        on_progress(check.done(), check.total()).await;
        if let Some(r) = step {
            break r.map_err(FlashAppError::Image)?;
        }
    };
    let m = flash_app_move(&h, flash::SLOT_ADDR, SRAM_LOAD_ORIGIN..SRAM_END)
        .map_err(FlashAppError::BadLoadAddress)?;
    let desc = SramSegDesc {
        src: m.from,
        dst: m.to,
        filesz: m.len,
        zero_extra: 0,
    };
    Ok((desc, h.entry))
}

/// Why writing the app slot failed. The slot may be partly written: it then reads invalid, and
/// the menu still works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashWriteError {
    /// Bigger than the slot.
    TooLarge,
    /// Didn't read back as written, at this byte of the image.
    Verify(u32),
}

/// Erase the app slot and program `body`, then `header` last, reading each back. Until the
/// header is in, the slot reads as empty: a write that fails or is cut off part-way never leaves
/// a header over a body that doesn't match it. `on_progress(done, total)` is called between 4 KB
/// chunks. On return the CPU caches hold nothing stale for the slot.
///
/// # Safety
/// Erases and programs the flash app slot; no code may run from flash during the call (true for
/// the app loader, which executes from SRAM).
pub async unsafe fn program_slot<F, Fut>(
    header: &[u8; APP_HEADER_LEN],
    body: &[u8],
    mut on_progress: F,
) -> Result<(), FlashWriteError>
where
    F: FnMut(u32, u32) -> Fut,
    Fut: core::future::Future<Output = ()>,
{
    let total = APP_HEADER_LEN + body.len();
    // The `spibsc` guards silently drop writes past the slot: refuse before erasing.
    if total as u32 > flash::SLOT_LEN {
        return Err(FlashWriteError::TooLarge);
    }
    on_progress(0, total as u32).await;

    // SAFETY: inside the app slot, which is all `erase_range`/`program` will touch.
    unsafe { flash::MAP.erase_range(flash::SLOT_OFFSET, total as u32) };

    // Program in chunks so the OLED can show progress. `FlashMap::program` splits each call into
    // ≤256-byte page writes and restores memory-mapped read mode (flushing the read cache).
    const CHUNK: usize = 0x1000;
    let base = flash::SLOT_OFFSET + APP_HEADER_LEN as u32;
    let mut off = 0usize;
    while off < body.len() {
        let n = CHUNK.min(body.len() - off);
        unsafe { flash::MAP.program(base + off as u32, &body[off..off + n]) };
        off += n;
        on_progress(off as u32, total as u32).await;
    }
    let result = verify(APP_HEADER_LEN, body).and_then(|()| {
        unsafe { flash::MAP.program(flash::SLOT_OFFSET, header) };
        verify(0, header)
    });

    // `probe` and `launch` read the slot through the cached window: drop any lines an earlier
    // read left, so they see what was just written.
    // SAFETY: cache maintenance over the slot's mapped window; nothing in it is dirty (it's
    // only ever read through this mapping).
    unsafe {
        rza1_hal::cache::dma_clean_inv_range(
            flash::SLOT_ADDR as usize,
            (flash::SLOT_ADDR + total as u32) as usize,
        )
    };
    on_progress(total as u32, total as u32).await;
    result
}

/// Read `want` back from the slot at `at`, through the **uncached** mirror (`+0x4000_0000`): the
/// SPIBSC layer recovers from a rejected erase/program (stuck WIP → Clear-Status) without
/// surfacing an error, and L1/L2 may hold stale lines for the cached window.
fn verify(at: usize, want: &[u8]) -> Result<(), FlashWriteError> {
    const UNCACHED_MIRROR: u32 = 0x4000_0000;
    for (i, &w) in want.iter().enumerate() {
        let addr = flash::SLOT_ADDR + UNCACHED_MIRROR + (at + i) as u32;
        // SAFETY: inside the slot's uncached mirror, always mapped.
        if unsafe { core::ptr::read_volatile(addr as *const u8) } != w {
            return Err(FlashWriteError::Verify((at + i) as u32));
        }
    }
    Ok(())
}
