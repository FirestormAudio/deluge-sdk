//! `cargo deluge loader`: build the app-loader's flashable `app-loader.bin`.
//!
//! The app-loader is not on crates.io (`publish = false`), so this only works
//! inside an SDK checkout. It converts the ELF with the toolchain's own
//! `llvm-objcopy` ([`locate_objcopy`]), so cargo-binutils is not needed.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::TARGET;
use crate::build::target_dir_in;
use crate::util::{arg_value, locate_objcopy};

/// The app-loader's manifest, relative to the SDK checkout root.
const LOADER_MANIFEST: &str = "firmwares/app-loader/Cargo.toml";
/// Where the prebuilt `app-loader.bin` is published.
const RELEASES_URL: &str = "https://github.com/FirestormAudio/deluge-sdk/releases";

/// Build `app-loader.bin`; with `--dest <sd-mount>`, also copy it to the card's
/// top level, ready to flash.
pub(crate) fn cmd_loader(args: &[String]) -> Result<(), String> {
    let cwd = env::current_dir().map_err(|e| format!("reading the current directory: {e}"))?;
    let root = find_checkout(&cwd).ok_or_else(|| {
        format!(
            "not inside a deluge-sdk checkout, so the app-loader can't be built here.\n\
             Download app-loader.bin (and check it against SHA256SUMS) from {RELEASES_URL},\n\
             or clone the repository and run this from inside it."
        )
    })?;

    let status = Command::new("cargo")
        .args(build_args())
        .current_dir(&root)
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err("app-loader build failed".to_string());
    }

    let elf = target_dir_in(&root)?
        .join(TARGET)
        .join("release")
        .join("app-loader");
    if !elf.is_file() {
        return Err(format!("expected ELF not found at {}", elf.display()));
    }
    let objcopy = locate_objcopy()
        .ok_or("llvm-objcopy not found (add it with `rustup component add llvm-tools-preview`)")?;
    let bin = elf.with_extension("bin");
    let status = Command::new(&objcopy)
        .args(["-O", "binary"])
        .arg(&elf)
        .arg(&bin)
        .status()
        .map_err(|e| format!("running {}: {e}", objcopy.display()))?;
    if !status.success() {
        return Err("llvm-objcopy -O binary failed".to_string());
    }
    println!("built {}", bin.display());

    match arg_value(args, "--dest") {
        Some(dest) => {
            let target = Path::new(&dest).join("app-loader.bin");
            fs::copy(&bin, &target).map_err(|e| format!("copying to {}: {e}", target.display()))?;
            println!("copied -> {}", target.display());
            println!(
                "eject the card, put it in the Deluge, and hold SHIFT while powering on to flash it."
            );
        }
        None => {
            println!("To flash it: copy app-loader.bin to the top level of the SD card, then hold");
            println!("SHIFT while powering on the Deluge.");
            println!("Or re-run with: cargo deluge loader --dest <sd-mount-point>");
        }
    }
    Ok(())
}

/// Find the SDK checkout containing `start`: the nearest ancestor holding the
/// app-loader crate.
pub(crate) fn find_checkout(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(LOADER_MANIFEST).is_file())
        .map(Path::to_path_buf)
}

/// The `cargo` arguments that build the loader, matching the workspace's
/// `build-app-loader-bin` alias.
pub(crate) fn build_args() -> Vec<&'static str> {
    vec![
        "build",
        "--target",
        TARGET,
        "-Zbuild-std=core",
        "-Zbuild-std-features=compiler-builtins-mem",
        "--release",
        "--no-default-features",
        "-p",
        "app-loader",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }

    #[test]
    fn finds_checkout_from_a_nested_dir() {
        let root = checkout_root();
        let nested = root.join("tools/cargo-deluge/src");
        assert_eq!(find_checkout(&nested).unwrap(), root);
    }

    #[test]
    fn finds_no_checkout_outside_it() {
        assert_eq!(find_checkout(Path::new("/")), None);
    }

    #[test]
    fn build_args_match_the_workspace_alias() {
        // The alias stays the CI path (images.yml), so the two must not drift.
        let config = include_str!("../../../.cargo/config.toml");
        let alias = config
            .lines()
            .find_map(|l| l.strip_prefix("build-app-loader-bin"))
            .expect("build-app-loader-bin alias");
        let alias = alias.split('"').nth(1).unwrap();
        // `objcopy <cargo args> -- <objcopy args>`: keep the cargo half.
        let cargo_half = alias
            .strip_prefix("objcopy ")
            .unwrap()
            .split(" -- ")
            .next()
            .unwrap();
        let mut ours: Vec<&str> = build_args();
        assert_eq!(ours.remove(0), "build");
        let mut want: Vec<&str> = cargo_half.split_whitespace().collect();
        want.extend(["--target", crate::TARGET]);
        ours.sort();
        want.sort();
        assert_eq!(ours, want);
    }
}
