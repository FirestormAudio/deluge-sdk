//! Build script for the wren firmware.
//!
//! Two jobs:
//!  1. Select rza1-rt's `link.x` and raise the program stack for the VM.
//!  2. Add the wren C VM to the link: the newlib (libc/libm) archives for the
//!     Cortex-A9 hard-float multilib, so the VM's libc references (string, math,
//!     snprintf for number formatting) resolve. These are passed as positional
//!     link args wrapped in a group because libc/libm/libnosys reference each
//!     other cyclically.

use std::env;
use std::process::Command;

/// Compiler flags that select the Cortex-A9 hard-float multilib.
const MULTILIB_FLAGS: &[&str] = &["-mcpu=cortex-a9", "-mfloat-abi=hard", "-mfpu=neon-vfpv3"];

fn main() {
    // Host (simulator) builds link with the platform's default linker and libc,
    // so skip the embedded linker script and the bare-metal newlib archives
    // entirely — the host libc resolves the wren VM's libc references, and
    // wren-sys's host build doesn't pull newlib (see wren-sys/build.rs).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none") {
        return;
    }

    // ── Linker script and program stack ─────────────────────────────────────
    // The Deluge's memory.x (from deluge-bsp) PROVIDEs a 32 KB program stack. The wren VM's
    // compiler and interpreter recurse deeply on the executor's (program) stack, especially in
    // debug builds, so take 256 KB; with RTT, 192 KB, which still ends above the RTT reserve
    // (0x202B0000–0x202C0000). The --defsym must come before -Tlink.x: lld applies it in
    // command-line order, so after the script it would rename the symbol without moving the
    // stacks.
    // The stack size follows this crate's `rtt` feature, so turn RTT on through it
    // (`--features rtt`): RTT enabled only further down (`deluge-sdk/rtt`) keeps the 256 KB
    // stack, which link_rtt.x rejects as overlapping the RTT reserve.
    let program_stack = if env::var_os("CARGO_FEATURE_RTT").is_some() { "0x30000" } else { "0x40000" };
    println!("cargo:rustc-link-arg-bins=--defsym=PROGRAM_STACK_SIZE={program_stack}");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");

    // ── newlib (libc/libm) for the wren C VM ────────────────────────────────
    // rust-lld is the linker (no override for armv7a-none-eabihf), so the
    // archives and group markers are passed straight through as lld args.
    // Note: libgcc is intentionally omitted — Rust's compiler-builtins already
    // provides the __aeabi_* integer-div/helper intrinsics the wren C code needs
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
