//! Build script for the wren firmware.
//!
//! Two jobs:
//!  1. Select the memory layout + linker script that match the `rtt` feature
//!     (same pattern as `demo-firmware`).
//!  2. Add the wren C VM to the link: the newlib (libc/libm) archives for the
//!     Cortex-A9 hard-float multilib, so the VM's libc references (string, math,
//!     snprintf for number formatting) resolve. These are passed as positional
//!     link args wrapped in a group because libc/libm/libnosys reference each
//!     other cyclically.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Compiler flags that select the Cortex-A9 hard-float multilib.
const MULTILIB_FLAGS: &[&str] = &["-mcpu=cortex-a9", "-mfloat-abi=hard", "-mfpu=neon-vfpv3"];

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // ── Memory layout / linker script (mirrors demo-firmware) ───────────────
    let rtt = env::var("CARGO_FEATURE_RTT").is_ok();
    let (memory_src, linker_script) = if rtt {
        ("memory_rtt.x", "rza1l_rtt.x")
    } else {
        ("memory.x", "rza1l.x")
    };
    fs::copy(manifest_dir.join(memory_src), out_dir.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg=-T{linker_script}");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=memory_rtt.x");

    // ── newlib (libc/libm) for crow's embedded Lua VM ───────────────────────
    // rust-lld is the linker (no override for armv7a-none-eabihf), so the
    // archives and group markers are passed straight through as lld args.
    // Note: libgcc is intentionally omitted — Rust's compiler-builtins already
    // provides the __aeabi_* integer-div/helper intrinsics the Lua C code needs
    // on the A9, and pulling libgcc drags in its ARM EH unwinder (references to
    // __exidx_start/_end that this bare-metal image doesn't define).
    // newlib-nano stubs out float in printf/sprintf by default; wren formats
    // every number with `sprintf("%.14g", …)`, so without this the VM prints
    // empty strings for numbers. `-u _printf_float` pulls newlib's float-capable
    // vfprintf variant. Must precede the libc group so it resolves from it.
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=_printf_float");

    println!("cargo:rustc-link-arg=--start-group");
    for lib in ["libc_nano.a", "libm.a", "libnosys.a"] {
        let path = newlib_archive(lib);
        println!("cargo:rustc-link-arg={path}");
    }
    println!("cargo:rustc-link-arg=--end-group");
}

/// Resolve the absolute path to a newlib/libgcc archive for the Cortex-A9
/// hard-float multilib via `arm-none-eabi-gcc -print-file-name`.
fn newlib_archive(name: &str) -> String {
    let out = Command::new("arm-none-eabi-gcc")
        .args(MULTILIB_FLAGS)
        .arg(format!("-print-file-name={name}"))
        .output()
        .unwrap_or_else(|e| panic!("failed to run arm-none-eabi-gcc: {e}"));
    let path = String::from_utf8(out.stdout).unwrap().trim().to_string();
    assert!(
        std::path::Path::new(&path).is_file(),
        "newlib archive {name} not found (got {path:?}); is the arm-none-eabi toolchain installed?"
    );
    path
}
