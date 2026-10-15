//! Puts the Deluge's memory layout on the linker search path as `memory.x`, for `rza1-rt`'s
//! `link.x` to INCLUDE: the RTT form (with the RTT_RAM regions) when the `rtt` feature is on.
//! The same feature turns on `rza1-rt/rtt`, which selects the matching form of `link.x`.
//! Without the default `memory-x` feature it publishes nothing, and the application supplies its
//! own `memory.x`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=memory_rtt.x");

    // Host builds (tests, the simulator) use the platform's own linker setup.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    if env::var_os("CARGO_FEATURE_MEMORY_X").is_none() {
        // OUT_DIR outlives a feature change: don't leave an earlier copy on the search path.
        let _ = fs::remove_file(out_dir.join("memory.x"));
        return;
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let layout = if env::var_os("CARGO_FEATURE_RTT").is_some() { "memory_rtt.x" } else { "memory.x" };

    fs::copy(manifest_dir.join(layout), out_dir.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
}
