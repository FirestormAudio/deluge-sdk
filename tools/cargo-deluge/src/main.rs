//! `cargo deluge` — build and scaffold Deluge SDK apps.
//!
//! A thin host-side cargo subcommand so app authors never touch `-Zbuild-std`,
//! linker flags, or the embedded target triple. Pure std, no dependencies (like
//! `tools/elf2uf2`).
//!
//! Subcommands:
//! - `cargo deluge new <name>`  — scaffold a new app crate.
//! - `cargo deluge build [--release]` — build the current app → ELF.
//! - `cargo deluge run [--release] [--dest <sd-mount>]` — build, then copy the
//!   ELF into `/APPS/` on a mounted Deluge SD card (or print how to).
//!
//! Apps are loaded by the app-loader as **ELF** from `/APPS/` and run from RAM;
//! this tool only ever emits an ELF (UF2 is the separate firmware-flash path).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Embedded target triple for the Deluge (RZ/A1L, Cortex-A9).
const TARGET: &str = "armv7a-none-eabihf";

// Canonical project files, baked in from the repo so generated apps can't drift
// from the SDK's own build setup.
const TPL_BUILD_RS: &str = include_str!("../../../examples/blinky/build.rs");
const TPL_MEMORY_X: &str = include_str!("../../../examples/blinky/memory.x");
const TPL_MEMORY_RTT_X: &str = include_str!("../../../examples/blinky/memory_rtt.x");
const TPL_TOOLCHAIN: &str = include_str!("../../../rust-toolchain.toml");

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    // When invoked as `cargo deluge …`, cargo passes "deluge" as the first arg.
    if args.first().map(String::as_str) == Some("deluge") {
        args.remove(0);
    }

    let cmd = args.first().cloned().unwrap_or_default();
    let rest = &args[args.len().min(1)..];

    let result = match cmd.as_str() {
        "new" => cmd_new(rest),
        "build" => cmd_build(rest).map(|_| ()),
        "run" => cmd_run(rest),
        "" | "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        other => Err(format!("unknown subcommand `{other}`\n\n{HELP}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

const HELP: &str = "\
Usage: cargo deluge <command>

Commands:
  new <name>                 Scaffold a new Deluge app crate
  build [--release]          Build the current app for the Deluge (-> ELF)
  run [--release] [--dest D] Build, then copy the ELF into <D>/APPS/ (a mounted
                             Deluge SD card), or print how to deploy it
  help                       Show this help";

fn print_help() {
    println!("{HELP}");
}

// ── new ─────────────────────────────────────────────────────────────────────

fn cmd_new(args: &[String]) -> Result<(), String> {
    let name = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .ok_or("`new` requires an app name: cargo deluge new <name>")?;
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("invalid app name `{name}` (use letters, digits, _ or -)"));
    }

    let dir = PathBuf::from(name);
    if dir.exists() {
        return Err(format!("`{}` already exists", dir.display()));
    }

    // Point the new app at the SDK's `deluge` crate if we can find it (in-repo
    // use); otherwise fall back to a crates.io version requirement.
    let deluge_dep = match find_sdk_deluge() {
        Some(p) => format!("deluge = {{ path = {:?} }}", p.display().to_string()),
        None => {
            eprintln!(
                "note: couldn't locate the SDK's `deluge` crate; using a version \
                 requirement. Edit Cargo.toml to point at your SDK checkout if needed."
            );
            "deluge = \"0.1\"".to_string()
        }
    };

    let crate_name = name.replace('-', "_");

    write(&dir.join("Cargo.toml"), &cargo_toml(name, &deluge_dep))?;
    write(&dir.join("rust-toolchain.toml"), TPL_TOOLCHAIN)?;
    write(&dir.join(".cargo/config.toml"), CARGO_CONFIG)?;
    write(&dir.join("build.rs"), TPL_BUILD_RS)?;
    write(&dir.join("memory.x"), TPL_MEMORY_X)?;
    write(&dir.join("memory_rtt.x"), TPL_MEMORY_RTT_X)?;
    write(&dir.join("src/main.rs"), &main_rs(&crate_name))?;
    write(&dir.join(".gitignore"), "/target\n")?;

    println!("created Deluge app `{name}`");
    println!("  cd {name}");
    println!("  cargo deluge run        # build + deploy to your Deluge SD card");
    Ok(())
}

/// Walk up from the current directory looking for the SDK's `deluge` crate.
fn find_sdk_deluge() -> Option<PathBuf> {
    let mut dir = env::current_dir().ok()?;
    loop {
        let candidate = dir.join("deluge");
        if candidate.join("Cargo.toml").is_file() {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
        if !dir.pop() {
            return None;
        }
    }
}

// ── build ─────────────────────────────────────────────────────────────────────

/// Build the current app; returns the path to the produced ELF.
fn cmd_build(args: &[String]) -> Result<PathBuf, String> {
    let release = args.iter().any(|a| a == "--release");

    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--target",
        TARGET,
        "-Zbuild-std=core",
        "-Zbuild-std-features=compiler-builtins-mem",
    ]);
    if release {
        cmd.arg("--release");
    }

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err("build failed".to_string());
    }

    let name = package_name()?;
    let profile = if release { "release" } else { "debug" };
    let elf = PathBuf::from("target").join(TARGET).join(profile).join(&name);
    if !elf.is_file() {
        return Err(format!("expected ELF not found at {}", elf.display()));
    }
    println!("built {}", elf.display());
    Ok(elf)
}

// ── run ─────────────────────────────────────────────────────────────────────

fn cmd_run(args: &[String]) -> Result<(), String> {
    let elf = cmd_build(args)?;
    let dest = arg_value(args, "--dest");

    let file_name = format!("{}.elf", elf.file_name().unwrap().to_string_lossy());

    match dest {
        Some(root) => {
            let apps = Path::new(&root).join("APPS");
            fs::create_dir_all(&apps)
                .map_err(|e| format!("creating {}: {e}", apps.display()))?;
            let target = apps.join(&file_name);
            fs::copy(&elf, &target).map_err(|e| format!("copying to {}: {e}", target.display()))?;
            println!("deployed -> {}", target.display());
            println!("power-cycle the Deluge (or re-enter the app menu) to run it.");
        }
        None => {
            println!();
            println!("To run it on a Deluge:");
            println!("  1. Connect USB and enter DATA TRANSFER mode (SD card mounts as a drive).");
            println!("  2. Copy the ELF to /APPS/ on the card, e.g. as {file_name}.");
            println!("  3. Power-cycle / pick it from the app menu.");
            println!();
            println!("Or re-run with: cargo deluge run --dest <sd-mount-point>");
        }
    }
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Read the package (bin) name from `./Cargo.toml` (minimal TOML scan).
fn package_name() -> Result<String, String> {
    let toml = fs::read_to_string("Cargo.toml")
        .map_err(|_| "no Cargo.toml in the current directory — run inside an app".to_string())?;
    let mut in_package = false;
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = t.strip_prefix("name") {
                if let Some(q1) = rest.find('"') {
                    if let Some(q2) = rest[q1 + 1..].find('"') {
                        return Ok(rest[q1 + 1..q1 + 1 + q2].to_string());
                    }
                }
            }
        }
    }
    Err("could not find package name in Cargo.toml".to_string())
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).cloned()
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("writing {}: {e}", path.display()))
}

// ── templates ───────────────────────────────────────────────────────────────

fn cargo_toml(name: &str, deluge_dep: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

# Standalone app: declare its own (empty) workspace so it doesn't get pulled
# into a surrounding workspace.
[workspace]

[[bin]]
name = "{name}"
test = false

[dependencies]
{deluge_dep}
embassy-executor = {{ version = "0.10", features = ["nightly", "platform-cortex-ar", "executor-thread"] }}
embassy-time = {{ version = "0.5", features = ["tick-hz-1_000_000"] }}

[features]
default = []
## Opt-in RTT (SEGGER) logging over a debug probe; off by default (reserves no RAM).
rtt = ["deluge/rtt"]

[profile.release]
opt-level = "s"
lto = true
codegen-units = 1
debug = true
"#
    )
}

const CARGO_CONFIG: &str = r#"# Build for the Deluge (RZ/A1L, Cortex-A9) by default, with build-std so a plain
# `cargo build` works. `cargo deluge build` passes the same flags explicitly.
[build]
target = "armv7a-none-eabihf"

[target.armv7a-none-eabihf]
rustflags = [
    "-C", "target-cpu=cortex-a9",
    "-C", "target-feature=+neon",
    "-C", "link-arg=--gc-sections",
    "-C", "force-frame-pointers=yes",
]

[unstable]
build-std = ["core"]
build-std-features = ["compiler-builtins-mem"]
"#;

fn main_rs(_crate_name: &str) -> String {
    r#"//! A Deluge app.

#![no_std]
#![no_main]
// Required by the Embassy task the `#[deluge::app]` macro generates.
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use embassy_time::Timer;

#[deluge::app]
async fn main(dlg: Deluge) {
    // The platform (heaps, clocks, interrupts, executor, panic handler) is
    // already up. Capabilities are taken from the `dlg` handle:
    //   let mut oled = dlg.oled().await;
    //   let input = dlg.input();
    //   let mut pads = dlg.pads().await;
    let mut led = dlg.sync_led();
    loop {
        led.toggle();
        Timer::after_millis(200).await;
    }
}
"#
    .to_string()
}
