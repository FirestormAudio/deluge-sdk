fn main() {
    // rza1-rt puts `link.x` on the linker search path and deluge-bsp puts the Deluge's
    // `memory.x` next to it; this selects the script. Host (simulator) builds use the
    // platform's own linker setup.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
        // The stock first-stage bootloader runs from the bottom of SRAM while it copies this
        // image there; stock_fsb.x fails the link if the image starts on top of it.
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-search={dir}");
        println!("cargo:rustc-link-arg-bins=-Tstock_fsb.x");
        println!("cargo:rerun-if-changed=stock_fsb.x");
    }
}
