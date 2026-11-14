//! `cargo deluge uf2`: pack an app or a loader into a `.uf2` for the boot menu's `USB UPDATE`.
//!
//! An app goes into the flash app slot as a DAPP image ([`deluge_image::app`]): from an ELF (its
//! SRAM `PT_LOAD`s flattened, as the loader's store-to-flash does) or from a flat `.bin` with the
//! first-stage boot metadata (Community and stock firmware). A loader is a boot-slot image
//! (`LOADER.SLT`) taken as-is. Either way the file is UF2 blocks under the unit's family ID.

use std::path::Path;

use deluge_image::app::{APP_HEADER_LEN, encode_app_header, validate_app};
use deluge_image::elf::{
    PT_LOAD, SRAM_HI, SRAM_LOAD_ORIGIN, find_fsb_base, le16, le32, validate_fsb_metadata,
    validate_header,
};
use deluge_image::slot::{self, VARIANT_OLED, VARIANT_SEVEN_SEG};
use deluge_image::uf2::{self, DELUGE_OLED, DELUGE_SEVEN_SEG, Families};

/// The Deluge's flash app slot (`deluge_bsp::flash::SLOT_LEN`): header and body.
const APP_SLOT_LEN: usize = 0x2C_0000;

/// Which front panel the file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    Oled,
    SevenSeg,
}

impl Variant {
    fn code(self) -> u16 {
        match self {
            Variant::Oled => VARIANT_OLED,
            Variant::SevenSeg => VARIANT_SEVEN_SEG,
        }
    }

    fn families(self) -> Families {
        match self {
            Variant::Oled => DELUGE_OLED,
            Variant::SevenSeg => DELUGE_SEVEN_SEG,
        }
    }
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
        file.len() / uf2::BLOCK_LEN,
        if o.loader { "a loader" } else { "an app" }
    );
    Ok(())
}

/// `input` as a `.uf2`: an app (ELF or FSB `.bin`) or, with `loader`, a boot-slot image.
pub(crate) fn pack(input: &[u8], loader: bool, variant: Variant) -> Result<Vec<u8>, String> {
    let fam = variant.families();
    let (image, family) = if loader {
        slot::validate_with(input, slot::DELUGE, &[variant.code()])
            .map_err(|e| format!("not a loader slot image for this variant: {e:?}"))?;
        (input.to_vec(), fam.loader)
    } else {
        (app_image(input, variant.code())?, fam.app)
    };
    Ok(uf2::encode(&image, family).flatten().collect())
}

/// An app's DAPP image from an ELF or a flat `.bin` with the first-stage boot metadata.
fn app_image(input: &[u8], variant: u16) -> Result<Vec<u8>, String> {
    let flat;
    let bin = if input.starts_with(&deluge_image::elf::ELF_MAGIC) {
        flat = flatten_sram(input)?;
        // Drop a leading NOLOAD region the linker placed below the vector table.
        &flat[find_fsb_base(&flat).unwrap_or(0)..]
    } else {
        input
    };
    if bin.len() < 0x30 {
        return Err("too short to be a firmware image".into());
    }
    let base = le32(bin, 0x20);
    let meta = validate_fsb_metadata(bin, base)
        .map_err(|e| format!("not a bootable SRAM image (first-stage boot metadata): {e:?}"))?;
    if APP_HEADER_LEN + bin.len() > APP_SLOT_LEN {
        return Err(format!(
            "{} bytes is too large for the {} byte app slot",
            bin.len(),
            APP_SLOT_LEN - APP_HEADER_LEN
        ));
    }
    let mut image = encode_app_header(bin, variant, meta.code_start, meta.entry).to_vec();
    image.extend_from_slice(bin);
    // What the loader will check, so a bad image fails here, not on the device.
    validate_app(&image, &[variant], (APP_SLOT_LEN - APP_HEADER_LEN) as u32)
        .map_err(|e| format!("the image doesn't validate: {e:?}"))?;
    Ok(image)
}

/// An ELF's `PT_LOAD` file images laid out by physical address from the lowest; all must be in
/// SRAM. Gaps are zeros; `.bss` is left to the image's start-up code. (`mkslot app`'s flattening,
/// with the app slot's SRAM rule.)
fn flatten_sram(elf: &[u8]) -> Result<Vec<u8>, String> {
    validate_header(elf).map_err(|e| format!("not a loadable ELF: {e:?}"))?;
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
        if kind != PT_LOAD || filesz == 0 {
            continue;
        }
        if paddr < SRAM_LOAD_ORIGIN || u64::from(paddr) + u64::from(filesz) > u64::from(SRAM_HI) {
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
