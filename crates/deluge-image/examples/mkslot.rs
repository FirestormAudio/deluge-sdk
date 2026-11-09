//! Build boot-slot images for stage-0, on the host.
//!
//! ```text
//! mkslot wrap --body <bin> --entry-offset <n> --sequence <n> (--variant oled|7seg | --variant-code <n>)
//!             [--slot-len <n>] --out <img>
//! mkslot compose --stage0 <bin> [--slot-a <img>] [--slot-b <img>] [--corrupt-body] [--corrupt-body-b]
//!                [--slot-a-offset <n>] [--slot-b-offset <n>] [--slot-len <n>] [--variant-code <n>] --out <bin>
//! mkslot app --elf <elf> --variant-code <n> --out <img>
//! ```
//!
//! `wrap` puts a slot header page ([`deluge_image::slot`]) in front of a raw XIP body.
//! `compose` builds an image for flash offset 0: stage-0, padded with `0xFF` to slot A, then
//! slot A's image, padded to slot B, then slot B's; `--corrupt-body` (`--corrupt-body-b`) flips
//! the last body byte of slot A (B), for a slot stage-0 must refuse. `app` flattens an ELF's
//! `PT_LOAD` file images by physical address (gaps zero-filled) under a
//! [`deluge_image::app`] header that loads it at the lowest one and enters at `e_entry`.
//!
//! The slot geometry and variant codes default to the Deluge's ([`slot::DELUGE`], OLED/7SEG);
//! `--slot-a-offset`, `--slot-b-offset`, `--slot-len` and `--variant-code` override them (and
//! `compose` then requires the code of both slots).
//!
//! Run with `cargo run --target x86_64-unknown-linux-gnu -p deluge-image --example mkslot -- …`.
//! Numbers take a `0x` prefix for hex.

use std::collections::HashMap;
use std::process::ExitCode;

use deluge_image::slot::{self, HEADER_LEN, SlotLayout, Variant};

const USAGE: &str = "usage:
  mkslot wrap --body <bin> --entry-offset <n> --sequence <n> (--variant oled|7seg | --variant-code <n>)
              [--slot-len <n>] --out <img>
  mkslot compose --stage0 <bin> [--slot-a <img>] [--slot-b <img>] [--corrupt-body] [--corrupt-body-b]
                 [--slot-a-offset <n>] [--slot-b-offset <n>] [--slot-len <n>] [--variant-code <n>] --out <bin>
  mkslot app --elf <elf> --variant-code <n> --out <img>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mkslot: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    let known: &[&str] = match cmd.as_str() {
        "wrap" => &[
            "body",
            "entry-offset",
            "sequence",
            "variant",
            "variant-code",
            "slot-len",
            "out",
        ],
        "compose" => &[
            "stage0",
            "slot-a",
            "slot-b",
            "slot-a-offset",
            "slot-b-offset",
            "slot-len",
            "variant-code",
            "out",
        ],
        "app" => &["elf", "variant-code", "out"],
        _ => return Err(USAGE.into()),
    };
    let (opts, flags) = parse_opts(rest, known)?;
    match cmd.as_str() {
        "wrap" => wrap(&opts, &flags),
        "app" => app(&opts, &flags),
        _ => compose(&opts, &flags),
    }
}

/// Split `--key value` options from bare `--flag`s (`--corrupt-body`, `--corrupt-body-b`), refusing
/// a key not in `known` or given twice: a typo must not quietly drop an option.
fn parse_opts(
    args: &[String],
    known: &[&str],
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let mut opts = HashMap::new();
    let mut flags = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let key = a
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected `{a}`\n{USAGE}"))?;
        if key == "corrupt-body" || key == "corrupt-body-b" {
            flags.push(key.to_string());
        } else {
            if !known.contains(&key) {
                return Err(format!("unknown option `{a}`\n{USAGE}"));
            }
            let v = it.next().ok_or_else(|| format!("`{a}` needs a value"))?;
            if opts.insert(key.to_string(), v.clone()).is_some() {
                return Err(format!("`{a}` given twice"));
            }
        }
    }
    Ok((opts, flags))
}

fn req<'a>(opts: &'a HashMap<String, String>, key: &str) -> Result<&'a str, String> {
    opts.get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("missing --{key}\n{USAGE}"))
}

fn num(s: &str) -> Result<u32, String> {
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => s.parse(),
    };
    r.map_err(|_| format!("`{s}` is not a number"))
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

fn write(path: &str, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("{path}: {e}"))
}

/// `key`'s value as a number, or `default` when it isn't given.
fn opt_num(opts: &HashMap<String, String>, key: &str, default: u32) -> Result<u32, String> {
    opts.get(key).map_or(Ok(default), |v| num(v))
}

/// The slot geometry from the options, the Deluge's by default. `placed`: the slots are laid out
/// in flash (`compose`), so slot A must end before slot B; `wrap` only needs the length.
fn layout(opts: &HashMap<String, String>, placed: bool) -> Result<SlotLayout, String> {
    let d = slot::DELUGE;
    let l = SlotLayout {
        slot_offsets: [
            opt_num(opts, "slot-a-offset", d.slot_offsets[0])?,
            opt_num(opts, "slot-b-offset", d.slot_offsets[1])?,
        ],
        slot_len: opt_num(opts, "slot-len", d.slot_len)?,
    };
    if (l.slot_len as usize) <= HEADER_LEN {
        return Err(format!(
            "--slot-len {} leaves no room for a body",
            l.slot_len
        ));
    }
    if placed
        && l.slot_offsets[0]
            .checked_add(l.slot_len)
            .is_none_or(|end| end > l.slot_offsets[1])
    {
        return Err("slot A must end at or before slot B".into());
    }
    Ok(l)
}

/// The variant code from `--variant-code` (a `u16`), or `None`.
fn variant_code(opts: &HashMap<String, String>) -> Result<Option<u16>, String> {
    opts.get("variant-code")
        .map(|v| u16::try_from(num(v)?).map_err(|_| format!("variant code `{v}` exceeds 16 bits")))
        .transpose()
}

fn wrap(opts: &HashMap<String, String>, flags: &[String]) -> Result<(), String> {
    if !flags.is_empty() {
        return Err(format!("wrap takes no flags\n{USAGE}"));
    }
    let body = read(req(opts, "body")?)?;
    let entry = num(req(opts, "entry-offset")?)?;
    let sequence = num(req(opts, "sequence")?)?;
    let variant = match (opts.get("variant").map(String::as_str), variant_code(opts)?) {
        (Some(_), Some(_)) => return Err("give --variant or --variant-code, not both".into()),
        (Some("oled"), None) => Variant::Oled.code(),
        (Some("7seg"), None) => Variant::SevenSeg.code(),
        (Some(v), None) => return Err(format!("unknown variant `{v}` (oled or 7seg)")),
        (None, Some(code)) => code,
        (None, None) => return Err(format!("missing --variant or --variant-code\n{USAGE}")),
    };
    let header = slot::encode_header_with(&body, variant, sequence, entry, layout(opts, false)?)
        .map_err(|e| format!("cannot wrap this body: {e:?}"))?;
    let mut img = header.to_vec();
    img.extend_from_slice(&body);
    write(req(opts, "out")?, &img)
}

fn app(opts: &HashMap<String, String>, flags: &[String]) -> Result<(), String> {
    if !flags.is_empty() {
        return Err(format!("app takes no flags\n{USAGE}"));
    }
    let variant = variant_code(opts)?.ok_or(format!("missing --variant-code\n{USAGE}"))?;
    let (load, entry, body) = flatten(&read(req(opts, "elf")?)?)?;
    let mut img = deluge_image::app::encode_app_header(&body, variant, load, entry).to_vec();
    img.extend_from_slice(&body);
    // What the loader will check, so a bad image fails here, not on the device.
    deluge_image::app::validate_app(&img, &[variant], u32::MAX)
        .map_err(|e| format!("the flattened image doesn't validate: {e:?}"))?;
    write(req(opts, "out")?, &img)
}

/// An ELF32 LE executable's `PT_LOAD` file images laid out by physical address from the lowest:
/// `(load address, entry, body)`. Gaps between segments are zeros; the memory-only tail of a
/// segment (`.bss`) is left to the image's own start-up code. Overlapping segments are refused.
fn flatten(elf: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    use deluge_image::elf::{PT_LOAD, le16, le32, validate_header};
    validate_header(elf).map_err(|e| format!("not a loadable ELF: {e:?}"))?;
    let (entry, phoff) = (le32(elf, 24), le32(elf, 28) as usize);
    let (phentsize, phnum) = (le16(elf, 42) as usize, le16(elf, 44) as usize);
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
        let bytes = elf
            .get(offset as usize..offset as usize + filesz as usize)
            .ok_or("a segment's file image runs past the end of the file")?;
        segs.push((paddr, bytes));
    }
    segs.sort_by_key(|&(p, _)| p);
    let load = segs.first().ok_or("no PT_LOAD segment with file data")?.0;
    let mut body: Vec<u8> = Vec::new();
    for (paddr, bytes) in segs {
        let at = (paddr - load) as usize;
        if at < body.len() {
            return Err(format!("segment at {paddr:#x} overlaps the one before it"));
        }
        body.resize(at, 0);
        body.extend_from_slice(bytes);
    }
    Ok((load, entry, body))
}

fn compose(opts: &HashMap<String, String>, flags: &[String]) -> Result<(), String> {
    let layout = layout(opts, true)?;
    let accepts: Vec<u16> = match variant_code(opts)? {
        Some(code) => vec![code],
        None => vec![Variant::Oled.code(), Variant::SevenSeg.code()],
    };
    let stage0 = read(req(opts, "stage0")?)?;
    // Stage-0's region ends where slot A starts.
    let stage0_len = layout.slot_offsets[0] as usize;
    if stage0.len() > stage0_len {
        return Err(format!(
            "stage-0 is {} bytes, more than its {stage0_len}-byte region",
            stage0.len()
        ));
    }
    let corrupt = |flag: &str| flags.iter().any(|f| f == flag);
    let mut out = stage0;
    for (key, flag, at) in [
        ("slot-a", "corrupt-body", layout.slot_offsets[0]),
        ("slot-b", "corrupt-body-b", layout.slot_offsets[1]),
    ] {
        let Some(path) = opts.get(key) else {
            if corrupt(flag) {
                return Err(format!("--{flag} needs --{key}"));
            }
            continue;
        };
        let mut img = read(path)?;
        if img.len() > layout.slot_len as usize {
            return Err(format!(
                "{path} is {} bytes, more than a {}-byte slot",
                img.len(),
                layout.slot_len
            ));
        }
        let header = slot::validate_with(&img, layout, &accepts)
            .map_err(|e| format!("{path} is not a valid slot: {e:?}"))?;
        if corrupt(flag) {
            img[HEADER_LEN + header.body_len as usize - 1] ^= 0xFF;
        }
        out.resize(at as usize, 0xFF);
        out.extend_from_slice(&img);
    }
    write(req(opts, "out")?, &out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ELF32 LE ARM executable with `segs` as `(paddr, bytes)` `PT_LOAD`s and entry `entry`.
    fn elf(entry: u32, segs: &[(u32, &[u8])]) -> Vec<u8> {
        let phoff = 52u32;
        let data_at = phoff + 32 * segs.len() as u32;
        let mut e = vec![0u8; 52];
        e[..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        e[4] = 1; // ELFCLASS32
        e[5] = 1; // little-endian
        e[6] = 1;
        e[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        e[18..20].copy_from_slice(&0x28u16.to_le_bytes()); // EM_ARM
        e[20..24].copy_from_slice(&1u32.to_le_bytes());
        e[24..28].copy_from_slice(&entry.to_le_bytes());
        e[28..32].copy_from_slice(&phoff.to_le_bytes());
        e[40..42].copy_from_slice(&52u16.to_le_bytes());
        e[42..44].copy_from_slice(&32u16.to_le_bytes());
        e[44..46].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        let mut off = data_at;
        for &(paddr, bytes) in segs {
            let mut ph = [0u8; 32];
            ph[0..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
            ph[4..8].copy_from_slice(&off.to_le_bytes());
            ph[8..12].copy_from_slice(&paddr.to_le_bytes()); // vaddr
            ph[12..16].copy_from_slice(&paddr.to_le_bytes());
            ph[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            ph[20..24].copy_from_slice(&(bytes.len() as u32 + 16).to_le_bytes()); // memsz > filesz
            e.extend_from_slice(&ph);
            off += bytes.len() as u32;
        }
        for &(_, bytes) in segs {
            e.extend_from_slice(bytes);
        }
        e
    }

    #[test]
    fn app_flattens_two_segments() {
        let f = elf(
            0x2002_0000,
            &[(0x2002_0000, &[1, 2, 3, 4]), (0x2002_0010, &[9, 9])],
        );
        let (load, entry, body) = flatten(&f).unwrap();
        assert_eq!((load, entry), (0x2002_0000, 0x2002_0000));
        // The gap between the segments is zeros; nothing past the last file byte.
        assert_eq!(body, [1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 9]);
    }

    #[test]
    fn app_refuses_overlapping_segments() {
        let f = elf(0x100, &[(0x100, &[1, 2, 3, 4]), (0x102, &[9, 9])]);
        assert!(flatten(&f).is_err());
    }

    #[test]
    fn app_refuses_a_non_elf() {
        assert!(flatten(b"not an elf at all, really not").is_err());
    }
}
