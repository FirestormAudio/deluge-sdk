//! Minimal streaming ELF32-LE loader for ARM firmware images.
//!
//! ## Load strategy
//!
//! An app image that targets upper SRAM (`0x20020000+`) would corrupt the
//! running bootloader if its `PT_LOAD` segments were written directly: the
//! bootloader's data, heap and stacks live there.  To avoid this, SRAM-targeting
//! segments are staged in SDRAM during the ELF read phase, then relocated to
//! their final SRAM addresses by a small trampoline running from data-retention
//! RAM (`0x20000000–0x2001FFFF`).  That region is untouched by both the
//! first-stage bootloader and the apps, so the trampoline is safe to execute
//! while SRAM is being overwritten.
//!
//! SDRAM-targeting segments (`0x0C000000–0x0EFFFFFF`) are written directly;
//! they cannot overwrite the bootloader.

use deluge_bsp::fat::{self, FatError, RawFile};
use embassy_time::{Duration, Timer};

/// ELF magic bytes.
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
/// ELF class: 32-bit.
const ELFCLASS32: u8 = 1;
/// ELF data encoding: little-endian.
const ELFDATA2LSB: u8 = 1;
/// ELF machine: ARM (Thumb/ARM).
const EM_ARM: u16 = 0x28;
/// Program header type: loadable segment.
const PT_LOAD: u32 = 1;

/// Maximum number of program headers processed.
const MAX_PHDRS: usize = 8;

/// Chunk size for streamed segment copies (one FAT sector).
const CHUNK: usize = 512;

// ---------------------------------------------------------------------------
// Staging constants and types
// ---------------------------------------------------------------------------

/// Base of the SRAM region that apps are permitted to target.
const SRAM_LOAD_ORIGIN: u32 = 0x2002_0000;

/// Staging area in SDRAM for SRAM-targeting segments.
///
/// A segment intended for SRAM address `p` is written to
/// `SDRAM_STAGE_BASE + (p - SRAM_LOAD_ORIGIN)` during the load phase.
/// The trampoline then copies it to `p` after the bootloader's own SRAM
/// is no longer needed.  The staging window is at most 2.875 MB
/// (0x0F000000–0x0F2DFFFF inclusive, exclusive end 0x0F2E0000),
/// safely within the 64 MB SDRAM range.
const SDRAM_STAGE_BASE: u32 = 0x0F00_0000;

/// Descriptor for one SRAM-targeting `PT_LOAD` segment.
///
/// Passed to [`crate::launcher::launch_via_trampoline`] after the ELF load
/// completes.  Layout is `repr(C)` so the trampoline blob can read it with
/// plain `ldr` instructions without any offset calculation.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct SramSegDesc {
    /// Source address (SDRAM staging area).
    pub src: u32,
    /// Final SRAM destination address.
    pub dst: u32,
    /// Bytes to copy from `src` to `dst`.
    pub filesz: u32,
    /// Additional bytes to zero after the copy (`p_memsz - p_filesz`).
    pub zero_extra: u32,
}

/// Result returned by a successful [`load_from_sd`] call.
pub struct LoadResult {
    /// Application entry point (`e_entry`).
    pub entry: u32,
    /// Descriptors for SRAM segments parked in SDRAM staging.
    pub sram_descs: [SramSegDesc; MAX_PHDRS],
    /// Number of valid entries in `sram_descs`.
    pub n_sram: usize,
}

/// ELF type: executable file.
const ET_EXEC: u16 = 2;

/// Errors from the ELF loader.
#[derive(Debug, Clone)]
pub enum ElfError {
    /// ELF magic bytes are not `\x7FELF`.
    BadMagic,
    /// Not a 32-bit little-endian ARM executable ELF.
    WrongFormat,
    /// A `PT_LOAD` segment's physical address is outside the permitted regions.
    BadLoadAddress,
    /// SD card / FAT read or seek error.
    Io(#[allow(dead_code)] FatError),
    /// `read` returned 0 before the segment was fully copied.
    UnexpectedEof,
}

impl From<FatError> for ElfError {
    fn from(e: FatError) -> Self {
        ElfError::Io(e)
    }
}

/// Read exactly `buf.len()` bytes from `file` into `buf`.
fn read_exact(
    vm: &mut fat::DelugeVolumeManager,
    file: RawFile,
    buf: &mut [u8],
) -> Result<(), ElfError> {
    let mut pos = 0;
    while pos < buf.len() {
        let n = vm.read(file, &mut buf[pos..])?;
        if n == 0 {
            return Err(ElfError::UnexpectedEof);
        }
        pos += n;
    }
    Ok(())
}

#[inline]
fn le16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(buf[off..off + 2].try_into().unwrap())
}

#[inline]
fn le32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

/// Resolve an uncached **mirror-alias** load address to the cached physical
/// address it shadows.
///
/// The RZ/A1L exposes non-cacheable mirrors of OCRAM and SDRAM at
/// `physical + 0x4000_0000` (see the MMU map in `rza1l_hal::mmu`):
///   * OCRAM mirror `0x6000_0000..0x60A0_0000` ⇒ OCRAM `0x2000_0000..0x20A0_0000`
///   * SDRAM mirror `0x4C00_0000..0x5000_0000` ⇒ SDRAM `0x0C00_0000..0x1000_0000`
///
/// Firmware images intentionally place explicitly non-cacheable sections (the
/// RTT control block, DMA buffers, …) at these mirror addresses.  For
/// load-range *classification* and SRAM *staging-offset* maths we must work
/// with the underlying physical address; the segment is still written through
/// the original (mirror) address so the application sees the non-cacheable
/// view it asked for.
fn mirror_to_phys(addr: u32) -> u32 {
    const OFF: u32 = 0x4000_0000; // rza1l_hal::UNCACHED_MIRROR_OFFSET
    if (0x6000_0000..0x60A0_0000).contains(&addr) || (0x4C00_0000..0x5000_0000).contains(&addr) {
        addr - OFF
    } else {
        addr
    }
}

/// Classify a `PT_LOAD` target range.
enum SegTarget {
    /// Segment targets SDRAM (0x0C000000–0x0EFFFFFF); write directly.
    Sdram,
    /// Segment targets upper SRAM (0x20020000–0x202FFFFF); must be staged.
    Sram,
}

/// Return the target classification for `addr..addr+len`, or `None` if the
/// range is outside every permitted region.
fn classify_load_range(addr: u32, len: u32) -> Option<SegTarget> {
    if len == 0 {
        // Empty segments are harmless; treat as SDRAM (no staging needed).
        return Some(SegTarget::Sdram);
    }
    let end = addr.checked_add(len)?;
    if addr >= 0x0C00_0000 && end <= 0x0F00_0000 {
        // SDRAM, but must not overlap the staging window itself.
        return Some(SegTarget::Sdram);
    }
    if addr >= 0x2002_0000 && end <= 0x2030_0000 {
        return Some(SegTarget::Sram);
    }
    None
}

/// Stream-load an ELF32 file from the SD card.
///
/// Parses program headers and processes all `PT_LOAD` segments.
///
/// - **SDRAM targets** (`0x0C000000–0x0EFFFFFF`): written to their final
///   addresses immediately (they cannot overwrite the bootloader).
/// - **SRAM targets** (`0x20020000–0x202FFFFF`): written to the SDRAM
///   staging area; the caller must invoke
///   [`crate::launcher::launch_via_trampoline`] to move them before
///   branching to the app.
///
/// Returns a [`LoadResult`] containing the entry point and any SRAM segment
/// descriptors that still need relocating.  The file cursor is moved
/// arbitrarily; the caller should close the file after this returns.
///
/// # Safety
/// Writes to physical RAM derived from ELF program headers.  Each
/// destination is validated before any write, but the caller must ensure no
/// live data occupies the SDRAM staging window (`0x0F000000–0x0F2FFFFF`).
pub async unsafe fn load_from_sd_with_progress<F, Fut>(
    vm: &mut fat::DelugeVolumeManager,
    file: RawFile,
    mut on_progress: F,
) -> Result<LoadResult, ElfError>
where
    F: FnMut(u32, u32) -> Fut,
    Fut: core::future::Future<Output = ()>,
{
    // 1. Read and validate the 52-byte ELF header.
    let mut hdr = [0u8; 52];
    read_exact(vm, file, &mut hdr)?;

    if hdr[0..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }
    if hdr[4] != ELFCLASS32 || hdr[5] != ELFDATA2LSB {
        return Err(ElfError::WrongFormat);
    }
    if le16(&hdr, 16) != ET_EXEC {
        return Err(ElfError::WrongFormat);
    }
    if le16(&hdr, 18) != EM_ARM {
        return Err(ElfError::WrongFormat);
    }

    let e_entry = le32(&hdr, 24);
    let e_phoff = le32(&hdr, 28);
    let e_phentsize = le16(&hdr, 42) as usize;
    let e_phnum = le16(&hdr, 44) as usize;

    if e_phentsize != 32 {
        return Err(ElfError::WrongFormat);
    }
    if e_phnum > MAX_PHDRS {
        return Err(ElfError::WrongFormat);
    }
    let phnum = e_phnum;

    // 2. Seek to and read all program headers into a stack buffer.
    //    MAX_PHDRS × 32 bytes = 256 bytes stack.
    vm.file_seek_from_start(file, e_phoff)?;
    let mut phdr_buf = [0u8; MAX_PHDRS * 32];
    read_exact(vm, file, &mut phdr_buf[..phnum * e_phentsize])?;

    // 3. Copy each PT_LOAD segment to its write destination.
    let mut chunk_buf = [0u8; CHUNK];

    let mut sram_descs = [SramSegDesc::default(); MAX_PHDRS];
    let mut n_sram: usize = 0;

    // Pre-compute total bytes that will be streamed from SD for true load progress.
    let mut total_bytes: u32 = 0;
    for i in 0..phnum {
        let ph = &phdr_buf[i * e_phentsize..][..32];
        if le32(ph, 0) != PT_LOAD {
            continue;
        }
        let p_paddr = le32(ph, 12);
        if (0x2000_0000..0x2002_0000).contains(&mirror_to_phys(p_paddr)) {
            continue;
        }
        total_bytes = total_bytes.saturating_add(le32(ph, 16));
    }
    let mut copied_bytes: u32 = 0;
    let mut last_percent: u8 = if total_bytes == 0 { 100 } else { 0 };
    on_progress(copied_bytes, total_bytes).await;

    for i in 0..phnum {
        let ph = &phdr_buf[i * e_phentsize..][..32];

        if le32(ph, 0) != PT_LOAD {
            continue;
        }

        let p_offset = le32(ph, 4);
        let p_paddr = le32(ph, 12);
        let p_filesz = le32(ph, 16) as usize;
        let p_memsz = le32(ph, 20) as usize;

        // A well-formed PT_LOAD always has filesz <= memsz.
        if p_filesz > p_memsz {
            return Err(ElfError::WrongFormat);
        }

        // Skip segments that target data-retention RAM (0x20000000-0x2001FFFF).
        // That region is reserved for the trampoline, and applications
        // place only self-zeroed BSS there (e.g. the community firmware's
        // `.frunk_bss` @ 0x20000000).  Writing it would corrupt the running
        // trampoline; the app's own startup zeroes it after handoff.
        if (0x2000_0000..0x2002_0000).contains(&mirror_to_phys(p_paddr)) {
            continue;
        }

        // Classify against the *physical* address (resolving any uncached
        // mirror alias), but keep writing through `p_paddr` so a segment that
        // asked for the non-cacheable view (e.g. the RTT buffer at 0x602b0000)
        // still lands there.
        let p_phys = mirror_to_phys(p_paddr);

        // Validate target address and determine where to write.
        let (write_addr, is_sram) = {
            let target =
                classify_load_range(p_phys, p_memsz as u32).ok_or(ElfError::BadLoadAddress)?;
            match target {
                SegTarget::Sdram => (p_paddr, false),
                SegTarget::Sram => (
                    // Staging slot is keyed by the physical SRAM offset.
                    SDRAM_STAGE_BASE + (p_phys - SRAM_LOAD_ORIGIN),
                    true,
                ),
            }
        };

        vm.file_seek_from_start(file, p_offset)?;

        let mut remaining = p_filesz;
        let mut dst = write_addr as *mut u8;

        unsafe {
            while remaining > 0 {
                let want = remaining.min(CHUNK);
                let n = vm.read(file, &mut chunk_buf[..want])?;
                if n == 0 {
                    return Err(ElfError::UnexpectedEof);
                }
                core::ptr::copy_nonoverlapping(chunk_buf.as_ptr(), dst, n);
                dst = dst.add(n);
                remaining -= n;

                copied_bytes = copied_bytes.saturating_add(n as u32);
                let percent = if total_bytes == 0 {
                    100
                } else {
                    ((copied_bytes.saturating_mul(100)) / total_bytes) as u8
                };
                if percent != last_percent {
                    on_progress(copied_bytes, total_bytes).await;
                    last_percent = percent;
                    // Let other tasks keep running while large images stream.
                    Timer::after(Duration::from_millis(0)).await;
                }
            }

            // Zero the BSS-like gap between filesz and memsz.
            if p_memsz > p_filesz {
                core::ptr::write_bytes(dst, 0, p_memsz - p_filesz);
            }
        }

        // Record SRAM segments so the caller can drive the trampoline.
        if is_sram {
            sram_descs[n_sram] = SramSegDesc {
                src: write_addr,
                dst: p_paddr,
                filesz: p_filesz as u32,
                zero_extra: (p_memsz - p_filesz) as u32,
            };
            n_sram += 1;
        }
    }

    if last_percent < 100 {
        on_progress(total_bytes, total_bytes).await;
    }

    Ok(LoadResult {
        entry: e_entry,
        sram_descs,
        n_sram,
    })
}
