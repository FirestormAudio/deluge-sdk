//! `cargo deluge debug` / `trace`: thin wrappers over the Cortex-A9 `probe-rs`
//! fork (J-Link).

use std::process::Command;

use crate::CHIP;
use crate::build::cmd_build;
use crate::util::arg_value;

pub(crate) fn cmd_debug(args: &[String]) -> Result<(), String> {
    let elf = cmd_build(args)?;
    let mut cmd = Command::new("probe-rs");
    cmd.args(["run", "--chip", CHIP]);
    cmd.arg(&elf);
    cmd.args(passthrough_args(args));
    run_probe_rs(cmd)
}

pub(crate) fn cmd_trace(args: &[String]) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
