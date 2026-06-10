//! `elf2uf2` — convert a RAM-linked ARM firmware ELF into a Deluge-SSB `.uf2`.
//!
//! This is the single-step replacement for the old `objcopy -O binary` +
//! `uf2conv.py` pipeline (see `tools/uf2/README.md`).  It takes the *same* ELF
//! that the second-stage bootloader's SD-card path consumes
//! (`second-stage-bootloader/src/elf.rs`) and emits a `.uf2` that the USB
//! update path flashes into the app slot (`second-stage-bootloader/src/uf2.rs`).
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
//! `second-stage-bootloader/src/uf2.rs`.

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
    let args = parse_args()?;

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

fn parse_args() -> Result<Args, String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut base = DEFAULT_BASE;
    let mut family = DEFAULT_FAMILY;
    let mut emit_bin = false;
    let mut skip_fsb_check = false;

    let mut it = std::env::args().skip(1);
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
