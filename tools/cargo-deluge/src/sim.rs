//! `cargo deluge sim`: build the current app for the **host** and run it in the
//! desktop simulator.
//!
//! Unlike every other subcommand, this does not cross-compile to the Deluge: it
//! builds the app for the host triple, where the SDK's host backend swaps the
//! real peripherals for the in-process simulator panel (OLED/pads/LEDs/audio).
//! The app binary opens the simulator window itself, so this just runs it and
//! streams its output.
//!
//! Flags:
//!   - `--release`              optimized build
//!   - `--audio-in <file.wav>`  feed a WAV as the codec input (looped) instead
//!                              of the mic — handy for deterministic DSP runs
//!   - `--audio-out <file.wav>` record the codec output to a WAV
//!   - `--hardware [<port>]`    drive the in-process app from a physical Deluge
//!                              running controller-firmware over USB-CDC (and
//!                              mirror illumination back to it); auto-detects the
//!                              port if omitted

use std::path::PathBuf;
use std::process::Command;

pub(crate) fn cmd_sim(args: &[String]) -> Result<(), String> {
    let release = args.iter().any(|a| a == "--release");
    let host = host_triple()?;

    // `--target <host>` overrides the workspace's forced `armv7a-none-eabihf`
    // (`.cargo/config.toml`); building for the host auto-selects the SDK's host
    // backend via `cfg(not(target_os = "none"))`. Std is prebuilt, so no
    // `-Zbuild-std` is needed.
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--target", &host]);
    if release {
        cmd.arg("--release");
    }

    // Audio file bridges: passed to the simulator via env vars (read by
    // `run_in_process`). `--audio-in <wav>` feeds a WAV as the codec input;
    // `--audio-out <wav>` records the output. Paths are absolutised so they
    // resolve against the user's shell cwd, not cargo's package dir.
    if let Some(p) = flag_value(args, "--audio-in") {
        cmd.env("DELUGE_SIM_AUDIO_IN", absolute(&p));
    }
    if let Some(p) = flag_value(args, "--audio-out") {
        cmd.env("DELUGE_SIM_AUDIO_OUT", absolute(&p));
    }

    // Physical Deluge control surface: attach a real Deluge running
    // controller-firmware over USB-CDC as an additional panel for the in-process
    // brain. `--hardware <port>` names the serial device; bare `--hardware`
    // auto-detects by USB VID/PID. Read by `run_in_process`.
    if args.iter().any(|a| a == "--hardware" || a.starts_with("--hardware=")) {
        let port = flag_value(args, "--hardware").filter(|v| !v.starts_with('-'));
        cmd.env("DELUGE_SIM_HARDWARE", port.unwrap_or_else(|| "auto".to_string()));
    }

    // Headless mode: no window — replay a script and dump golden-frame snapshots.
    let headless = args.iter().any(|a| a == "--headless");
    if headless {
        cmd.env("DELUGE_HEADLESS", "1");
    }
    if let Some(p) = flag_value(args, "--script") {
        cmd.env("DELUGE_SIM_SCRIPT", absolute(&p));
    }
    if let Some(p) = flag_value(args, "--out") {
        cmd.env("DELUGE_SIM_OUT", absolute(&p));
    }

    if headless {
        println!("building for {host} and running headless…");
    } else {
        println!("building for {host} and launching the simulator…");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err("simulator exited with an error".to_string());
    }
    Ok(())
}

/// The value following `flag` in `args` (`--flag value` or `--flag=value`).
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix(flag) {
            if let Some(v) = v.strip_prefix('=') {
                return Some(v.to_string());
            }
            if v.is_empty() {
                return it.next().cloned();
            }
        }
    }
    None
}

/// Absolutise a path against the current dir so it's stable when cargo runs the
/// app from the package directory.
fn absolute(p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map(|d| d.join(&path)).unwrap_or(path)
    }
}

/// The host target triple (`rustc -vV` → `host:`), used to override the
/// workspace's embedded default target.
fn host_triple() -> Result<String, String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|e| format!("failed to run rustc: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_string)
        .ok_or_else(|| "could not determine host triple from `rustc -vV`".to_string())
}
