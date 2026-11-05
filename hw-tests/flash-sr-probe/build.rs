fn main() {
    // rza1-rt puts `link.x` on the linker search path and deluge-bsp puts the Deluge's
    // `memory.x` next to it; this selects the script.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
    }
    // The mode is chosen at build time (see `MODE` in main.rs).
    println!("cargo:rerun-if-env-changed=FLASH_SR_PROBE_MODE");
}
