//! `elf2uf2` — convert a RAM-linked ARM firmware ELF into a Deluge-SSB `.uf2`.
//!
//! This is the single-step replacement for the old `objcopy -O binary` +
//! `uf2conv.py` pipeline (see `tools/uf2/README.md`).  It takes the *same* ELF
//! that the second-stage bootloader's SD-card path consumes
//! (`app-loader/src/elf.rs`) and emits a `.uf2` that the USB
//! update path flashes into the app slot (`app-loader/src/uf2.rs`).
//!
//! ## What it does
//!  1. Parses the ELF32-LE ARM executable and flattens its `PT_LOAD` segments
//!     into a contiguous image keyed by physical address (LMA) — exactly what
//!     `objcopy -O binary` produces, i.e. the raw `.bin` the on-flash boot path
//!     ([`flashboot.rs`]) expects.
//!  2. Validates the embedded FSB metadata at `bin + 0x20`
//!     (`code_start` / `code_end` / `code_execute` + the
//!     `.BootLoad_ValidProgramTest.` signature) so a non-bootable image is
//!     rejected on the host instead of bricking a boot.
//!  3. Wraps the image in UF2 blocks targeting the app slot
//!     (`--base`, default `0x18100000`), stamped with the Deluge family ID
//!     (`--family`, default `0x6E275A1C`).
//!
//! Keep the defaults in sync with `rza1l-hal/src/spibsc.rs` and
//! `app-loader/src/uf2.rs`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- Defaults that mirror the firmware-side constants ----------------------

/// Flash app-slot memory-mapped base — `spibsc::FLASH_SLOT_ADDR`.
/// This is the `targetAddr` of UF2 block 0; image byte `i` maps to `BASE + i`.
const DEFAULT_BASE: u32 = 0x1810_0000;
/// Flash app-slot length — `spibsc::FLASH_SLOT_LEN` (3 MB).
const SLOT_LEN: u32 = 0x0030_0000;
/// Custom Deluge SSB family ID — `uf2::UF2_FAMILY_DELUGE`.
const DEFAULT_FAMILY: u32 = 0x6E27_5A1C;
/// Payload bytes per UF2 block (one NOR page) — `uf2::DUMP_PAYLOAD`.
const PAYLOAD: u32 = 256;

// --- FSB metadata layout (rza1l-hal/src/startup.rs) ------------------------

const META_CODE_START: usize = 0x20;
const META_CODE_END: usize = 0x24;
const META_CODE_EXECUTE: usize = 0x28;
const META_SIGNATURE: usize = 0x2C;
const SIGNATURE: &[u8] = b".BootLoad_ValidProgramTest.";

// --- UF2 block format (https://github.com/microsoft/uf2) -------------------

const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY_PRESENT: u32 = 0x0000_2000;

// --- ELF32 constants -------------------------------------------------------

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS32: u8 = 1;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const EM_ARM: u16 = 0x28;
const PT_LOAD: u32 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("elf2uf2: error: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    input: PathBuf,
    output: PathBuf,
    base: u32,
    family: u32,
    /// Also write the intermediate flat `.bin` next to the UF2.
    emit_bin: bool,
    /// Skip the FSB-metadata sanity check (program at your own risk).
    skip_fsb_check: bool,
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;

    let elf = std::fs::read(&args.input)
        .map_err(|e| format!("reading {}: {e}", args.input.display()))?;

    let (image, image_base) = flatten_elf(&elf)?;

    if !args.skip_fsb_check {
        check_fsb_metadata(&image, image_base, args.base)?;
    }

    if image.len() as u32 > SLOT_LEN {
        return Err(format!(
            "image is {} bytes but the app slot is only {} bytes ({} KB)",
            image.len(),
            SLOT_LEN,
            SLOT_LEN / 1024
        ));
    }

    let uf2 = build_uf2(&image, args.base, args.family);
    std::fs::write(&args.output, &uf2)
        .map_err(|e| format!("writing {}: {e}", args.output.display()))?;

    let blocks = uf2.len() / 512;
    println!(
        "elf2uf2: {} -> {} ({} bytes image, {blocks} blocks, base {:#010x}, family {:#010x})",
        args.input.display(),
        args.output.display(),
        image.len(),
        args.base,
        args.family,
    );

    if args.emit_bin {
        let bin_path = args.output.with_extension("bin");
        std::fs::write(&bin_path, &image)
            .map_err(|e| format!("writing {}: {e}", bin_path.display()))?;
        println!("elf2uf2: also wrote {}", bin_path.display());
    }

    Ok(())
}

/// Parse `PT_LOAD` segments and flatten them into a contiguous image, keyed by
/// physical address (LMA) — equivalent to `arm-none-eabi-objcopy -O binary`.
///
/// Returns the flattened image and the lowest LMA it starts at (which, for a
/// well-formed firmware, equals the FSB `code_start`).
fn flatten_elf(elf: &[u8]) -> Result<(Vec<u8>, u32), String> {
    if elf.len() < 52 {
        return Err("file is too short to be an ELF32".into());
    }
    if elf[0..4] != ELF_MAGIC {
        return Err("not an ELF file (bad magic)".into());
    }
    if elf[4] != ELFCLASS32 || elf[5] != ELFDATA2LSB {
        return Err("not a 32-bit little-endian ELF".into());
    }
    if le16(elf, 16) != ET_EXEC {
        return Err("not an ET_EXEC executable ELF".into());
    }
    if le16(elf, 18) != EM_ARM {
        return Err("not an EM_ARM (machine 0x28) ELF".into());
    }

    let e_phoff = le32(elf, 28) as usize;
    let e_phentsize = le16(elf, 42) as usize;
    let e_phnum = le16(elf, 44) as usize;
    if e_phentsize < 32 {
        return Err(format!("unexpected program-header size {e_phentsize}"));
    }

    // Collect loadable segments with file content.
    struct Seg {
        paddr: u32,
        offset: usize,
        filesz: usize,
    }
    let mut segs: Vec<Seg> = Vec::new();
    for i in 0..e_phnum {
        let ph = e_phoff + i * e_phentsize;
        if ph + 32 > elf.len() {
            return Err("program header table runs past end of file".into());
        }
        if le32(elf, ph) != PT_LOAD {
            continue;
        }
        let p_offset = le32(elf, ph + 4) as usize;
        let p_paddr = le32(elf, ph + 12);
        let p_filesz = le32(elf, ph + 16) as usize;
        if p_filesz == 0 {
            continue; // .bss-only segment: nothing to flash.
        }
        if p_offset + p_filesz > elf.len() {
            return Err("a PT_LOAD segment points past end of file".into());
        }
        segs.push(Seg {
            paddr: p_paddr,
            offset: p_offset,
            filesz: p_filesz,
        });
    }

    if segs.is_empty() {
        return Err("no loadable PT_LOAD segments with content found".into());
    }

    let lo = segs.iter().map(|s| s.paddr).min().unwrap();
    let hi = segs
        .iter()
        .map(|s| s.paddr + s.filesz as u32)
        .max()
        .unwrap();

    let mut image = vec![0u8; (hi - lo) as usize];
    for s in &segs {
        let dst = (s.paddr - lo) as usize;
        image[dst..dst + s.filesz].copy_from_slice(&elf[s.offset..s.offset + s.filesz]);
    }
    Ok((image, lo))
}

/// Reject images that the on-flash boot path ([`flashboot::probe`]) would refuse,
/// so the failure surfaces here instead of as a silent non-boot on the device.
///
/// `image_base` is the lowest LMA of the flattened image (its byte 0). The FSB
/// copies `code_start..code_end` out of the slot into SRAM; `code_end` is the
/// linker's 64 KB-aligned `end`, so it is normally *larger* than the flat image
/// — the rounded-up tail lands in the firmware's heap region and is harmless.
fn check_fsb_metadata(image: &[u8], image_base: u32, base: u32) -> Result<(), String> {
    if image.len() < META_SIGNATURE + SIGNATURE.len() {
        return Err(
            "image is too small to contain FSB metadata; is this a RAM-linked firmware ELF? \
             (pass --skip-fsb-check to override)"
                .into(),
        );
    }
    if &image[META_SIGNATURE..META_SIGNATURE + SIGNATURE.len()] != SIGNATURE {
        return Err(
            "FSB signature \".BootLoad_ValidProgramTest.\" not found at +0x2C; the on-flash \
             bootloader will not boot this image (pass --skip-fsb-check to override)"
                .into(),
        );
    }

    let code_start = le32(image, META_CODE_START);
    let code_end = le32(image, META_CODE_END);
    let code_execute = le32(image, META_CODE_EXECUTE);

    // Byte 0 of the image *is* code_start, so the metadata must agree with the
    // actual lowest LMA the linker emitted. A mismatch means the metadata and
    // the layout disagree — exactly the kind of bug worth catching on the host.
    if code_start != image_base {
        return Err(format!(
            "FSB code_start {code_start:#010x} disagrees with the image's lowest load \
             address {image_base:#010x}"
        ));
    }
    if code_end <= code_start {
        return Err(format!(
            "FSB metadata is inconsistent: code_end {code_end:#010x} <= code_start {code_start:#010x}"
        ));
    }
    if code_execute < code_start || code_execute >= code_end {
        return Err(format!(
            "FSB entry {code_execute:#010x} is outside code_start..code_end \
             ({code_start:#010x}..{code_end:#010x})"
        ));
    }
    // The flat image must fit inside the span the FSB will copy. It is normally
    // shorter (code_end is 64 KB-rounded); only a *longer* image is a real bug.
    let coded_len = code_end - code_start;
    if image.len() > coded_len as usize {
        return Err(format!(
            "image ({} bytes) extends past FSB code_end (span {coded_len} bytes); \
             loadable data lies beyond the region the bootloader copies",
            image.len()
        ));
    }

    // Purely informational, but a sanity anchor for the operator.
    println!(
        "elf2uf2: FSB ok — load {code_start:#010x}, entry {code_execute:#010x}, \
         {} image bytes (FSB copies {coded_len}) -> flash {base:#010x}",
        image.len()
    );
    Ok(())
}

/// Wrap a flat image in 512-byte UF2 blocks targeting `base..base+len`.
fn build_uf2(image: &[u8], base: u32, family: u32) -> Vec<u8> {
    let payload = PAYLOAD as usize;
    let num_blocks = image.len().div_ceil(payload) as u32;
    let mut out = Vec::with_capacity(num_blocks as usize * 512);

    for block_no in 0..num_blocks {
        let off = block_no as usize * payload;
        let chunk = &image[off..(off + payload).min(image.len())];

        let mut blk = [0u8; 512];
        put32(&mut blk, 0x00, UF2_MAGIC_START0);
        put32(&mut blk, 0x04, UF2_MAGIC_START1);
        put32(&mut blk, 0x08, UF2_FLAG_FAMILY_PRESENT);
        put32(&mut blk, 0x0C, base + off as u32);
        put32(&mut blk, 0x10, PAYLOAD);
        put32(&mut blk, 0x14, block_no);
        put32(&mut blk, 0x18, num_blocks);
        put32(&mut blk, 0x1C, family);
        blk[0x20..0x20 + chunk.len()].copy_from_slice(chunk);
        put32(&mut blk, 0x1FC, UF2_MAGIC_END);
        out.extend_from_slice(&blk);
    }
    out
}

// --- arg parsing -----------------------------------------------------------

fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut base = DEFAULT_BASE;
    let mut family = DEFAULT_FAMILY;
    let mut emit_bin = false;
    let mut skip_fsb_check = false;

    let mut it = args;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-o" | "--output" => {
                output = Some(PathBuf::from(
                    it.next().ok_or("--output needs a path")?,
                ));
            }
            "--base" => base = parse_u32(&it.next().ok_or("--base needs a value")?)?,
            "--family" => family = parse_u32(&it.next().ok_or("--family needs a value")?)?,
            "--bin" => emit_bin = true,
            "--skip-fsb-check" => skip_fsb_check = true,
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ => {
                if input.is_some() {
                    return Err("more than one input file given".into());
                }
                input = Some(PathBuf::from(arg));
            }
        }
    }

    let input = input.ok_or("no input ELF given (try --help)")?;
    let output = output.unwrap_or_else(|| default_output(&input));
    Ok(Args {
        input,
        output,
        base,
        family,
        emit_bin,
        skip_fsb_check,
    })
}

fn default_output(input: &Path) -> PathBuf {
    let mut p = input.to_path_buf();
    p.set_extension("uf2");
    p
}

fn parse_u32(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(h, 16)
    } else {
        t.parse::<u32>()
    };
    v.map_err(|_| format!("invalid 32-bit number: {s}"))
}

fn print_usage() {
    println!(
        "elf2uf2 — convert a RAM-linked ARM firmware ELF into a Deluge-SSB .uf2\n\
         \n\
         USAGE:\n\
             elf2uf2 <input.elf> [-o <output.uf2>] [options]\n\
         \n\
         OPTIONS:\n\
             -o, --output <path>   Output .uf2 path (default: input with .uf2 ext)\n\
             --base <addr>         Flash target base address (default: {DEFAULT_BASE:#010x})\n\
             --family <id>         UF2 family ID (default: {DEFAULT_FAMILY:#010x})\n\
             --bin                 Also write the intermediate flat .bin\n\
             --skip-fsb-check      Skip the FSB-metadata bootability check\n\
             -h, --help            Show this help\n\
         \n\
         The defaults match spibsc::FLASH_SLOT_ADDR and uf2::UF2_FAMILY_DELUGE."
    );
}

// --- little-endian helpers -------------------------------------------------

#[inline]
fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[inline]
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// One loadable segment for the ELF builder: physical load address + bytes.
    struct Seg {
        paddr: u32,
        data: Vec<u8>,
    }

    /// Build a minimal but well-formed ELF32-LE ARM executable carrying the
    /// given `PT_LOAD` segments. Layout: 52-byte header, then `n` 32-byte
    /// program headers, then each segment's bytes back to back.
    fn build_elf(segs: &[Seg]) -> Vec<u8> {
        const EHSIZE: usize = 52;
        const PHENTSIZE: usize = 32;
        let phoff = EHSIZE;
        let data_off = phoff + segs.len() * PHENTSIZE;

        let total = data_off + segs.iter().map(|s| s.data.len()).sum::<usize>();
        let mut elf = vec![0u8; total];

        // ELF header.
        elf[0..4].copy_from_slice(&ELF_MAGIC);
        elf[4] = ELFCLASS32;
        elf[5] = ELFDATA2LSB;
        elf[6] = 1; // EI_VERSION
        put16(&mut elf, 16, ET_EXEC);
        put16(&mut elf, 18, EM_ARM);
        put32(&mut elf, 24, segs.first().map_or(0, |s| s.paddr)); // e_entry
        put32(&mut elf, 28, phoff as u32);
        put16(&mut elf, 40, EHSIZE as u16);
        put16(&mut elf, 42, PHENTSIZE as u16);
        put16(&mut elf, 44, segs.len() as u16);

        // Program headers + segment bodies.
        let mut body = data_off;
        for (i, s) in segs.iter().enumerate() {
            let ph = phoff + i * PHENTSIZE;
            put32(&mut elf, ph, PT_LOAD);
            put32(&mut elf, ph + 4, body as u32); // p_offset
            put32(&mut elf, ph + 8, s.paddr); // p_vaddr
            put32(&mut elf, ph + 12, s.paddr); // p_paddr (LMA)
            put32(&mut elf, ph + 16, s.data.len() as u32); // p_filesz
            put32(&mut elf, ph + 20, s.data.len() as u32); // p_memsz
            put32(&mut elf, ph + 24, 5); // p_flags = R+X
            put32(&mut elf, ph + 28, 4); // p_align
            elf[body..body + s.data.len()].copy_from_slice(&s.data);
            body += s.data.len();
        }
        elf
    }

    fn put16(b: &mut [u8], o: usize, v: u16) {
        b[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }

    /// Build a flat image with valid FSB metadata for `[base, base+len)`.
    fn image_with_fsb(base: u32, len: usize) -> Vec<u8> {
        let mut img = vec![0u8; len];
        put32(&mut img, META_CODE_START, base);
        put32(&mut img, META_CODE_END, base + len as u32);
        put32(&mut img, META_CODE_EXECUTE, base); // entry at start
        img[META_SIGNATURE..META_SIGNATURE + SIGNATURE.len()].copy_from_slice(SIGNATURE);
        img
    }

    // ---- flatten_elf -------------------------------------------------------

    #[test]
    fn flatten_single_segment() {
        let elf = build_elf(&[Seg {
            paddr: 0x0C00_0000,
            data: vec![1, 2, 3, 4],
        }]);
        let (image, base) = flatten_elf(&elf).unwrap();
        assert_eq!(base, 0x0C00_0000);
        assert_eq!(image, vec![1, 2, 3, 4]);
    }

    #[test]
    fn flatten_two_segments_with_gap() {
        // Segments at base and base+8 leave a 4-byte zero-filled gap between.
        let elf = build_elf(&[
            Seg {
                paddr: 0x0C00_0000,
                data: vec![0xAA; 4],
            },
            Seg {
                paddr: 0x0C00_0008,
                data: vec![0xBB; 4],
            },
        ]);
        let (image, base) = flatten_elf(&elf).unwrap();
        assert_eq!(base, 0x0C00_0000);
        assert_eq!(image.len(), 12);
        assert_eq!(&image[0..4], &[0xAA; 4]);
        assert_eq!(&image[4..8], &[0, 0, 0, 0], "gap must be zero-filled");
        assert_eq!(&image[8..12], &[0xBB; 4]);
    }

    #[test]
    fn flatten_orders_by_lma_not_phdr_order() {
        // Higher-address segment listed first; image base is still the lowest.
        let elf = build_elf(&[
            Seg {
                paddr: 0x0C00_0008,
                data: vec![0xBB; 4],
            },
            Seg {
                paddr: 0x0C00_0000,
                data: vec![0xAA; 4],
            },
        ]);
        let (image, base) = flatten_elf(&elf).unwrap();
        assert_eq!(base, 0x0C00_0000);
        assert_eq!(&image[0..4], &[0xAA; 4]);
        assert_eq!(&image[8..12], &[0xBB; 4]);
    }

    #[test]
    fn flatten_rejects_short_file() {
        assert!(flatten_elf(&[0u8; 8]).is_err());
    }

    #[test]
    fn flatten_rejects_bad_magic() {
        let mut elf = build_elf(&[Seg {
            paddr: 0,
            data: vec![0; 4],
        }]);
        elf[1] = b'X';
        assert!(flatten_elf(&elf).unwrap_err().contains("magic"));
    }

    #[test]
    fn flatten_rejects_non_32bit_or_big_endian() {
        let mut elf = build_elf(&[Seg {
            paddr: 0,
            data: vec![0; 4],
        }]);
        elf[4] = 2; // ELFCLASS64
        assert!(flatten_elf(&elf).is_err());
        elf[4] = ELFCLASS32;
        elf[5] = 2; // ELFDATA2MSB
        assert!(flatten_elf(&elf).is_err());
    }

    #[test]
    fn flatten_rejects_non_exec_and_non_arm() {
        let mut elf = build_elf(&[Seg {
            paddr: 0,
            data: vec![0; 4],
        }]);
        put16(&mut elf, 16, 1); // ET_REL
        assert!(flatten_elf(&elf).unwrap_err().contains("ET_EXEC"));
        put16(&mut elf, 16, ET_EXEC);
        put16(&mut elf, 18, 0x3E); // EM_X86_64
        assert!(flatten_elf(&elf).unwrap_err().contains("EM_ARM"));
    }

    #[test]
    fn flatten_skips_bss_only_segment() {
        // A PT_LOAD with filesz=0 (e.g. .bss) contributes nothing to the image.
        let mut elf = build_elf(&[
            Seg {
                paddr: 0x0C00_0000,
                data: vec![0x11; 4],
            },
            Seg {
                paddr: 0x0C00_0010,
                data: vec![],
            },
        ]);
        // The builder set p_memsz=0 too; bump memsz to mimic a real .bss.
        let ph1 = 52 + 32;
        put32(&mut elf, ph1 + 20, 0x100);
        let (image, _) = flatten_elf(&elf).unwrap();
        assert_eq!(image.len(), 4, "bss-only segment must not extend the image");
    }

    #[test]
    fn flatten_rejects_no_loadable_segments() {
        let elf = build_elf(&[]);
        assert!(flatten_elf(&elf).is_err());
    }

    #[test]
    fn flatten_rejects_segment_past_eof() {
        let mut elf = build_elf(&[Seg {
            paddr: 0,
            data: vec![0; 4],
        }]);
        let ph = 52;
        put32(&mut elf, ph + 16, 0x1000); // p_filesz way past EOF
        assert!(flatten_elf(&elf).unwrap_err().contains("past end"));
    }

    // ---- check_fsb_metadata ------------------------------------------------

    #[test]
    fn fsb_accepts_valid_metadata() {
        let img = image_with_fsb(0x0C00_0000, 0x200);
        check_fsb_metadata(&img, 0x0C00_0000, DEFAULT_BASE).unwrap();
    }

    #[test]
    fn fsb_rejects_missing_signature() {
        let mut img = image_with_fsb(0x0C00_0000, 0x200);
        img[META_SIGNATURE] ^= 0xFF;
        assert!(check_fsb_metadata(&img, 0x0C00_0000, DEFAULT_BASE)
            .unwrap_err()
            .contains("signature"));
    }

    #[test]
    fn fsb_rejects_too_small_image() {
        let img = vec![0u8; META_SIGNATURE]; // can't hold the signature
        assert!(check_fsb_metadata(&img, 0, DEFAULT_BASE).is_err());
    }

    #[test]
    fn fsb_rejects_code_start_mismatch() {
        let img = image_with_fsb(0x0C00_0000, 0x200);
        // Image actually loads 0x100 lower than the metadata claims.
        assert!(check_fsb_metadata(&img, 0x0BFF_FF00, DEFAULT_BASE)
            .unwrap_err()
            .contains("code_start"));
    }

    #[test]
    fn fsb_rejects_entry_outside_code_range() {
        let mut img = image_with_fsb(0x0C00_0000, 0x200);
        put32(&mut img, META_CODE_EXECUTE, 0x0C00_0400); // past code_end
        assert!(check_fsb_metadata(&img, 0x0C00_0000, DEFAULT_BASE)
            .unwrap_err()
            .contains("entry"));
    }

    #[test]
    fn fsb_rejects_image_longer_than_coded_span() {
        let mut img = image_with_fsb(0x0C00_0000, 0x200);
        // Shrink code_end so the image extends past it.
        put32(&mut img, META_CODE_END, 0x0C00_0100);
        assert!(check_fsb_metadata(&img, 0x0C00_0000, DEFAULT_BASE)
            .unwrap_err()
            .contains("code_end"));
    }

    // ---- build_uf2 ---------------------------------------------------------

    #[test]
    fn uf2_block_count_and_remainder() {
        // 256 + 1 bytes -> 2 blocks (ceil), last carries one payload byte.
        let image: Vec<u8> = (0..=256u32).map(|i| i as u8).collect();
        let uf2 = build_uf2(&image, DEFAULT_BASE, DEFAULT_FAMILY);
        assert_eq!(uf2.len(), 2 * 512);

        // Block 1 header fields.
        assert_eq!(le32(&uf2, 512 + 0x00), UF2_MAGIC_START0);
        assert_eq!(le32(&uf2, 512 + 0x04), UF2_MAGIC_START1);
        assert_eq!(le32(&uf2, 512 + 0x08), UF2_FLAG_FAMILY_PRESENT);
        assert_eq!(le32(&uf2, 512 + 0x0C), DEFAULT_BASE + 256); // targetAddr
        assert_eq!(le32(&uf2, 512 + 0x10), PAYLOAD); // payloadSize (fixed 256)
        assert_eq!(le32(&uf2, 512 + 0x14), 1); // blockNo
        assert_eq!(le32(&uf2, 512 + 0x18), 2); // numBlocks
        assert_eq!(le32(&uf2, 512 + 0x1C), DEFAULT_FAMILY);
        assert_eq!(le32(&uf2, 512 + 0x1FC), UF2_MAGIC_END);

        // The remainder byte is present; the rest of the payload area is zero.
        assert_eq!(uf2[512 + 0x20], 256u32 as u8);
        assert!(uf2[512 + 0x21..512 + 0x20 + PAYLOAD as usize]
            .iter()
            .all(|&b| b == 0));
    }

    #[test]
    fn uf2_targetaddr_increments_by_payload() {
        let image = vec![0u8; 256 * 3];
        let uf2 = build_uf2(&image, DEFAULT_BASE, DEFAULT_FAMILY);
        assert_eq!(uf2.len(), 3 * 512);
        for blk in 0..3u32 {
            let o = blk as usize * 512;
            assert_eq!(le32(&uf2, o + 0x0C), DEFAULT_BASE + blk * PAYLOAD);
            assert_eq!(le32(&uf2, o + 0x14), blk);
            assert_eq!(le32(&uf2, o + 0x18), 3);
        }
    }

    // ---- constant sync (mirrors the firmware side) -------------------------

    #[test]
    fn defaults_match_firmware_constants() {
        // These must stay in lockstep with rza1l-hal/src/spibsc.rs and
        // app-loader/src/uf2.rs (asserted again from the firmware side).
        assert_eq!(DEFAULT_BASE, 0x1810_0000);
        assert_eq!(SLOT_LEN, 0x0030_0000);
        assert_eq!(DEFAULT_FAMILY, 0x6E27_5A1C);
        assert_eq!(PAYLOAD, 256);
        assert_eq!(SIGNATURE, b".BootLoad_ValidProgramTest.");
    }

    // ---- arg parsing -------------------------------------------------------

    fn args(v: &[&str]) -> Result<Args, String> {
        parse_args(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn args_defaults() {
        let a = args(&["fw.elf"]).unwrap();
        assert_eq!(a.input, PathBuf::from("fw.elf"));
        assert_eq!(a.output, PathBuf::from("fw.uf2"));
        assert_eq!(a.base, DEFAULT_BASE);
        assert_eq!(a.family, DEFAULT_FAMILY);
        assert!(!a.emit_bin && !a.skip_fsb_check);
    }

    #[test]
    fn args_all_options() {
        let a = args(&[
            "in.elf", "-o", "out.uf2", "--base", "0x1000", "--family", "42", "--bin",
            "--skip-fsb-check",
        ])
        .unwrap();
        assert_eq!(a.output, PathBuf::from("out.uf2"));
        assert_eq!(a.base, 0x1000);
        assert_eq!(a.family, 42);
        assert!(a.emit_bin && a.skip_fsb_check);
    }

    #[test]
    fn args_errors() {
        assert!(args(&[]).is_err(), "missing input");
        assert!(args(&["a.elf", "b.elf"]).is_err(), "two inputs");
        assert!(args(&["--frob"]).is_err(), "unknown option");
        assert!(args(&["a.elf", "-o"]).is_err(), "dangling -o");
    }

    #[test]
    fn parse_u32_forms() {
        assert_eq!(parse_u32("0x1810_0000".replace('_', "").as_str()).unwrap(), 0x1810_0000);
        assert_eq!(parse_u32("0X10").unwrap(), 16);
        assert_eq!(parse_u32("4096").unwrap(), 4096);
        assert!(parse_u32("nope").is_err());
    }

    #[test]
    fn default_output_swaps_extension() {
        assert_eq!(default_output(Path::new("a/b/fw.elf")), PathBuf::from("a/b/fw.uf2"));
        assert_eq!(default_output(Path::new("fw")), PathBuf::from("fw.uf2"));
    }

    // ---- end-to-end (in-memory) -------------------------------------------

    #[test]
    fn elf_to_uf2_pipeline() {
        // A RAM-linked-style ELF with FSB metadata flattens, validates, wraps.
        let base = 0x0C00_0000;
        let image = image_with_fsb(base, 0x200);
        let elf = build_elf(&[Seg {
            paddr: base,
            data: image.clone(),
        }]);

        let (flat, flat_base) = flatten_elf(&elf).unwrap();
        assert_eq!(flat, image);
        assert_eq!(flat_base, base);
        check_fsb_metadata(&flat, flat_base, DEFAULT_BASE).unwrap();

        let uf2 = build_uf2(&flat, DEFAULT_BASE, DEFAULT_FAMILY);
        assert_eq!(uf2.len(), 2 * 512); // 0x200 bytes = 2 blocks
    }
}
