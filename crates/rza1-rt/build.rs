//! Puts the linker scripts on the linker search path; a binary selects one with `-T`, and the
//! script INCLUDEs the board's `memory.x`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    for script in ["rza1l.x", "rza1l_rtt.x"] {
        fs::copy(manifest_dir.join(script), out_dir.join(script)).unwrap();
        println!("cargo:rerun-if-changed={script}");
    }
    println!("cargo:rustc-link-search={}", out_dir.display());
}
