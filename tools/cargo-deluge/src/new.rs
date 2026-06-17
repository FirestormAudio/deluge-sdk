//! `cargo deluge new`: scaffold a new Deluge app crate from baked-in templates.

use std::env;
use std::path::{Path, PathBuf};

use crate::util::write;

// Canonical project files, baked in from the repo so generated apps can't drift
// from the SDK's own build setup.
const TPL_BUILD_RS: &str = include_str!("../../../examples/blinky/build.rs");
const TPL_MEMORY_X: &str = include_str!("../../../examples/blinky/memory.x");
const TPL_MEMORY_RTT_X: &str = include_str!("../../../examples/blinky/memory_rtt.x");
const TPL_TOOLCHAIN: &str = include_str!("../../../rust-toolchain.toml");

pub(crate) fn cmd_new(args: &[String]) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TARGET;
    use crate::build::parse_package_name;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

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
