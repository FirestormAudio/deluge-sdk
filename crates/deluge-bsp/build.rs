//! Puts the Deluge's memory layout on the linker search path as `memory.x`, for `rza1-rt`'s
//! `link.x` to INCLUDE: the RTT form (with the RTT_RAM regions) when `rza1-rt`'s `rtt` feature
//! is on, which is when its `link.x` is the RTT form too. This crate's `rtt` feature turns it on;
//! the choice follows `rza1-rt`'s report (DEP_RZA1_RT_RTT), so enabling `rza1-rt/rtt` directly
//! gets the matching layout as well.
//! Without the default `memory-x` feature it publishes nothing, and the application supplies its
//! own `memory.x`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=memory_rtt.x");
    println!("cargo:rerun-if-env-changed=DEP_RZA1_RT_RTT");

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
    let rtt = env::var("DEP_RZA1_RT_RTT").as_deref() == Ok("1");
    let layout = if rtt { "memory_rtt.x" } else { "memory.x" };

    fs::copy(manifest_dir.join(layout), out_dir.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
}
