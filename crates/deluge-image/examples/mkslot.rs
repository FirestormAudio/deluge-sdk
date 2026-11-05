//! Build boot-slot images for stage-0, on the host.
//!
//! ```text
//! mkslot wrap --body <bin> --entry-offset <n> --sequence <n> --variant oled|7seg --out <img>
//! mkslot compose --stage0 <bin> [--slot-a <img>] [--corrupt-body] --out <bin>
//! ```
//!
//! `wrap` puts a slot header page ([`deluge_image::slot`]) in front of a raw XIP body.
//! `compose` builds an image for flash offset 0: stage-0, padded with `0xFF` to slot A, then
//! the slot image; `--corrupt-body` flips the last body byte, for a slot stage-0 must refuse.
//!
//! Run with `cargo run --target x86_64-unknown-linux-gnu -p deluge-image --example mkslot -- …`.
//! Numbers take a `0x` prefix for hex.

use std::collections::HashMap;
use std::process::ExitCode;

use deluge_image::slot::{self, HEADER_LEN, SLOT_A_OFFSET, SLOT_LEN, Variant};

/// Stage-0's flash region: one 64 KB erase block, ending where slot A starts.
const STAGE0_LEN: usize = SLOT_A_OFFSET as usize;

const USAGE: &str = "usage:
  mkslot wrap --body <bin> --entry-offset <n> --sequence <n> --variant oled|7seg --out <img>
  mkslot compose --stage0 <bin> [--slot-a <img>] [--corrupt-body] --out <bin>";

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
        "wrap" => &["body", "entry-offset", "sequence", "variant", "out"],
        "compose" => &["stage0", "slot-a", "out"],
        _ => return Err(USAGE.into()),
    };
    let (opts, flags) = parse_opts(rest, known)?;
    match cmd.as_str() {
        "wrap" => wrap(&opts, &flags),
        _ => compose(&opts, &flags),
    }
}

/// Split `--key value` options from bare `--flag`s (only `--corrupt-body` is a flag), refusing
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
        if key == "corrupt-body" {
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

fn wrap(opts: &HashMap<String, String>, flags: &[String]) -> Result<(), String> {
    if !flags.is_empty() {
        return Err(format!("wrap takes no flags\n{USAGE}"));
    }
    let body = read(req(opts, "body")?)?;
    let entry = num(req(opts, "entry-offset")?)?;
    let sequence = num(req(opts, "sequence")?)?;
    let variant = match req(opts, "variant")? {
        "oled" => Variant::Oled,
        "7seg" => Variant::SevenSeg,
        v => return Err(format!("unknown variant `{v}` (oled or 7seg)")),
    };
    let header = slot::encode_header(&body, variant, sequence, entry)
        .map_err(|e| format!("cannot wrap this body: {e:?}"))?;
    let mut img = header.to_vec();
    img.extend_from_slice(&body);
    write(req(opts, "out")?, &img)
}

fn compose(opts: &HashMap<String, String>, flags: &[String]) -> Result<(), String> {
    let stage0 = read(req(opts, "stage0")?)?;
    if stage0.len() > STAGE0_LEN {
        return Err(format!(
            "stage-0 is {} bytes, more than its {STAGE0_LEN}-byte region",
            stage0.len()
        ));
    }
    let mut out = stage0;
    if let Some(path) = opts.get("slot-a") {
        let mut img = read(path)?;
        if img.len() > SLOT_LEN as usize {
            return Err(format!(
                "{path} is {} bytes, more than a {SLOT_LEN}-byte slot",
                img.len()
            ));
        }
        let header =
            slot::validate(&img).map_err(|e| format!("{path} is not a valid slot: {e:?}"))?;
        if flags.iter().any(|f| f == "corrupt-body") {
            img[HEADER_LEN + header.body_len as usize - 1] ^= 0xFF;
        }
        out.resize(STAGE0_LEN, 0xFF);
        out.extend_from_slice(&img);
    } else if !flags.is_empty() {
        return Err("--corrupt-body needs --slot-a".into());
    }
    write(req(opts, "out")?, &out)
}
