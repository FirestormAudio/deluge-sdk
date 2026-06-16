//! `cargo deluge` — build and scaffold Deluge SDK apps.
//!
//! A thin host-side cargo subcommand so app authors never touch `-Zbuild-std`,
//! linker flags, or the embedded target triple. Pure std, no dependencies.
//!
//! Subcommands:
//! - `cargo deluge new <name>`  — scaffold a new app crate.
//! - `cargo deluge build [--release]` — build the current app → ELF.
//! - `cargo deluge run [--release] [--port <path>] [--log]` — build, then upload
//!   the ELF straight to a Deluge over USB (dev mode) and launch it from RAM.
//!   `--dest <sd-mount>` keeps the legacy "copy to /APPS/" behaviour.
//! - `cargo deluge debug [--release] [-- <args>]` — build, then `probe-rs run`
//!   (J-Link).
//! - `cargo deluge trace [--release] [--flow] [--duration-ms N] [-- <args>]` —
//!   build, then `probe-rs read-trace` (J-Link, `trace-a9` fork).
//!
//! Apps are loaded by the app-loader as **ELF** and run from RAM; this tool only
//! ever emits an ELF (UF2 is the separate firmware-flash path).

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

/// Embedded target triple for the Deluge (RZ/A1L, Cortex-A9).
const TARGET: &str = "armv7a-none-eabihf";
/// probe-rs chip identifier for the RZ/A1L on the Deluge.
const CHIP: &str = "R7S721020";

/// USB VID/PID the Deluge presents in every device mode (shared with the
/// app-loader / firmware — see `app-loader/src/devupload.rs`).
const DELUGE_VID: u16 = 0x16D0;
const DELUGE_PID: u16 = 0x0EDA;
/// Product string the dev-upload CDC listener advertises.
const UPLOAD_PRODUCT: &str = "Dev Upload";

/// Upload wire-frame: `magic | version | flags | len u32 | crc32 u32 | ELF`.
const FRAME_MAGIC: &[u8; 4] = b"DLUP";
const FRAME_VERSION: u8 = 1;

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
        "deploy" => cmd_deploy(rest),
        "debug" => cmd_debug(rest),
        "trace" => cmd_trace(rest),
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
  run [--release] [opts]     Build, then upload the ELF to a Deluge over USB and
                             launch it from RAM (requires DEV MODE on the unit).
                               --port <path>  serial port override (else auto)
                               --log          tail the app's USB log after launch
  deploy [--release] [opts]  Build, then copy the ELF to a mounted Deluge SD card.
                               --dest <dir>   copy to <dir>/APPS/ (else print how)
  debug [--release] [-- ...] Build, then `probe-rs run` over J-Link (--chip set)
  trace [--release] [opts]   Build, then `probe-rs read-trace` (trace-a9 fork):
                               --flow         compact execution-flow view
                               --duration-ms N capture window (default 2000)
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
    validate_app_name(name)?;

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

    scaffold(&dir, name, &deluge_dep)?;

    println!("created Deluge app `{name}`");
    println!("  cd {name}");
    println!("  cargo deluge run        # build + upload over USB (DEV MODE)");
    println!("  cargo deluge deploy --dest <sd-mount>   # or copy to the SD card");
    Ok(())
}

/// An app name must be a single path-safe token (letters, digits, `_`, `-`).
fn validate_app_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("app name must not be empty".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("invalid app name `{name}` (use letters, digits, _ or -)"));
    }
    Ok(())
}

/// Write the canonical app file-set into `dir` (already verified not to exist).
/// Split out from [`cmd_new`] so the full scaffold is testable in a temp dir.
fn scaffold(dir: &Path, name: &str, deluge_dep: &str) -> Result<(), String> {
    let crate_name = name.replace('-', "_");
    write(&dir.join("Cargo.toml"), &cargo_toml(name, deluge_dep))?;
    write(&dir.join("rust-toolchain.toml"), TPL_TOOLCHAIN)?;
    write(&dir.join(".cargo/config.toml"), CARGO_CONFIG)?;
    write(&dir.join("build.rs"), TPL_BUILD_RS)?;
    write(&dir.join("memory.x"), TPL_MEMORY_X)?;
    write(&dir.join("memory_rtt.x"), TPL_MEMORY_RTT_X)?;
    write(&dir.join("src/main.rs"), &main_rs(&crate_name))?;
    write(&dir.join(".gitignore"), "/target\n")?;
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
    // `run` is USB upload only; the SD build-and-copy path is `deploy`.
    if arg_value(args, "--dest").is_some() {
        return Err(
            "`run` uploads over USB; use `cargo deluge deploy --dest <sd-mount>` to copy \
             the ELF to /APPS/ instead."
                .to_string(),
        );
    }

    let elf = cmd_build(args)?;

    // The Deluge must be sitting on the boot menu with DEV MODE on (its
    // background CDC listener is what we upload to).
    let bytes = fs::read(&elf).map_err(|e| format!("reading {}: {e}", elf.display()))?;
    let frame = build_frame(&bytes);

    let port_path = match arg_value(args, "--port") {
        Some(p) => p,
        None => discover_upload_port()?,
    };
    println!("uploading {} ({} bytes) to {port_path}", elf.display(), bytes.len());

    let mut port = serialport::new(&port_path, 115_200)
        .timeout(Duration::from_secs(5))
        .open()
        .map_err(|e| format!("opening {port_path}: {e}"))?;

    upload(&mut *port, &frame)?;
    println!("\nuploaded — the Deluge is loading and launching it from RAM.");

    if args.iter().any(|a| a == "--log") {
        // The launched app re-enumerates as its own USB-log CDC; reconnect and
        // tail it. Best-effort: the port path may change after re-enumeration.
        drop(port);
        tail_log()?;
    }
    Ok(())
}

// ── deploy ────────────────────────────────────────────────────────────────────

/// Build, then copy the ELF into `<dest>/APPS/<name>.elf` on a mounted Deluge SD
/// card. With no `--dest`, print how to deploy it by hand.
fn cmd_deploy(args: &[String]) -> Result<(), String> {
    let elf = cmd_build(args)?;
    let file_name = format!("{}.elf", elf.file_name().unwrap().to_string_lossy());

    match arg_value(args, "--dest") {
        Some(root) => {
            let apps = Path::new(&root).join("APPS");
            fs::create_dir_all(&apps).map_err(|e| format!("creating {}: {e}", apps.display()))?;
            let target = apps.join(&file_name);
            fs::copy(&elf, &target)
                .map_err(|e| format!("copying to {}: {e}", target.display()))?;
            println!("deployed -> {}", target.display());
            println!("power-cycle the Deluge (or re-enter the app menu) to run it.");
        }
        None => {
            println!("To deploy it to a Deluge SD card:");
            println!("  1. Connect USB and enter DATA TRANSFER mode (the card mounts as a drive).");
            println!("  2. Copy the ELF to /APPS/ on the card, e.g. as {file_name}.");
            println!("  3. Power-cycle / pick it from the app menu.");
            println!();
            println!("Or re-run with: cargo deluge deploy --dest <sd-mount-point>");
            println!("(For probe-less push-to-run, use `cargo deluge run` with DEV MODE on.)");
        }
    }
    Ok(())
}

/// Write the framed image, showing a local progress bar (USB bulk flow-control
/// naturally paces this against how fast the device drains the data).
fn upload(port: &mut dyn serialport::SerialPort, frame: &[u8]) -> Result<(), String> {
    const CHUNK: usize = 4096;
    let mut written = 0usize;
    let total = frame.len();
    while written < total {
        let n = (total - written).min(CHUNK);
        port.write_all(&frame[written..written + n])
            .map_err(|e| format!("writing to port: {e}"))?;
        written += n;
        print_progress(written, total);
    }
    port.flush().map_err(|e| format!("flushing port: {e}"))?;
    Ok(())
}

/// Print/refresh a single-line `[####    ] 42%` progress bar on stderr.
fn print_progress(done: usize, total: usize) {
    let pct = if total == 0 { 100 } else { done * 100 / total };
    let filled = pct * 30 / 100;
    let bar: String = (0..30)
        .map(|i| if i < filled { '#' } else { ' ' })
        .collect();
    eprint!("\r[{bar}] {pct:>3}%");
    let _ = std::io::stderr().flush();
}

/// Find the Deluge dev-upload CDC port: a USB serial port with the Deluge
/// VID/PID whose product string names the upload listener.  If exactly one
/// Deluge serial port is present we use it even without the product match (some
/// platforms don't expose product strings); ambiguity is reported.
fn discover_upload_port() -> Result<String, String> {
    use serialport::SerialPortType;
    let ports = serialport::available_ports()
        .map_err(|e| format!("enumerating serial ports: {e}"))?;

    let mut deluge: Vec<(String, Option<String>)> = Vec::new();
    for p in ports {
        if let SerialPortType::UsbPort(info) = &p.port_type {
            if info.vid == DELUGE_VID && info.pid == DELUGE_PID {
                deluge.push((p.port_name.clone(), info.product.clone()));
            }
        }
    }

    // Prefer a product-string match for the upload listener.
    if let Some((name, _)) = deluge
        .iter()
        .find(|(_, prod)| prod.as_deref().is_some_and(|s| s.contains(UPLOAD_PRODUCT)))
    {
        return Ok(name.clone());
    }

    match deluge.as_slice() {
        [] => Err(
            "no Deluge serial port found. Make sure the unit is on the boot menu \
             with DEV MODE: ON, connected over USB. Pass --port <path> to override."
                .to_string(),
        ),
        [(name, _)] => Ok(name.clone()),
        many => {
            let list = many
                .iter()
                .map(|(n, p)| format!("  {n}  ({})", p.as_deref().unwrap_or("?")))
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "multiple Deluge serial ports found; pick one with --port <path>:\n{list}"
            ))
        }
    }
}

/// Reconnect to a Deluge CDC port after the app re-enumerates and stream its
/// output to stdout until interrupted (Ctrl-C).
fn tail_log() -> Result<(), String> {
    use serialport::SerialPortType;
    println!("--log: waiting for the app's USB log port to appear (Ctrl-C to stop)…");

    // Poll for a Deluge serial port to (re)appear for a few seconds.
    let mut path = None;
    for _ in 0..50 {
        if let Ok(ports) = serialport::available_ports() {
            for p in ports {
                if let SerialPortType::UsbPort(info) = &p.port_type {
                    if info.vid == DELUGE_VID && info.pid == DELUGE_PID {
                        path = Some(p.port_name.clone());
                        break;
                    }
                }
            }
        }
        if path.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let path = path.ok_or("no Deluge log port appeared after launch")?;

    let mut port = serialport::new(&path, 115_200)
        .timeout(Duration::from_millis(500))
        .open()
        .map_err(|e| format!("opening {path}: {e}"))?;
    println!("--log: tailing {path}");

    let mut buf = [0u8; 512];
    let mut out = std::io::stdout();
    loop {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                let _ = out.write_all(&buf[..n]);
                let _ = out.flush();
            }
            // Read timeouts are expected when the app is idle; keep waiting.
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("reading log port: {e}")),
        }
    }
}

// ── debug / trace (probe-rs fork wrappers) ────────────────────────────────────

fn cmd_debug(args: &[String]) -> Result<(), String> {
    let elf = cmd_build(args)?;
    let mut cmd = Command::new("probe-rs");
    cmd.args(["run", "--chip", CHIP]);
    cmd.arg(&elf);
    cmd.args(passthrough_args(args));
    run_probe_rs(cmd)
}

fn cmd_trace(args: &[String]) -> Result<(), String> {
    let elf = cmd_build(args)?;
    let duration = arg_value(args, "--duration-ms").unwrap_or_else(|| "2000".to_string());

    let mut cmd = Command::new("probe-rs");
    cmd.args(["read-trace", "--chip", CHIP, "--duration-ms", &duration]);
    // --flow gives the compact execution-flow view; otherwise decode packets.
    if args.iter().any(|a| a == "--flow") {
        cmd.arg("--flow");
    } else {
        cmd.arg("--decode");
    }
    cmd.args(["--elf"]).arg(&elf);
    cmd.args(passthrough_args(args));
    run_probe_rs(cmd)
}

/// Run a configured probe-rs command, inheriting stdio, and turn a missing
/// binary into the fork-install instructions.
fn run_probe_rs(mut cmd: Command) -> Result<(), String> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("probe-rs exited with {s}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "`probe-rs` not found ({e}).\n\nInstall the Cortex-A9 trace fork:\n  \
             git clone -b trace-a9 https://github.com/stellar-aria/probe-rs\n  \
             cargo install --path probe-rs/probe-rs-tools --locked"
        )),
        Err(e) => Err(format!("failed to run probe-rs: {e}")),
    }
}

/// Everything after a literal `--` separator, passed through to probe-rs.
fn passthrough_args(args: &[String]) -> &[String] {
    match args.iter().position(|a| a == "--") {
        Some(i) => &args[i + 1..],
        None => &[],
    }
}

// ── upload framing ────────────────────────────────────────────────────────────

/// Build the upload wire-frame for `elf`:
/// `magic | version | flags | len u32 LE | crc32 u32 LE | <elf>`.
fn build_frame(elf: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + elf.len());
    frame.extend_from_slice(FRAME_MAGIC);
    frame.push(FRAME_VERSION);
    frame.push(0); // flags (reserved)
    frame.extend_from_slice(&(elf.len() as u32).to_le_bytes());
    frame.extend_from_slice(&crc32(elf).to_le_bytes());
    frame.extend_from_slice(elf);
    frame
}

/// CRC-32 (IEEE) — the same checksum the device computes
/// (`deluge_image::crc32`); a test pins the two implementations together.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Read the package (bin) name from `./Cargo.toml` (minimal TOML scan).
fn package_name() -> Result<String, String> {
    let toml = fs::read_to_string("Cargo.toml")
        .map_err(|_| "no Cargo.toml in the current directory — run inside an app".to_string())?;
    parse_package_name(&toml).ok_or_else(|| "could not find package name in Cargo.toml".to_string())
}

/// Extract `name = "…"` from the `[package]` table of a Cargo manifest.
/// A `name` key in any other table (e.g. `[[bin]]`) is ignored.
fn parse_package_name(toml: &str) -> Option<String> {
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
                        return Some(rest[q1 + 1..q1 + 1 + q2].to_string());
                    }
                }
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---- arg_value ---------------------------------------------------------

    #[test]
    fn arg_value_finds_and_misses() {
        let a: Vec<String> = ["run", "--dest", "/mnt/sd", "--release"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(arg_value(&a, "--dest"), Some("/mnt/sd".to_string()));
        assert_eq!(arg_value(&a, "--missing"), None);
    }

    #[test]
    fn arg_value_flag_without_value_is_none() {
        let a = vec!["--dest".to_string()];
        assert_eq!(arg_value(&a, "--dest"), None);
    }

    // ---- crc32 / framing ---------------------------------------------------

    #[test]
    fn crc32_matches_known_vectors() {
        // Standard CRC-32/ISO-HDLC check values.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    /// The host framing CRC must agree byte-for-byte with the device's
    /// `deluge_image::crc32`, or an upload would always be rejected.
    #[test]
    fn crc32_agrees_with_device() {
        for sample in [
            b"".as_slice(),
            b"123456789",
            b"\x7FELF\x01\x01\x01",
            &[0u8, 1, 2, 3, 255, 254, 7, 42, 99],
        ] {
            assert_eq!(crc32(sample), deluge_image::crc32(sample));
        }
    }

    #[test]
    fn frame_round_trips() {
        let elf = b"\x7FELF fake image bytes";
        let frame = build_frame(elf);

        assert_eq!(&frame[0..4], FRAME_MAGIC);
        assert_eq!(frame[4], FRAME_VERSION);
        assert_eq!(frame[5], 0, "flags reserved");
        let len = u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]);
        assert_eq!(len as usize, elf.len());
        let crc = u32::from_le_bytes([frame[10], frame[11], frame[12], frame[13]]);
        assert_eq!(crc, crc32(elf));
        assert_eq!(&frame[14..], elf, "payload follows the 14-byte header");
        assert_eq!(frame.len(), 14 + elf.len());
    }

    // ---- passthrough_args --------------------------------------------------

    #[test]
    fn passthrough_splits_on_double_dash() {
        let a: Vec<String> = ["trace", "--flow", "--", "--extra", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(passthrough_args(&a), &["--extra".to_string(), "1".to_string()]);

        let none: Vec<String> = vec!["debug".to_string(), "--release".to_string()];
        assert!(passthrough_args(&none).is_empty());
    }

    // ---- validate_app_name -------------------------------------------------

    #[test]
    fn app_name_accepts_valid() {
        for n in ["app", "my_app", "my-app", "app123", "A1-b_2"] {
            assert!(validate_app_name(n).is_ok(), "{n} should be valid");
        }
    }

    #[test]
    fn app_name_rejects_invalid() {
        for n in ["", "my app", "app/evil", "../esc", "app.rs", "app!"] {
            assert!(validate_app_name(n).is_err(), "{n:?} should be rejected");
        }
    }

    // ---- parse_package_name ------------------------------------------------

    #[test]
    fn package_name_from_package_table() {
        let toml = "[package]\nname = \"cool-app\"\nversion = \"0.1.0\"\n";
        assert_eq!(parse_package_name(toml).as_deref(), Some("cool-app"));
    }

    #[test]
    fn package_name_ignores_bin_table_name() {
        // The [[bin]] name must not shadow the [package] name, even when the
        // [[bin]] table appears first.
        let toml = "[[bin]]\nname = \"the-bin\"\n\n[package]\nname = \"the-pkg\"\n";
        assert_eq!(parse_package_name(toml).as_deref(), Some("the-pkg"));
    }

    #[test]
    fn package_name_absent() {
        assert_eq!(parse_package_name("[dependencies]\nfoo = \"1\"\n"), None);
    }

    // ---- templates ---------------------------------------------------------

    #[test]
    fn cargo_toml_template_is_coherent() {
        let t = cargo_toml("blinky", "deluge = \"0.1\"");
        assert!(t.contains("name = \"blinky\""));
        assert!(t.contains("deluge = \"0.1\""));
        assert!(t.contains("embassy-executor"));
        assert!(t.contains("rtt = [\"deluge/rtt\"]"));
        // The parser must be able to read the name back out.
        assert_eq!(parse_package_name(&t).as_deref(), Some("blinky"));
    }

    #[test]
    fn main_rs_template_is_an_app() {
        let m = main_rs("blinky");
        assert!(m.contains("#![no_std]"));
        assert!(m.contains("#![no_main]"));
        assert!(m.contains("#[deluge::app]"));
        assert!(m.contains("async fn main"));
    }

    #[test]
    fn baked_in_templates_nonempty() {
        assert!(!TPL_BUILD_RS.is_empty());
        assert!(!TPL_MEMORY_X.is_empty());
        assert!(!TPL_MEMORY_RTT_X.is_empty());
        assert!(!TPL_TOOLCHAIN.is_empty());
        assert!(TPL_TOOLCHAIN.contains("armv7a-none-eabihf"));
        assert!(CARGO_CONFIG.contains("build-std"));
        assert_eq!(TARGET, "armv7a-none-eabihf");
    }

    // ---- scaffold (full file-set into a temp dir, no chdir) ----------------

    /// Unique temp directory; zero-dependency (no `tempfile` crate), cleaned up
    /// by the returned guard's `Drop`.
    struct TmpDir(PathBuf);
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tmpdir() -> TmpDir {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = env::temp_dir().join(format!(
            "cargo-deluge-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }

    #[test]
    fn scaffold_writes_expected_file_set() {
        let tmp = tmpdir();
        let app = tmp.0.join("my-app");
        scaffold(&app, "my-app", "deluge = \"0.1\"").unwrap();

        for f in [
            "Cargo.toml",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            "build.rs",
            "memory.x",
            "memory_rtt.x",
            "src/main.rs",
            ".gitignore",
        ] {
            assert!(app.join(f).is_file(), "missing scaffolded file: {f}");
        }

        let cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
        assert_eq!(parse_package_name(&cargo).as_deref(), Some("my-app"));
        assert_eq!(fs::read_to_string(app.join(".gitignore")).unwrap(), "/target\n");
    }
}

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
