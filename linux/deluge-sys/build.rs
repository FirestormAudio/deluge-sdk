fn main() {
    // DELUGE_SDK_ROOT points at the installed prefix (headers + lib).
    let root = std::env::var("DELUGE_SDK_ROOT").unwrap_or_else(|_| "/usr/local".into());
    println!("cargo:rerun-if-env-changed=DELUGE_SDK_ROOT");
    println!("cargo:rerun-if-changed=wrapper.h");
    // Watch the archive itself, not just the headers. `libdeluge.a` lives
    // outside the cargo tree (it comes from the deluge-ndk build, via the
    // bundle sysroot), and an implementation-only change leaves the headers
    // untouched, so without this cargo keeps linking the stale library.
    println!("cargo:rerun-if-changed={root}/lib/libdeluge.a");
    println!("cargo:rustc-link-lib=deluge");
    // libdeluge is a static archive (libdeluge.a); its ALSA symbol references
    // are not resolved transitively, so link libasound explicitly (mirrors
    // CMakeLists.txt: target_link_libraries(deluge PUBLIC ${ALSA_LIBRARIES} ...)).
    println!("cargo:rustc-link-lib=asound");
    println!("cargo:rustc-link-search=native={root}/lib");
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{root}/include"))
        .allowlist_function("deluge_.*")
        .allowlist_type("deluge_.*")
        .generate()
        .expect("bindgen");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("bindings.rs")).unwrap();
}
