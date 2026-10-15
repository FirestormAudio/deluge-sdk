//! Puts `link.x` on the linker search path: the RTT form when the `rtt` feature is on. A binary
//! links with `-Tlink.x`, and the script INCLUDEs the board's `memory.x`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=link.x");
    println!("cargo:rerun-if-changed=link_rtt.x");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rtt = env::var_os("CARGO_FEATURE_RTT").is_some();
    let script = if rtt { "link_rtt.x" } else { "link.x" };
    // `links` metadata: a board crate's build script reads it as DEP_RZA1_RT_RTT and publishes
    // the matching memory.x, so the script and the layout can't disagree.
    println!("cargo:rtt={}", u8::from(rtt));

    fs::copy(manifest_dir.join(script), out_dir.join("link.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
}
