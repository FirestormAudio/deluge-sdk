//! `cargo deluge uf2`: pack an app or a loader into a `.uf2` for the boot menu's `USB UPDATE`.
//!
//! An app goes into the flash app slot as a DAPP image ([`deluge_image::app`]): from an ELF (its
//! SRAM `PT_LOAD`s flattened, as the loader's store-to-flash does) or from a flat `.bin` with the
//! first-stage boot metadata (Community and stock firmware). A loader is a boot-slot image
//! (`LOADER.SLT`) taken as-is. Either way the file is UF2 blocks under the unit's family ID.

use std::path::Path;

use crate::frame::crc32;

// The formats below are the device's (`deluge_image::{uf2, app, slot, elf}`), written out here:
// this tool is published and `deluge-image` isn't, so it can't be a dependency. The tests pin
// every one of them against `deluge-image` (a dev-dependency), so the two can't drift.

/// The Deluge's UF2 family IDs (`deluge_image::uf2::{DELUGE_OLED, DELUGE_SEVEN_SEG}`).
const OLED_APP: u32 = 0xA799_ECAC;
const OLED_LOADER: u32 = 0x51B0_2BFD;
const SEVEN_SEG_APP: u32 = 0xC6E8_31B1;
const SEVEN_SEG_LOADER: u32 = 0x9851_DF9D;
/// UF2 (`deluge_image::uf2`).
const UF2_BLOCK: usize = 512;
const UF2_PAYLOAD: usize = 256;
const UF2_MAGIC0: u32 = 0x0A32_4655;
const UF2_MAGIC1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY: u32 = 0x2000;
/// The DAPP app header (`deluge_image::app`).
const APP_MAGIC: u32 = 0x5050_4144;
const APP_VERSION: u16 = 1;
const APP_HEADER_LEN: usize = 0x40;
/// The boot-slot header (`deluge_image::slot`, the Deluge's layout).
const SLOT_MAGIC: u32 = 0x544C_5344;
const SLOT_VERSION: u16 = 1;
const SLOT_HEADER_LEN: usize = 0x100;
const SLOT_LEN: usize = 0x7_0000;
/// The first-stage boot metadata (`deluge_image::elf`).
const FSB_CODE_START: usize = 0x20;
const FSB_CODE_END: usize = 0x24;
const FSB_CODE_EXECUTE: usize = 0x28;
const FSB_SIGNATURE_OFF: usize = 0x2C;
const FSB_SIGNATURE: &[u8] = b".BootLoad_ValidProgramTest.";
/// The SRAM apps load into (`deluge_image::elf::{SRAM_LOAD_ORIGIN, SRAM_HI}`).
const SRAM: std::ops::Range<u32> = 0x2002_0000..0x2030_0000;
/// The Deluge's flash app slot (`deluge_bsp::flash::SLOT_LEN`): header and body.
const APP_SLOT_LEN: usize = 0x2C_0000;

/// Which front panel the file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    Oled,
    SevenSeg,
}

impl Variant {
    /// The variant code (`deluge_image::slot::{VARIANT_OLED, VARIANT_SEVEN_SEG}`).
    fn code(self) -> u16 {
        match self {
            Variant::Oled => 0,
            Variant::SevenSeg => 1,
        }
    }

    /// (app, loader) family IDs.
    fn families(self) -> (u32, u32) {
        match self {
            Variant::Oled => (OLED_APP, OLED_LOADER),
            Variant::SevenSeg => (SEVEN_SEG_APP, SEVEN_SEG_LOADER),
        }
    }
}

fn le16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// `cargo deluge uf2`'s arguments.
#[derive(Debug)]
pub(crate) struct Opts {
    pub input: String,
    pub loader: bool,
    pub variant: Variant,
    pub out: String,
}

/// Parse `<input> [--loader] [--variant oled|7seg] [-o <out.uf2>]`.
pub(crate) fn parse(args: &[String]) -> Result<Opts, String> {
    let usage = "usage: cargo deluge uf2 <input> [--loader] [--variant oled|7seg] [-o <out.uf2>]";
    let (mut input, mut loader, mut variant, mut out) = (None, false, Variant::Oled, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--loader" => loader = true,
            "--variant" => {
                variant = match it.next().map(String::as_str) {
                    Some("oled") => Variant::Oled,
                    Some("7seg") => Variant::SevenSeg,
                    other => {
                        return Err(format!(
                            "--variant takes oled or 7seg, not {other:?}\n{usage}"
                        ));
                    }
                }
            }
            "-o" | "--out" => {
                out = Some(
                    it.next()
                        .ok_or(format!("-o needs a path\n{usage}"))?
                        .clone(),
                )
            }
            f if f.starts_with('-') => return Err(format!("unknown option {f}\n{usage}")),
            path if input.is_none() => input = Some(path.to_string()),
            extra => return Err(format!("unexpected argument {extra}\n{usage}")),
        }
    }
    let input = input.ok_or(format!("no input file\n{usage}"))?;
    let out = out.unwrap_or_else(|| {
        Path::new(&input)
            .with_extension("uf2")
            .to_string_lossy()
            .into_owned()
    });
    Ok(Opts {
        input,
        loader,
        variant,
        out,
    })
}

pub(crate) fn cmd_uf2(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let input = std::fs::read(&o.input).map_err(|e| format!("reading {}: {e}", o.input))?;
    let file = pack(&input, o.loader, o.variant)?;
    std::fs::write(&o.out, &file).map_err(|e| format!("writing {}: {e}", o.out))?;
    println!(
        "{}: {} blocks ({} for USB UPDATE)",
        o.out,
        file.len() / UF2_BLOCK,
        if o.loader { "a loader" } else { "an app" }
    );
    Ok(())
}

/// `input` as a `.uf2`: an app (ELF or FSB `.bin`) or, with `loader`, a boot-slot image.
pub(crate) fn pack(input: &[u8], loader: bool, variant: Variant) -> Result<Vec<u8>, String> {
    let (app_family, loader_family) = variant.families();
    let (image, family) = if loader {
        check_slot_image(input, variant.code())?;
        (input.to_vec(), loader_family)
    } else {
        (app_image(input, variant.code())?, app_family)
    };
    Ok(uf2_encode(&image, family))
}

/// `image` as UF2 blocks of 256-byte payloads under `family`, `targetAddr` counting from 0.
fn uf2_encode(image: &[u8], family: u32) -> Vec<u8> {
    let num_blocks = image.len().div_ceil(UF2_PAYLOAD) as u32;
    let mut out = Vec::with_capacity(num_blocks as usize * UF2_BLOCK);
    for (i, chunk) in image.chunks(UF2_PAYLOAD).enumerate() {
        let mut b = [0u8; UF2_BLOCK];
        let mut put = |off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(0, UF2_MAGIC0);
        put(4, UF2_MAGIC1);
        put(8, UF2_FLAG_FAMILY);
        put(12, (i * UF2_PAYLOAD) as u32);
        put(16, chunk.len() as u32);
        put(20, i as u32);
        put(24, num_blocks);
        put(28, family);
        put(UF2_BLOCK - 4, UF2_MAGIC_END);
        b[32..32 + chunk.len()].copy_from_slice(chunk);
        out.extend_from_slice(&b);
    }
    out
}

/// A DAPP header for `body`: where it loads, its entry, its length and CRC, under `variant`.
fn dapp_header(body: &[u8], variant: u16, load: u32, entry: u32) -> [u8; APP_HEADER_LEN] {
    let mut h = [0xFFu8; APP_HEADER_LEN];
    h[0..4].copy_from_slice(&APP_MAGIC.to_le_bytes());
    h[4..6].copy_from_slice(&APP_VERSION.to_le_bytes());
    h[6..8].copy_from_slice(&variant.to_le_bytes());
    h[8..12].copy_from_slice(&load.to_le_bytes());
    h[12..16].copy_from_slice(&entry.to_le_bytes());
    h[16..20].copy_from_slice(&(body.len() as u32).to_le_bytes());
    h[20..24].copy_from_slice(&crc32(body).to_le_bytes());
    let hcrc = crc32(&h[..0x3C]);
    h[0x3C..0x40].copy_from_slice(&hcrc.to_le_bytes());
    h
}

/// A loader slot image's checks, as stage-0 makes them: magic, version, variant, length, both
/// CRCs. (The device checks it again before writing.)
fn check_slot_image(img: &[u8], variant: u16) -> Result<(), String> {
    let bad = |why: &str| Err(format!("not a loader slot image for this variant: {why}"));
    if img.len() < SLOT_HEADER_LEN || le32(img, 0) != SLOT_MAGIC {
        return bad("no slot header");
    }
    if le16(img, 4) != SLOT_VERSION {
        return bad("a slot header version this tool doesn't know");
    }
    if le16(img, 6) != variant {
        return bad("another variant's");
    }
    if le32(img, 0xF8) != crc32(&img[..0xF8]) {
        return bad("the header CRC doesn't match");
    }
    let len = le32(img, 0x0C) as usize;
    if len == 0 || len > SLOT_LEN - SLOT_HEADER_LEN || img.len() < SLOT_HEADER_LEN + len {
        return bad("the body length is wrong");
    }
    if le32(img, 0x10) != crc32(&img[SLOT_HEADER_LEN..SLOT_HEADER_LEN + len]) {
        return bad("the body CRC doesn't match");
    }
    Ok(())
}

/// An app's DAPP image from an ELF or a flat `.bin` with the first-stage boot metadata.
fn app_image(input: &[u8], variant: u16) -> Result<Vec<u8>, String> {
    let flat;
    let bin = if input.starts_with(b"\x7FELF") {
        flat = flatten_sram(input)?;
        // Drop a leading NOLOAD region the linker placed below the vector table.
        &flat[fsb_base(&flat).unwrap_or(0)..]
    } else {
        input
    };
    let (load, entry) = fsb_metadata(bin)?;
    if APP_HEADER_LEN + bin.len() > APP_SLOT_LEN {
        return Err(format!(
            "{} bytes is too large for the {} byte app slot",
            bin.len(),
            APP_SLOT_LEN - APP_HEADER_LEN
        ));
    }
    if load < SRAM.start || u64::from(load) + bin.len() as u64 > u64::from(SRAM.end) {
        return Err(format!("it loads at {load:#x}, outside SRAM"));
    }
    let mut image = dapp_header(bin, variant, load, entry).to_vec();
    image.extend_from_slice(bin);
    Ok(image)
}

/// `(load address, entry)` from a flat image's first-stage boot metadata, checked as the device
/// checks it (`deluge_image::elf::validate_fsb_metadata`).
fn fsb_metadata(bin: &[u8]) -> Result<(u32, u32), String> {
    let bad = |why: &str| {
        Err(format!(
            "not a bootable SRAM image (first-stage boot metadata): {why}"
        ))
    };
    if bin.len() < FSB_SIGNATURE_OFF + FSB_SIGNATURE.len() {
        return bad("too short");
    }
    if &bin[FSB_SIGNATURE_OFF..FSB_SIGNATURE_OFF + FSB_SIGNATURE.len()] != FSB_SIGNATURE {
        return bad("no boot signature");
    }
    let (start, end, entry) = (
        le32(bin, FSB_CODE_START),
        le32(bin, FSB_CODE_END),
        le32(bin, FSB_CODE_EXECUTE),
    );
    if end <= start {
        return bad("the code end is below its start");
    }
    if entry < start || entry >= end {
        return bad("the entry is outside the code");
    }
    if bin.len() > (end - start) as usize {
        return bad("the image is longer than its metadata says");
    }
    Ok((start, entry))
}

/// Where the vector table starts in a flattened image: the boot signature's offset less its
/// place in the metadata (`deluge_image::elf::find_fsb_base`).
fn fsb_base(img: &[u8]) -> Option<usize> {
    img.windows(FSB_SIGNATURE.len())
        .enumerate()
        .skip(FSB_SIGNATURE_OFF)
        .find(|(_, w)| *w == FSB_SIGNATURE)
        .map(|(i, _)| i - FSB_SIGNATURE_OFF)
}

/// An ELF32 LE ARM executable's `PT_LOAD` file images laid out by physical address from the
/// lowest; all must be in SRAM. Gaps are zeros; `.bss` is left to the image's start-up code.
fn flatten_sram(elf: &[u8]) -> Result<Vec<u8>, String> {
    let ok = elf.len() >= 52
        && elf[4] == 1 // 32-bit
        && elf[5] == 1 // little-endian
        && le16(elf, 16) == 2 // ET_EXEC
        && le16(elf, 18) == 0x28; // EM_ARM
    if !ok {
        return Err("not a 32-bit little-endian ARM executable ELF".into());
    }
    let (phoff, phentsize, phnum) = (
        le32(elf, 28) as usize,
        le16(elf, 42) as usize,
        le16(elf, 44) as usize,
    );
    let mut segs = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let field = |o: usize| {
            elf.get(ph + o..ph + o + 4)
                .map(|_| le32(elf, ph + o))
                .ok_or("program header past the end of the file")
        };
        let (kind, offset, paddr, filesz) = (field(0)?, field(4)?, field(12)?, field(16)?);
        if kind != 1 || filesz == 0 {
            continue; // not PT_LOAD, or memory only
        }
        if paddr < SRAM.start || u64::from(paddr) + u64::from(filesz) > u64::from(SRAM.end) {
            return Err(format!(
                "a segment at {paddr:#x} is outside SRAM: only SRAM-linked apps go to the app slot"
            ));
        }
        let bytes = elf
            .get(offset as usize..offset as usize + filesz as usize)
            .ok_or("a segment's file image runs past the end of the file")?;
        segs.push((paddr, bytes));
    }
    segs.sort_by_key(|&(p, _)| p);
    let load = segs.first().ok_or("no PT_LOAD segment with file data")?.0;
    let mut body = Vec::new();
    for (paddr, bytes) in segs {
        let at = (paddr - load) as usize;
        if at < body.len() {
            return Err(format!("segment at {paddr:#x} overlaps the one before it"));
        }
        body.resize(at, 0);
        body.extend_from_slice(bytes);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deluge_image::app::validate_app;
    use deluge_image::slot::{DELUGE, VARIANT_OLED, VARIANT_SEVEN_SEG, encode_header_with};
    use deluge_image::uf2::{BLOCK_LEN, DELUGE_OLED, DELUGE_SEVEN_SEG, decode};

    const BASE: u32 = 0x2002_0000;

    /// A flat SRAM image with FSB metadata at `+0x20`, `len` bytes long, entry at `+0x100`.
    fn fsb_bin(len: usize) -> Vec<u8> {
        let mut b: Vec<u8> = (0..len).map(|i| (i * 5 + 1) as u8).collect();
        b[0x20..0x24].copy_from_slice(&BASE.to_le_bytes());
        // The linker rounds the end up to 64 KB past the image.
        let end = (BASE + len as u32 + 0xFFFF) & !0xFFFF;
        b[0x24..0x28].copy_from_slice(&end.to_le_bytes());
        b[0x28..0x2C].copy_from_slice(&(BASE + 0x100).to_le_bytes());
        let sig = b".BootLoad_ValidProgramTest.\0";
        b[0x2C..0x2C + sig.len()].copy_from_slice(sig);
        b
    }

    /// An ELF32 LE ARM executable with `segs` as `(paddr, bytes)` `PT_LOAD`s.
    fn elf(entry: u32, segs: &[(u32, &[u8])]) -> Vec<u8> {
        let phoff = 52u32;
        let mut e = vec![0u8; 52];
        e[..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        e[4] = 1;
        e[5] = 1;
        e[6] = 1;
        e[16..18].copy_from_slice(&2u16.to_le_bytes());
        e[18..20].copy_from_slice(&0x28u16.to_le_bytes());
        e[20..24].copy_from_slice(&1u32.to_le_bytes());
        e[24..28].copy_from_slice(&entry.to_le_bytes());
        e[28..32].copy_from_slice(&phoff.to_le_bytes());
        e[40..42].copy_from_slice(&52u16.to_le_bytes());
        e[42..44].copy_from_slice(&32u16.to_le_bytes());
        e[44..46].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        let mut off = phoff + 32 * segs.len() as u32;
        for &(paddr, bytes) in segs {
            let mut ph = [0u8; 32];
            ph[0..4].copy_from_slice(&1u32.to_le_bytes());
            ph[4..8].copy_from_slice(&off.to_le_bytes());
            ph[8..12].copy_from_slice(&paddr.to_le_bytes());
            ph[12..16].copy_from_slice(&paddr.to_le_bytes());
            ph[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            ph[20..24].copy_from_slice(&(bytes.len() as u32 + 16).to_le_bytes());
            e.extend_from_slice(&ph);
            off += bytes.len() as u32;
        }
        for &(_, bytes) in segs {
            e.extend_from_slice(bytes);
        }
        e
    }

    /// The payloads of a `.uf2` in order, and every block's family.
    fn unpack(file: &[u8]) -> (Vec<u8>, Vec<u32>) {
        let mut payload = Vec::new();
        let mut families = Vec::new();
        for chunk in file.chunks(BLOCK_LEN) {
            let b = decode(chunk.try_into().unwrap()).expect("a UF2 block");
            assert_eq!(b.target as usize, payload.len());
            payload.extend_from_slice(b.payload);
            families.push(b.family);
        }
        (payload, families)
    }

    #[test]
    fn bin_becomes_dapp_uf2() {
        let bin = fsb_bin(3000);
        let (image, families) = unpack(&pack(&bin, false, Variant::Oled).unwrap());
        let h = validate_app(&image, &[VARIANT_OLED], u32::MAX).unwrap();
        assert_eq!(h.load_addr, BASE);
        assert_eq!(h.entry, BASE + 0x100);
        assert_eq!(&image[64..], &bin[..]);
        assert!(families.iter().all(|&f| f == DELUGE_OLED.app));
    }

    #[test]
    fn elf_becomes_the_same_dapp_as_its_bin() {
        let bin = fsb_bin(3000);
        let (a, b) = bin.split_at(1000);
        // Two SRAM segments with a gap of zeros between them, as a linker can lay them out.
        let mut flat = a.to_vec();
        flat.resize(1024, 0);
        flat.extend_from_slice(b);
        let f = elf(BASE + 0x100, &[(BASE, a), (BASE + 1024, b)]);
        let (image, _) = unpack(&pack(&f, false, Variant::Oled).unwrap());
        assert_eq!(&image[64..], &flat[..]);
    }

    #[test]
    fn elf_with_a_segment_outside_sram_is_refused() {
        let bin = fsb_bin(1000);
        let f = elf(BASE + 0x100, &[(BASE, &bin), (0x0C00_0000, &[1, 2, 3, 4])]);
        let e = pack(&f, false, Variant::Oled).unwrap_err();
        assert!(e.contains("SRAM"), "{e}");
    }

    #[test]
    fn seven_seg_uses_its_family() {
        let (image, families) = unpack(&pack(&fsb_bin(1000), false, Variant::SevenSeg).unwrap());
        assert!(validate_app(&image, &[VARIANT_SEVEN_SEG], u32::MAX).is_ok());
        assert!(families.iter().all(|&f| f == DELUGE_SEVEN_SEG.app));
    }

    #[test]
    fn loader_slot_image_passes_through() {
        let body = vec![0x11u8; 5000];
        let mut slt = encode_header_with(&body, VARIANT_OLED, 1, 0x100, DELUGE)
            .unwrap()
            .to_vec();
        slt.extend_from_slice(&body);
        let (image, families) = unpack(&pack(&slt, true, Variant::Oled).unwrap());
        assert_eq!(image, slt);
        assert!(families.iter().all(|&f| f == DELUGE_OLED.loader));
    }

    #[test]
    fn loader_of_the_wrong_variant_is_refused() {
        let body = vec![0x11u8; 500];
        let mut slt = encode_header_with(&body, VARIANT_OLED, 1, 0x100, DELUGE)
            .unwrap()
            .to_vec();
        slt.extend_from_slice(&body);
        assert!(pack(&slt, true, Variant::SevenSeg).is_err());
    }

    #[test]
    fn garbage_is_refused() {
        assert!(pack(b"not a firmware image at all", false, Variant::Oled).is_err());
        assert!(pack(b"not a firmware image at all", true, Variant::Oled).is_err());
    }

    #[test]
    fn too_large_for_the_slot_is_refused() {
        let bin = fsb_bin(0x2C_0000);
        let e = pack(&bin, false, Variant::Oled).unwrap_err();
        assert!(e.contains("slot"), "{e}");
    }

    #[test]
    fn the_formats_are_the_device_s() {
        use deluge_image::app::encode_app_header;
        use deluge_image::elf::{SRAM_HI, SRAM_LOAD_ORIGIN, find_fsb_base};
        // Family IDs and variant codes.
        assert_eq!(
            Variant::Oled.families(),
            (DELUGE_OLED.app, DELUGE_OLED.loader)
        );
        assert_eq!(
            Variant::SevenSeg.families(),
            (DELUGE_SEVEN_SEG.app, DELUGE_SEVEN_SEG.loader)
        );
        assert_eq!(Variant::Oled.code(), VARIANT_OLED);
        assert_eq!(Variant::SevenSeg.code(), VARIANT_SEVEN_SEG);
        assert_eq!(SRAM, SRAM_LOAD_ORIGIN..SRAM_HI);
        assert_eq!(SLOT_LEN as u32, DELUGE.slot_len);
        // The DAPP header and the UF2 blocks, byte for byte.
        let body: Vec<u8> = (0..1234).map(|i| (i * 7) as u8).collect();
        assert_eq!(
            dapp_header(&body, 1, BASE, BASE + 4),
            encode_app_header(&body, 1, BASE, BASE + 4)
        );
        let device: Vec<u8> = deluge_image::uf2::encode(&body, OLED_APP)
            .flatten()
            .collect();
        assert_eq!(uf2_encode(&body, OLED_APP), device);
        // Finding the vector table.
        let mut flat = vec![0u8; 0x200];
        flat.extend_from_slice(&fsb_bin(1000));
        assert_eq!(fsb_base(&flat), find_fsb_base(&flat));
        assert_eq!(fsb_base(&fsb_bin(1000)), find_fsb_base(&fsb_bin(1000)));
    }

    #[test]
    fn slot_checks_agree_with_the_device_s() {
        use deluge_image::slot::validate_with;
        let body = vec![0x22u8; 3000];
        let mut good = encode_header_with(&body, VARIANT_OLED, 1, 0x100, DELUGE)
            .unwrap()
            .to_vec();
        good.extend_from_slice(&body);
        let mut bad_body = good.clone();
        bad_body[SLOT_HEADER_LEN + 5] ^= 1;
        let mut bad_header = good.clone();
        bad_header[0x08] ^= 1;
        for img in [&good, &bad_body, &bad_header, &good[..2000].to_vec()] {
            for v in [VARIANT_OLED, VARIANT_SEVEN_SEG] {
                assert_eq!(
                    check_slot_image(img, v).is_ok(),
                    validate_with(img, DELUGE, &[v]).is_ok(),
                    "variant {v}"
                );
            }
        }
    }

    #[test]
    fn args_parse() {
        let a: Vec<String> = ["fw.bin", "--variant", "7seg", "-o", "x.uf2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let o = parse(&a).unwrap();
        assert_eq!(o.input, "fw.bin");
        assert_eq!(o.variant, Variant::SevenSeg);
        assert!(!o.loader);
        assert_eq!(o.out, "x.uf2");
        let d = parse(&["dir/fw.elf".to_string()]).unwrap();
        assert_eq!(d.variant, Variant::Oled);
        assert_eq!(d.out, "dir/fw.uf2");
        assert!(parse(&["fw.bin".into(), "--variant".into(), "lcd".into()]).is_err());
        assert!(parse(&[]).is_err());
    }
}
