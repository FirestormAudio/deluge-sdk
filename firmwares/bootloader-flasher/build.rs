use std::env;
use std::fs;
use std::path::PathBuf;

/// The bottom 1 MB the bootloaders occupy (and flash-protect covers): the most an embedded image
/// may span.
const MAX_IMAGE: u64 = 0x10_0000;

fn main() {
    // rza1-rt puts `link.x` on the linker search path and deluge-bsp puts the Deluge's
    // `memory.x` next to it; this selects the script. Host (simulator) builds use the
    // platform's own linker setup.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
    }

    // What the tool writes is fixed at build time (see the crate docs):
    //   fsb   (default) — `BOOT.BIN` from the SD card to offset 0
    //   image — BOOTLOADER_FLASHER_IMAGE, embedded, to offset 0
    //   wipe  — erase the whole chip, then as `image`
    println!("cargo:rerun-if-env-changed=BOOTLOADER_FLASHER_MODE");
    println!("cargo:rerun-if-env-changed=BOOTLOADER_FLASHER_IMAGE");
    println!(r#"cargo:rustc-check-cfg=cfg(flasher_mode, values("fsb", "image", "wipe"))"#);
    let mode = env::var("BOOTLOADER_FLASHER_MODE").unwrap_or_else(|_| "fsb".into());
    match mode.as_str() {
        "fsb" => {}
        "image" | "wipe" => embed_image(&mode),
        other => panic!("BOOTLOADER_FLASHER_MODE={other}: expected one of fsb, image, wipe"),
    }
    println!("cargo:rustc-cfg=flasher_mode=\"{mode}\"");
}

/// Copy BOOTLOADER_FLASHER_IMAGE to `OUT_DIR/embedded.bin` for `include_bytes!`, refusing an
/// image that is missing, empty, larger than the bootloader region, or not a raw image that the
/// boot ROM can enter at offset 0. A relative path is taken from the workspace root, where the
/// cargo aliases run (a build script's own working directory is this package).
fn embed_image(mode: &str) {
    let given = env::var("BOOTLOADER_FLASHER_IMAGE").unwrap_or_else(|_| {
        panic!("BOOTLOADER_FLASHER_MODE={mode} requires BOOTLOADER_FLASHER_IMAGE=<path to a .bin>")
    });
    let path = PathBuf::from(&given);
    let path = if path.is_absolute() {
        path
    } else {
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("../..")
            .join(path)
    };
    let path = path
        .canonicalize()
        .unwrap_or_else(|e| panic!("BOOTLOADER_FLASHER_IMAGE={given}: {}: {e}", path.display()));
    println!("cargo:rerun-if-changed={}", path.display());
    let image = fs::read(&path).unwrap_or_else(|e| panic!("BOOTLOADER_FLASHER_IMAGE={given}: {e}"));
    if image.is_empty() {
        panic!("BOOTLOADER_FLASHER_IMAGE={given} is empty");
    }
    if image.len() as u64 > MAX_IMAGE {
        panic!(
            "BOOTLOADER_FLASHER_IMAGE={given} is {} bytes, larger than 1 MB (the bootloader region)",
            image.len()
        );
    }
    if let Err(why) = check_vectors(&image) {
        panic!("BOOTLOADER_FLASHER_IMAGE={given} is not a raw boot image: {why}");
    }
    println!(
        "cargo:warning=bootloader-flasher {mode}: embedding {} ({} bytes)",
        path.display(),
        image.len()
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("embedded.bin");
    fs::write(out, &image).unwrap();
}

/// The boot ROM branches to offset 0 and executes it, so the image must start with a vector
/// table: 8 ARM-state, always-executed `b` (whose target lies inside the image) or
/// `ldr pc, [pc, #imm]` (the stock first-stage bootloader's form). Rejects an ELF or any other
/// file passed by mistake, which would otherwise program and verify "fine" and never boot.
fn check_vectors(image: &[u8]) -> Result<(), String> {
    if image.starts_with(b"\x7fELF") {
        return Err("it is an ELF; pass the objcopy'd .bin".into());
    }
    if image.len() < 32 {
        return Err("shorter than a vector table".into());
    }
    for i in 0..8 {
        let w = u32::from_le_bytes(image[4 * i..4 * i + 4].try_into().unwrap());
        let ok = if w >> 24 == 0xEA {
            let imm = ((w & 0x00FF_FFFF) << 8) as i32 >> 6; // sign-extended, * 4
            let target = 4 * i as i64 + 8 + imm as i64;
            (0..image.len() as i64).contains(&target)
        } else {
            w & 0xFFFF_F000 == 0xE59F_F000
        };
        if !ok {
            return Err(format!(
                "vector {i} is {w:#010x}, not a `b` into the image or `ldr pc, [pc, #imm]`"
            ));
        }
    }
    Ok(())
}
