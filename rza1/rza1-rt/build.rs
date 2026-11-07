//! Puts `link.x` on the linker search path: the RTT form when the `rtt` feature is on. A binary
//! links with `-Tlink.x`, and the script INCLUDEs the board's `memory.x` and the zero-init data
//! (`zero_init.x`), after the image or, with `zero-init-below-image`, before it. Also puts `link_xip.x`
//! there, for an image that executes in place from flash (`-Tlink_xip.x`, INCLUDEs `memory_xip.x`).

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=link.x");
    println!("cargo:rerun-if-changed=link_rtt.x");
    println!("cargo:rerun-if-changed=link_xip.x");
    println!("cargo:rerun-if-changed=zero_init.x");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rtt = env::var_os("CARGO_FEATURE_RTT").is_some();
    let script = if rtt { "link_rtt.x" } else { "link.x" };
    // `links` metadata: a board crate's build script reads it as DEP_RZA1_RT_RTT and publishes
    // the matching memory.x, so the script and the layout can't disagree.
    println!("cargo:rtt={}", u8::from(rtt));

    fs::copy(manifest_dir.join(script), out_dir.join("link.x")).unwrap();
    // link.x INCLUDEs both names; the zero-init data goes in one and the other is left empty.
    let zero_init = fs::read_to_string(manifest_dir.join("zero_init.x")).unwrap();
    let below = env::var_os("CARGO_FEATURE_ZERO_INIT_BELOW_IMAGE").is_some();
    let (here, empty) = if below {
        ("zero_init_below_image.x", "zero_init_after_image.x")
    } else {
        ("zero_init_after_image.x", "zero_init_below_image.x")
    };
    fs::write(out_dir.join(here), zero_init).unwrap();
    fs::write(out_dir.join(empty), "").unwrap();
    // The execute-in-place script, selected with `-Tlink_xip.x`; it INCLUDEs memory_xip.x.
    fs::copy(manifest_dir.join("link_xip.x"), out_dir.join("link_xip.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
}
