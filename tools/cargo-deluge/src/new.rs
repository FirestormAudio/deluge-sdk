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
// J-Link bring-up script reused from the SDK root (referenced by launch.json).
const TPL_JLINK: &str = include_str!("../../../rza1_debug.JLinkScript");

// Static project + editor templates, kept as real files under `templates/` so
// they are edited as files rather than as Rust string literals.
const TPL_MAIN: &str = include_str!("../templates/src/main.rs");
const TPL_CARGO_CONFIG: &str = include_str!("../templates/cargo/config.toml");
const TPL_GITIGNORE: &str = include_str!("../templates/gitignore");
const TPL_VSCODE_EXTENSIONS: &str = include_str!("../templates/vscode/extensions.json");
const TPL_VSCODE_SETTINGS: &str = include_str!("../templates/vscode/settings.json");
const TPL_VSCODE_TASKS: &str = include_str!("../templates/vscode/tasks.json");
// `__APP_NAME__` is substituted with the app name (the debug ELF path).
const TPL_VSCODE_LAUNCH: &str = include_str!("../templates/vscode/launch.json");

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
    write(&dir.join("Cargo.toml"), &cargo_toml(name, deluge_dep))?;
    write(&dir.join("rust-toolchain.toml"), TPL_TOOLCHAIN)?;
    write(&dir.join(".cargo/config.toml"), TPL_CARGO_CONFIG)?;
    write(&dir.join("build.rs"), TPL_BUILD_RS)?;
    write(&dir.join("memory.x"), TPL_MEMORY_X)?;
    write(&dir.join("memory_rtt.x"), TPL_MEMORY_RTT_X)?;
    write(&dir.join("src/main.rs"), TPL_MAIN)?;
    write(&dir.join(".gitignore"), TPL_GITIGNORE)?;

    // VS Code integration: recommended extensions, rust-analyzer set up for the
    // bare-metal target, `cargo deluge` tasks, and a J-Link debug config. The
    // launch config loads `rza1_debug.JLinkScript`, scaffolded alongside, and
    // points at the app's debug ELF (hence the name substitution).
    write(&dir.join(".vscode/extensions.json"), TPL_VSCODE_EXTENSIONS)?;
    write(&dir.join(".vscode/settings.json"), TPL_VSCODE_SETTINGS)?;
    write(&dir.join(".vscode/tasks.json"), TPL_VSCODE_TASKS)?;
    write(
        &dir.join(".vscode/launch.json"),
        &TPL_VSCODE_LAUNCH.replace("__APP_NAME__", name),
    )?;
    write(&dir.join("rza1_debug.JLinkScript"), TPL_JLINK)?;
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
        assert!(TPL_MAIN.contains("#![no_std]"));
        assert!(TPL_MAIN.contains("#![no_main]"));
        assert!(TPL_MAIN.contains("#[deluge::app]"));
        assert!(TPL_MAIN.contains("async fn main"));
    }

    #[test]
    fn baked_in_templates_nonempty() {
        assert!(!TPL_BUILD_RS.is_empty());
        assert!(!TPL_MEMORY_X.is_empty());
        assert!(!TPL_MEMORY_RTT_X.is_empty());
        assert!(!TPL_TOOLCHAIN.is_empty());
        assert!(TPL_TOOLCHAIN.contains("armv7a-none-eabihf"));
        assert!(TPL_CARGO_CONFIG.contains("build-std"));
        assert!(!TPL_JLINK.is_empty());
        assert_eq!(TARGET, "armv7a-none-eabihf");
    }

    #[test]
    fn vscode_templates_are_valid_and_unsubstituted() {
        // The four .vscode files ship non-empty.
        for t in [
            TPL_VSCODE_EXTENSIONS,
            TPL_VSCODE_SETTINGS,
            TPL_VSCODE_TASKS,
            TPL_VSCODE_LAUNCH,
        ] {
            assert!(!t.is_empty());
        }
        // Only launch.json carries the app-name placeholder; it must still be
        // present in the template (substituted at scaffold time).
        assert!(TPL_VSCODE_LAUNCH.contains("__APP_NAME__"));
        assert!(!TPL_VSCODE_EXTENSIONS.contains("__APP_NAME__"));
        // The default build task is the USB run command.
        assert!(TPL_VSCODE_TASKS.contains("cargo deluge run"));
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
            ".vscode/extensions.json",
            ".vscode/settings.json",
            ".vscode/tasks.json",
            ".vscode/launch.json",
            "rza1_debug.JLinkScript",
        ] {
            assert!(app.join(f).is_file(), "missing scaffolded file: {f}");
        }

        let cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
        assert_eq!(parse_package_name(&cargo).as_deref(), Some("my-app"));
        assert!(fs::read_to_string(app.join(".gitignore")).unwrap().contains("/target"));

        // launch.json must have the app name substituted in (the debug ELF path)
        // and no leftover placeholder.
        let launch = fs::read_to_string(app.join(".vscode/launch.json")).unwrap();
        assert!(launch.contains("debug/my-app"), "app name not substituted: {launch}");
        assert!(!launch.contains("__APP_NAME__"), "placeholder left in launch.json");
    }
}
