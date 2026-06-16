//! Pure ELF32 helpers for the streaming SD app-loader: header validation,
//! uncached-mirror resolution, and `PT_LOAD` target classification + SRAM
//! staging-address math.
//!
//! The loader itself streams segments off the SD card and writes physical RAM
//! (hardware), but every *decision* it makes about where a segment may go — and
//! the staging arithmetic that keeps an SRAM-targeting segment from clobbering
//! the running bootloader — is pure and lives here so it can be unit-tested.

// --- ELF32 constants -------------------------------------------------------

/// ELF magic bytes.
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
/// ELF class: 32-bit.
pub const ELFCLASS32: u8 = 1;
/// ELF data encoding: little-endian.
pub const ELFDATA2LSB: u8 = 1;
/// ELF type: executable.
pub const ET_EXEC: u16 = 2;
/// ELF machine: ARM.
pub const EM_ARM: u16 = 0x28;
/// Program-header type: loadable segment.
pub const PT_LOAD: u32 = 1;

// --- Deluge load-region geometry -------------------------------------------

/// Uncached mirror alias offset (`rza1l_hal::UNCACHED_MIRROR_OFFSET`).
pub const UNCACHED_MIRROR_OFFSET: u32 = 0x4000_0000;

/// SDRAM region usable by app images: `0x0C000000..0x0F000000` (the top 1 MB is
/// reserved for the SRAM staging window, see [`SDRAM_STAGE_BASE`]).
pub const SDRAM_LO: u32 = 0x0C00_0000;
/// Exclusive end of the directly-writable SDRAM region.
pub const SDRAM_HI: u32 = 0x0F00_0000;

/// Upper-SRAM region apps may target: `0x20020000..0x20300000`.
pub const SRAM_LOAD_ORIGIN: u32 = 0x2002_0000;
/// Exclusive end of the permitted SRAM load region.
pub const SRAM_HI: u32 = 0x2030_0000;

/// Base of the SDRAM staging window for SRAM-targeting segments. A segment for
/// SRAM address `p` is parked at `SDRAM_STAGE_BASE + (p - SRAM_LOAD_ORIGIN)`.
pub const SDRAM_STAGE_BASE: u32 = 0x0F00_0000;

/// Maximum program headers the loader processes.
pub const MAX_PHDRS: usize = 8;

/// Where a `PT_LOAD` segment is allowed to land.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadTarget {
    /// SDRAM (`0x0C000000..0x0F000000`): written directly to its final address.
    Sdram,
    /// Upper SRAM (`0x20020000..0x20300000`): staged in SDRAM, relocated later.
    Sram,
}

/// Descriptor for one staged SRAM segment, handed to the relocation trampoline.
///
/// `repr(C)` so the trampoline can read fields with plain `ldr` at fixed
/// offsets; the field order/size is part of that ABI — do not reorder.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
#[repr(C)]
pub struct SegDesc {
    /// Source address in the SDRAM staging window.
    pub src: u32,
    /// Final SRAM destination address.
    pub dst: u32,
    /// Bytes to copy from `src` to `dst`.
    pub filesz: u32,
    /// Extra bytes to zero after the copy (`p_memsz - p_filesz`).
    pub zero_extra: u32,
}

/// Read a little-endian `u16` at `off`.
#[inline]
pub fn le16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

/// Read a little-endian `u32` at `off`.
#[inline]
pub fn le32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Validate the 52-byte ELF32 header as a little-endian ARM executable.
///
/// Returns `Ok(())` if the loader should accept the image; the streaming loader
/// runs this on the first header bytes before reading any program header.
pub fn validate_header(buf: &[u8]) -> Result<(), HeaderError> {
    if buf.len() < 52 {
        return Err(HeaderError::TooShort);
    }
    if buf[0..4] != ELF_MAGIC {
        return Err(HeaderError::BadMagic);
    }
    if buf[4] != ELFCLASS32 || buf[5] != ELFDATA2LSB {
        return Err(HeaderError::WrongFormat);
    }
    if le16(buf, 16) != ET_EXEC || le16(buf, 18) != EM_ARM {
        return Err(HeaderError::WrongFormat);
    }
    Ok(())
}

/// Reason [`validate_header`] rejected an image.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderError {
    /// Fewer than 52 bytes — not a complete ELF32 header.
    TooShort,
    /// Magic bytes are not `\x7FELF`.
    BadMagic,
    /// Not a 32-bit little-endian ARM executable.
    WrongFormat,
}

/// Resolve an uncached **mirror-alias** address to the physical address it
/// shadows. OCRAM/SDRAM are aliased at `physical + 0x4000_0000`; classification
/// and staging math must use the underlying physical address.
pub fn mirror_to_phys(addr: u32) -> u32 {
    if (0x6000_0000..0x60A0_0000).contains(&addr) || (0x4C00_0000..0x5000_0000).contains(&addr) {
        addr - UNCACHED_MIRROR_OFFSET
    } else {
        addr
    }
}

/// Classify the physical target range `addr..addr+len`, or `None` if it is
/// outside every region a `PT_LOAD` segment may occupy.
///
/// `addr` must already be [`mirror_to_phys`]-resolved.
pub fn classify_load_range(addr: u32, len: u32) -> Option<LoadTarget> {
    if len == 0 {
        // Empty segments are harmless; no staging needed.
        return Some(LoadTarget::Sdram);
    }
    let end = addr.checked_add(len)?;
    if addr >= SDRAM_LO && end <= SDRAM_HI {
        return Some(LoadTarget::Sdram);
    }
    if addr >= SRAM_LOAD_ORIGIN && end <= SRAM_HI {
        return Some(LoadTarget::Sram);
    }
    None
}

/// Staging address for an SRAM-targeting segment whose final address is `dst`.
///
/// `dst` must be in `[SRAM_LOAD_ORIGIN, SRAM_HI)` (i.e. classified as
/// [`LoadTarget::Sram`]); the result is inside the SDRAM staging window.
pub fn sram_stage_addr(dst: u32) -> u32 {
    SDRAM_STAGE_BASE + (dst - SRAM_LOAD_ORIGIN)
}

// --- FSB metadata --------------------------------------------------------

/// Image offsets of the first-stage-bootloader (FSB) metadata words, emitted by
/// `rza1l-hal/src/startup.rs` just past the eight-entry vector table.
pub const FSB_CODE_START: usize = 0x20;
/// Offset of the `code_end` word (one past the last image byte).
pub const FSB_CODE_END: usize = 0x24;
/// Offset of the `code_execute` (entry point) word.
pub const FSB_CODE_EXECUTE: usize = 0x28;
/// Offset of the validity signature.
pub const FSB_SIGNATURE_OFF: usize = 0x2C;
/// Signature string a bootable image carries at [`FSB_SIGNATURE_OFF`].
pub const FSB_SIGNATURE: &[u8] = b".BootLoad_ValidProgramTest.";

/// Validated FSB metadata read from a flat firmware image.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FsbMeta {
    /// Load address of image byte 0.
    pub code_start: u32,
    /// One past the last image byte (linker `end`, 64 KB-rounded).
    pub code_end: u32,
    /// Entry point.
    pub entry: u32,
}

/// Why [`validate_fsb_metadata`] rejected a flattened image.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsbError {
    /// Image is shorter than the metadata block at `+0x2C`.
    TooSmall,
    /// The `.BootLoad_ValidProgramTest.` signature is absent at `+0x2C`.
    BadSignature,
    /// `code_start` disagrees with the image's lowest load address.
    CodeStartMismatch,
    /// `code_end <= code_start`.
    BadCodeEnd,
    /// Entry point is outside `code_start..code_end`.
    EntryOutOfRange,
    /// Flat image extends past the span the FSB will copy (`code_end`).
    ImageTooLong,
}

/// Validate the FSB metadata embedded in a flattened (`objcopy -O binary`-style)
/// firmware image, the same checks the on-flash boot path applies before it will
/// copy and jump.  Catching a bad image here means it is refused *before* the
/// flash slot is erased, instead of silently failing to boot.
///
/// `image_base` is the lowest load address (LMA) of the flattened image — byte 0
/// of `image` — which must equal the metadata's `code_start`.
pub fn validate_fsb_metadata(image: &[u8], image_base: u32) -> Result<FsbMeta, FsbError> {
    if image.len() < FSB_SIGNATURE_OFF + FSB_SIGNATURE.len() {
        return Err(FsbError::TooSmall);
    }
    if &image[FSB_SIGNATURE_OFF..FSB_SIGNATURE_OFF + FSB_SIGNATURE.len()] != FSB_SIGNATURE {
        return Err(FsbError::BadSignature);
    }

    let code_start = le32(image, FSB_CODE_START);
    let code_end = le32(image, FSB_CODE_END);
    let entry = le32(image, FSB_CODE_EXECUTE);

    // Byte 0 of the image *is* code_start, so the metadata must agree with the
    // actual lowest LMA the linker emitted.
    if code_start != image_base {
        return Err(FsbError::CodeStartMismatch);
    }
    if code_end <= code_start {
        return Err(FsbError::BadCodeEnd);
    }
    if entry < code_start || entry >= code_end {
        return Err(FsbError::EntryOutOfRange);
    }
    // The flat image must fit inside the span the FSB copies. It is normally
    // *shorter* (code_end is 64 KB-rounded); only a longer image is a real bug.
    if image.len() > (code_end - code_start) as usize {
        return Err(FsbError::ImageTooLong);
    }

    Ok(FsbMeta {
        code_start,
        code_end,
        entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf_header(class: u8, data: u8, etype: u16, machine: u16) -> [u8; 52] {
        let mut h = [0u8; 52];
        h[0..4].copy_from_slice(&ELF_MAGIC);
        h[4] = class;
        h[5] = data;
        h[16..18].copy_from_slice(&etype.to_le_bytes());
        h[18..20].copy_from_slice(&machine.to_le_bytes());
        h
    }

    #[test]
    fn header_accepts_arm_le_exec() {
        let h = elf_header(ELFCLASS32, ELFDATA2LSB, ET_EXEC, EM_ARM);
        assert_eq!(validate_header(&h), Ok(()));
    }

    #[test]
    fn header_rejects_bad_inputs() {
        assert_eq!(validate_header(&[0u8; 10]), Err(HeaderError::TooShort));

        let mut h = elf_header(ELFCLASS32, ELFDATA2LSB, ET_EXEC, EM_ARM);
        h[1] = b'Z';
        assert_eq!(validate_header(&h), Err(HeaderError::BadMagic));

        assert_eq!(
            validate_header(&elf_header(2, ELFDATA2LSB, ET_EXEC, EM_ARM)),
            Err(HeaderError::WrongFormat),
            "ELFCLASS64"
        );
        assert_eq!(
            validate_header(&elf_header(ELFCLASS32, 2, ET_EXEC, EM_ARM)),
            Err(HeaderError::WrongFormat),
            "big-endian"
        );
        assert_eq!(
            validate_header(&elf_header(ELFCLASS32, ELFDATA2LSB, 1, EM_ARM)),
            Err(HeaderError::WrongFormat),
            "ET_REL"
        );
        assert_eq!(
            validate_header(&elf_header(ELFCLASS32, ELFDATA2LSB, ET_EXEC, 0x3E)),
            Err(HeaderError::WrongFormat),
            "x86-64"
        );
    }

    #[test]
    fn mirror_resolves_aliases() {
        // OCRAM mirror 0x6000_0000 → 0x2000_0000.
        assert_eq!(mirror_to_phys(0x6000_0000), 0x2000_0000);
        assert_eq!(mirror_to_phys(0x6009_FFFF), 0x2009_FFFF);
        // SDRAM mirror 0x4C00_0000 → 0x0C00_0000.
        assert_eq!(mirror_to_phys(0x4C00_0000), 0x0C00_0000);
        // Non-mirror addresses pass through unchanged.
        assert_eq!(mirror_to_phys(0x2002_0000), 0x2002_0000);
        assert_eq!(mirror_to_phys(0x0C00_0000), 0x0C00_0000);
        // Just outside the mirror windows: unchanged.
        assert_eq!(mirror_to_phys(0x60A0_0000), 0x60A0_0000);
        assert_eq!(mirror_to_phys(0x5000_0000), 0x5000_0000);
    }

    #[test]
    fn classify_sdram_and_sram() {
        assert_eq!(classify_load_range(SDRAM_LO, 0x1000), Some(LoadTarget::Sdram));
        assert_eq!(classify_load_range(SDRAM_HI - 1, 1), Some(LoadTarget::Sdram));
        assert_eq!(classify_load_range(SRAM_LOAD_ORIGIN, 0x1000), Some(LoadTarget::Sram));
        assert_eq!(classify_load_range(SRAM_HI - 4, 4), Some(LoadTarget::Sram));
    }

    #[test]
    fn classify_empty_segment_is_sdram() {
        // Even at an otherwise-illegal address, a zero-length segment is fine.
        assert_eq!(classify_load_range(0x0000_0000, 0), Some(LoadTarget::Sdram));
    }

    #[test]
    fn classify_rejects_out_of_region_and_overlaps() {
        // Below SDRAM.
        assert_eq!(classify_load_range(0x0800_0000, 0x10), None);
        // SDRAM segment spilling into the staging window is rejected.
        assert_eq!(classify_load_range(SDRAM_HI - 4, 0x100), None);
        // SRAM segment past the top of the region.
        assert_eq!(classify_load_range(SRAM_HI - 4, 0x100), None);
        // Gap between SDRAM and SRAM (e.g. low OCRAM the bootloader uses).
        assert_eq!(classify_load_range(0x2000_0000, 0x10), None);
        // Address+len overflow must not panic.
        assert_eq!(classify_load_range(0xFFFF_FF00, 0x200), None);
    }

    #[test]
    fn staging_address_math() {
        // Byte 0 of the SRAM region maps to the staging base.
        assert_eq!(sram_stage_addr(SRAM_LOAD_ORIGIN), SDRAM_STAGE_BASE);
        // Offset within SRAM is preserved into the staging window.
        assert_eq!(sram_stage_addr(SRAM_LOAD_ORIGIN + 0x1234), SDRAM_STAGE_BASE + 0x1234);
        // The whole SRAM region stays inside SDRAM (staging window ≤ ~2.875 MB).
        let top = sram_stage_addr(SRAM_HI - 1);
        assert!(top < 0x1000_0000, "staging must stay within 64 MB SDRAM");
    }

    /// Build a minimal flat image carrying valid FSB metadata.
    fn fsb_image(code_start: u32, code_end: u32, entry: u32, len: usize) -> Vec<u8> {
        let mut img = vec![0u8; len.max(FSB_SIGNATURE_OFF + FSB_SIGNATURE.len())];
        img[FSB_CODE_START..FSB_CODE_START + 4].copy_from_slice(&code_start.to_le_bytes());
        img[FSB_CODE_END..FSB_CODE_END + 4].copy_from_slice(&code_end.to_le_bytes());
        img[FSB_CODE_EXECUTE..FSB_CODE_EXECUTE + 4].copy_from_slice(&entry.to_le_bytes());
        img[FSB_SIGNATURE_OFF..FSB_SIGNATURE_OFF + FSB_SIGNATURE.len()]
            .copy_from_slice(FSB_SIGNATURE);
        img
    }

    #[test]
    fn fsb_accepts_well_formed_image() {
        let img = fsb_image(0x2002_0000, 0x2003_0000, 0x2002_0100, 0x800);
        assert_eq!(
            validate_fsb_metadata(&img, 0x2002_0000),
            Ok(FsbMeta {
                code_start: 0x2002_0000,
                code_end: 0x2003_0000,
                entry: 0x2002_0100,
            })
        );
    }

    #[test]
    fn fsb_rejects_bad_images() {
        // Too short to hold the metadata block.
        assert_eq!(validate_fsb_metadata(&[0u8; 16], 0), Err(FsbError::TooSmall));

        // Missing signature.
        let mut img = fsb_image(0x2002_0000, 0x2003_0000, 0x2002_0100, 0x800);
        img[FSB_SIGNATURE_OFF] = 0;
        assert_eq!(validate_fsb_metadata(&img, 0x2002_0000), Err(FsbError::BadSignature));

        // code_start disagrees with the image's lowest load address.
        let img = fsb_image(0x2002_0000, 0x2003_0000, 0x2002_0100, 0x800);
        assert_eq!(
            validate_fsb_metadata(&img, 0x2002_1000),
            Err(FsbError::CodeStartMismatch)
        );

        // code_end <= code_start.
        let img = fsb_image(0x2002_0000, 0x2002_0000, 0x2002_0000, 0x800);
        assert_eq!(validate_fsb_metadata(&img, 0x2002_0000), Err(FsbError::BadCodeEnd));

        // Entry outside code_start..code_end.
        let img = fsb_image(0x2002_0000, 0x2003_0000, 0x2003_0000, 0x800);
        assert_eq!(
            validate_fsb_metadata(&img, 0x2002_0000),
            Err(FsbError::EntryOutOfRange)
        );

        // Flat image longer than code_end - code_start.
        let img = fsb_image(0x2002_0000, 0x2002_0100, 0x2002_0000, 0x800);
        assert_eq!(validate_fsb_metadata(&img, 0x2002_0000), Err(FsbError::ImageTooLong));
    }

    #[test]
    fn seg_desc_is_repr_c_four_u32() {
        // The trampoline reads this by fixed offset; lock the layout.
        assert_eq!(core::mem::size_of::<SegDesc>(), 16);
        assert_eq!(core::mem::align_of::<SegDesc>(), 4);
    }
}
