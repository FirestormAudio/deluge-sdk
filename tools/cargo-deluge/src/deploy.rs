//! `cargo deluge deploy`: build, then copy the ELF onto a mounted Deluge SD card.

use std::fs;
use std::path::Path;

use crate::build::cmd_build;
use crate::util::arg_value;

/// Build, then copy the ELF into `<dest>/APPS/<name>.elf` on a mounted Deluge SD
/// card. With no `--dest`, print how to deploy it by hand.
pub(crate) fn cmd_deploy(args: &[String]) -> Result<(), String> {
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
