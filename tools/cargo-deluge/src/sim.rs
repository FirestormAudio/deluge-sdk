//! `cargo deluge sim`: build the current app for the **host** and run it in the
//! desktop simulator.
//!
//! Unlike every other subcommand, this does not cross-compile to the Deluge: it
//! builds the app for the host triple, where the SDK's host backend swaps the
//! real peripherals for the in-process simulator panel (OLED/pads/LEDs/audio).
//! The app binary opens the simulator window itself, so this just runs it and
//! streams its output.

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

    println!("building for {host} and launching the simulator…");
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err("simulator exited with an error".to_string());
    }
    Ok(())
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
