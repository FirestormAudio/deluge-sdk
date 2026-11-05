use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // rza1-rt puts `link_xip.x` on the linker search path; it INCLUDEs `memory_xip.x`, which
    // is this crate's `memory_slot_a.x`. Host builds have no use for either.
    println!("cargo:rerun-if-changed=memory_slot_a.x");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        fs::copy("memory_slot_a.x", out_dir.join("memory_xip.x")).unwrap();
        println!("cargo:rustc-link-search={}", out_dir.display());
        println!("cargo:rustc-link-arg-bins=-Tlink_xip.x");
    }
}
