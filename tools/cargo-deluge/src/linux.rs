//! `cargo deluge linux`: build the current app for the Deluge's Linux userland
//! (armv7 musl, static) against the bundle's libdeluge sysroot, then pack it
//! into an app image — or a `--bare` binary — with the bundle's deluge-mkimage.
//!
//! Unlike the device path, this needs no `-Zbuild-std` (the musl std is a
//! rustup target) and no local sysroot build (the bundle ships libdeluge +
//! deluge-mkimage). Point it at an unpacked bundle with `DELUGE_BASE`.

use std::path::PathBuf;
use std::process::Command;

use crate::build::{package_name, target_dir};
use crate::util::{arg_value, strip_elf};

/// The Deluge's Linux userland triple (musl, static).
const LINUX_TARGET: &str = "armv7-unknown-linux-musleabihf";

/// The child `cargo build` for the Linux userland, against the bundle at `base`. It carries the
/// whole cross setup (linker, static CRT, the bundle's toolchain on `PATH`, the libdeluge
/// sysroot), so it works from any app, whatever its own `.cargo/config.toml` says.
fn build_command(base: &str, debug: bool, features: Option<&str>) -> Result<Command, String> {
    build_command_with(base, debug, features, |k| std::env::var(k).ok())
}

/// [`build_command`], reading the environment through `var`.
fn build_command_with(
    base: &str,
    debug: bool,
    features: Option<&str>,
    var: impl Fn(&str) -> Option<String>,
) -> Result<Command, String> {
    let sysroot = format!("{base}/toolchain/arm-buildroot-linux-musleabihf/sysroot/usr");
    // The bundle's toolchain first, so `arm-linux-gcc` resolves.
    let path = var("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(PathBuf::from(format!("{base}/toolchain/bin")))
            .chain(std::env::split_paths(&path)),
    )
    .map_err(|e| format!("PATH: {e}"))?;
    // The static CRT goes into whichever flags cargo will read: CARGO_ENCODED_RUSTFLAGS beats
    // RUSTFLAGS, which beats the per-target variable. With `--target`, none of them reach build
    // scripts.
    const CRT_STATIC: &str = "-C target-feature=+crt-static";
    let set = |k: &str| var(k).filter(|v| !v.trim().is_empty());
    let (rustflags_var, rustflags) = if let Some(encoded) = set("CARGO_ENCODED_RUSTFLAGS") {
        let static_crt = CRT_STATIC.replace(' ', "\x1f");
        (
            "CARGO_ENCODED_RUSTFLAGS",
            format!("{encoded}\x1f{static_crt}"),
        )
    } else if let Some(flags) = set("RUSTFLAGS") {
        ("RUSTFLAGS", format!("{flags} {CRT_STATIC}"))
    } else {
        let target_var = "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_RUSTFLAGS";
        match set(target_var) {
            Some(flags) => (target_var, format!("{flags} {CRT_STATIC}")),
            None => (target_var, CRT_STATIC.to_string()),
        }
    };

    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--target", LINUX_TARGET, "--no-default-features"]);
    if !debug {
        cmd.arg("--release");
    }
    if let Some(features) = features.filter(|f| !f.is_empty()) {
        cmd.arg("--features").arg(features);
    }
    cmd.env("DELUGE_SDK_ROOT", &sysroot)
        .env("PATH", &path)
        .env(
            "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER",
            "arm-linux-gcc",
        )
        .env(rustflags_var, rustflags);
    Ok(cmd)
}

pub(crate) fn cmd_linux(args: &[String]) -> Result<(), String> {
    let debug = args.iter().any(|a| a == "--debug");
    let bare = args.iter().any(|a| a == "--bare");
    let run = args.iter().any(|a| a == "--run");
    let out = arg_value(args, "--out");
    let features = arg_value(args, "--features");
    // Which bundle rootfs to bake the app into: `base` (the appliance default,
    // no shell/network) or `platform` (adds console getty, USB NCM, SSH).
    // Both boot the app from the same /usr/bin/deluge-app slot; left unset,
    // deluge-mkimage defaults to base.
    let image_profile = arg_value(args, "--profile");
    // A directory of extra files baked over the rootfs, passed straight to
    // deluge-mkimage. The point is testing a rootfs change (a fixed init or
    // service script) without a full Buildroot rebuild — the overlay wins over
    // the profile's own copy of the same path.
    let overlay = arg_value(args, "--overlay");

    // `--run` dev-uploads the bootable appliance *image* from RAM; `--bare`
    // emits a `/LINUX/APPS/` binary instead, which isn't a bootable image and
    // can't be launched this way.
    if run && bare {
        return Err("--run uploads the bootable appliance image; drop --bare".to_string());
    }
    // `--bare` emits a bare binary, never a rootfs, so neither a profile nor an
    // overlay means anything — reject rather than silently ignoring the flag.
    if bare && image_profile.is_some() {
        return Err("--bare emits a raw binary, not an image; drop --profile".to_string());
    }
    if bare && overlay.is_some() {
        return Err("--bare emits a raw binary, not an image; drop --overlay".to_string());
    }
    // Catch the typo'd/missing directory here rather than letting mkimage bake
    // a silently empty overlay — an image that looks right and behaves as if
    // the change were never made is the worst outcome for a bring-up test.
    if let Some(dir) = &overlay
        && !std::path::Path::new(dir).is_dir()
    {
        return Err(format!("--overlay {dir} is not a directory"));
    }

    let base = std::env::var("DELUGE_BASE")
        .map_err(|_| "set DELUGE_BASE to an unpacked deluge-linux bundle".to_string())?;
    let mkimage = PathBuf::from(&base).join("tools/deluge-mkimage");
    if !mkimage.is_file() {
        return Err(format!("bundle has no packer at {}", mkimage.display()));
    }

    let mut cmd = build_command(&base, debug, features.as_deref())?;
    if !cmd.status().map_err(|e| format!("cargo: {e}"))?.success() {
        return Err("build failed".to_string());
    }

    let name = package_name()?;
    let profile = if debug { "debug" } else { "release" };
    let bin = target_dir()?.join(LINUX_TARGET).join(profile).join(&name);
    if !bin.is_file() {
        return Err(format!("binary not found at {}", bin.display()));
    }

    // The workspace's release profile keeps debug info (for device gdb/RTT
    // work), which bloats a Linux appliance image well past the bundle's
    // rootfs.uimg size ceiling. Strip before packing unless asked not to;
    // `--bare` binaries aren't image-size-constrained but are stripped too
    // for a smaller card footprint.
    let no_strip = args.iter().any(|a| a == "--no-strip");
    let packed_bin = if no_strip {
        bin.clone()
    } else {
        strip_elf(&bin)?
    };

    let mut mk = Command::new(&mkimage);
    mk.arg(&packed_bin);
    if let Some(profile) = &image_profile {
        mk.arg("--profile").arg(profile);
    }
    if let Some(dir) = &overlay {
        mk.arg("--overlay").arg(dir);
    }
    let product = if bare {
        let p = target_dir()?.join("bare").join(&name);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        mk.arg("--bare").arg("-o").arg(&p);
        p
    } else {
        let stem = out.unwrap_or_else(|| name.to_uppercase());
        let p = target_dir()?.join(format!("{stem}.ELF"));
        mk.arg("-o").arg(&p);
        p
    };
    if !mk
        .status()
        .map_err(|e| format!("deluge-mkimage: {e}"))?
        .success()
    {
        return Err("deluge-mkimage failed".to_string());
    }
    println!("packed {}", product.display());

    if run {
        // The appliance image is already segment-only (deluge-mkimage's
        // output, not a raw cargo build ELF), so unlike `run` there's nothing
        // to strip here — go straight to the shared USB dev-upload path.
        crate::run::upload_elf(&product, args)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn crt_static_survives_a_global_rustflags() {
        // Cargo ignores target.<triple>.rustflags when RUSTFLAGS is set, so the flag goes there.
        let cmd = build_command_with("/b", false, None, |k| {
            (k == "RUSTFLAGS").then(|| "-C target-cpu=native".to_string())
        })
        .unwrap();
        assert_eq!(
            env(&cmd, "RUSTFLAGS").as_deref(),
            Some("-C target-cpu=native -C target-feature=+crt-static")
        );
    }

    #[test]
    fn crt_static_survives_encoded_rustflags() {
        let cmd = build_command_with("/b", false, None, |k| {
            (k == "CARGO_ENCODED_RUSTFLAGS").then(|| "-Cfoo\x1f-Cbar".to_string())
        })
        .unwrap();
        assert_eq!(
            env(&cmd, "CARGO_ENCODED_RUSTFLAGS").as_deref(),
            Some("-Cfoo\x1f-Cbar\x1f-C\x1ftarget-feature=+crt-static")
        );
    }

    #[test]
    fn linux_sets_the_cross_linker_for_the_child_cargo() {
        let cmd = build_command_with("/bundle", false, Some("linux"), |k| {
            (k == "PATH").then(|| "/usr/bin".to_string())
        })
        .unwrap();
        assert_eq!(
            env(&cmd, "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER").as_deref(),
            Some("arm-linux-gcc")
        );
        let flags = env(
            &cmd,
            "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_RUSTFLAGS",
        )
        .unwrap();
        assert!(flags.contains("target-feature=+crt-static"), "{flags}");
        let path = env(&cmd, "PATH").unwrap();
        let first = std::env::split_paths(&path).next().unwrap();
        assert_eq!(first, std::path::Path::new("/bundle/toolchain/bin"));
        assert_eq!(
            env(&cmd, "DELUGE_SDK_ROOT").as_deref(),
            Some("/bundle/toolchain/arm-buildroot-linux-musleabihf/sysroot/usr")
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a == "--release"), "{args:?}");
        assert!(
            args.windows(2).any(|w| w == ["--features", "linux"]),
            "{args:?}"
        );
    }
}
