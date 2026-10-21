//! Build script for the `additive_osc` example.
//!
//! Two jobs:
//!
//! 1. **Compile the C++/Argon DSP core** (`csrc/additive.cpp`) into a static
//!    archive linked into the app, on *both* targets:
//!      - the **device** (`target_os = "none"`) with the bare ARM toolchain
//!        (`arm-none-eabi-g++`, cortex-a9 hard-float NEON), matching the
//!        firmware ABI — Argon auto-selects its NEON backend via `__ARM_NEON`;
//!      - the **host** (the desktop simulator) with the default host `g++`,
//!        where Argon transparently falls back to SIMDe.
//!
//!    Argon is header-only; it is located via the `ARGON_SRC` env var, defaulting
//!    to the sibling `../../../../argon` checkout (mirrors deluge-wren's `wren-sys` `WREN_SRC`).
//!    The C++ is built libm-free and without exceptions/RTTI so it links against
//!    the firmware's newlib-libc-only runtime (see `csrc/additive.cpp`).
//!
//! 2. **Device linker setup** — select rza1-rt's `link.x` (the same as the other examples).

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // `CARGO_CFG_TARGET_OS` is "none" only for the embedded firmware triple; the
    // desktop simulator and the Linux-userland backend both build for hosted
    // triples.
    let embedded = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none");
    // Detect NEON from the effective target features (rustflags included)
    // rather than `target_arch = "arm"`, which also covers NEON-less cores.
    // The workspace `.cargo/config.toml` enables `+neon` for the Linux ARM
    // target (`armv7-unknown-linux-musleabihf`), matching the device build.
    let arm = env::var("CARGO_CFG_TARGET_FEATURE")
        .map(|f| f.split(',').any(|feat| feat == "neon"))
        .unwrap_or(false);

    build_dsp_core(&manifest_dir, embedded, arm);

    if embedded {
        link_firmware();
    }
}

/// Compile `csrc/additive.cpp` (the C++/Argon additive engine) into the app.
fn build_dsp_core(manifest_dir: &Path, embedded: bool, arm: bool) {
    // Locate the header-only Argon library.
    let argon = env::var("ARGON_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("../../../../argon"));
    let argon_include = argon.join("include");
    assert!(
        argon_include.join("argon.hpp").exists(),
        "Argon headers not found at {} — set ARGON_SRC to your argon checkout \
         (e.g. a sibling clone of https://github.com/stellar-aria/argon)",
        argon_include.display()
    );

    let source = manifest_dir.join("csrc/additive.cpp");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++23")
        .file(&source)
        .include(&argon_include)
        // Argon is template-heavy but throws nothing; keep the archive free of
        // exception/RTTI machinery and libstdc++ so it links into the no_std app.
        .flag("-fno-exceptions")
        .flag("-fno-rtti")
        .cpp_link_stdlib(None)
        .opt_level(2)
        .warnings(false);

    if embedded {
        // Bare-metal cortex-a9 hard-float NEON, matching the Rust target ABI
        // (same toolchain/flags recipe as deluge-wren's `wren-sys`, g++ instead of gcc). Argon
        // selects its NEON backend automatically from `__ARM_NEON`.
        build
            .compiler("arm-none-eabi-g++")
            .archiver("arm-none-eabi-ar")
            .flag("-mcpu=cortex-a9")
            .flag("-mfloat-abi=hard")
            .flag("-mfpu=neon-vfpv3")
            .flag("-ffunction-sections")
            .flag("-fdata-sections")
            .flag("-fno-unwind-tables")
            .flag("-fno-asynchronous-unwind-tables")
            .flag("-fno-common");
    } else if arm {
        // Linux-userland backend (`armv7-unknown-linux-musleabihf`): hardware
        // NEON via the bundle's cross toolchain, which `cc` finds from the
        // target triple. No SIMDe here: `-I /usr/include/simde` trips GCC's
        // "unsafe header path used in cross-compilation" guard. The
        // toolchain's default FPU for this target is `vfpv3-d16` (no NEON, so
        // `__ARM_NEON` is undefined and Argon finds no backend); select the
        // same FPU as the bare-metal branch instead.
        build.flag("-mfpu=neon-vfpv3");
    } else {
        // Host (simulator): no `__ARM_NEON`, so Argon includes SIMDe's
        // `<arm/neon.h>` (and self-defines `SIMDE_ENABLE_NATIVE_ALIASES`). SIMDe
        // is system-installed at /usr/include/simde here; allow an override via
        // `SIMDE_SRC` for other hosts.
        let simde = env::var("SIMDE_SRC").unwrap_or_else(|_| "/usr/include/simde".to_string());
        build.include(&simde);
    }

    build.compile("additive");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-env-changed=ARGON_SRC");
    println!("cargo:rerun-if-env-changed=SIMDE_SRC");
}

/// Device-only: select rza1-rt's `link.x` (deluge-bsp puts the Deluge's `memory.x` next to it).
fn link_firmware() {
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
}
