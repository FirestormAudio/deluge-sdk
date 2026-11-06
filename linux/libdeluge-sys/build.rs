//! Find libdeluge and link it: under `DELUGE_SDK_ROOT` when it is set (`cargo deluge linux` points
//! it at the bundle's sysroot), otherwise through pkg-config (`deluge`), otherwise under
//! `/usr/local`. docs.rs has no libdeluge, so there the build only compiles the checked-in
//! bindings.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=DELUGE_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    println!("cargo:rerun-if-changed=wrapper.h");
    if std::env::var_os("DOCS_RS").is_some() {
        return;
    }

    let under = |root: PathBuf| (root.join("include"), vec![root.join("lib")]);
    let (include, lib_dirs) = match std::env::var_os("DELUGE_SDK_ROOT") {
        Some(root) => under(PathBuf::from(root)),
        None => match pkg_config::Config::new()
            .cargo_metadata(false)
            .env_metadata(true)
            .probe("deluge")
        {
            Ok(lib) => (
                lib.include_paths.first().cloned().unwrap_or_default(),
                lib.link_paths,
            ),
            Err(_) => under(PathBuf::from("/usr/local")),
        },
    };

    for dir in &lib_dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
        // `libdeluge.a` comes from outside the cargo tree, and an implementation-only change
        // leaves the headers untouched: watch the archive so cargo relinks.
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("libdeluge.a").display()
        );
    }
    // Static when an archive is there (the bundle ships `libdeluge.a`); a pkg-config install may
    // have only the shared library.
    let has_archive = lib_dirs.iter().any(|d| d.join("libdeluge.a").is_file());
    let kind = if has_archive { "static" } else { "dylib" };
    println!("cargo:rustc-link-lib={kind}=deluge");
    // The static archive's ALSA references are not resolved transitively.
    println!("cargo:rustc-link-lib=asound");
    println!("cargo:include={}", include.display());

    #[cfg(feature = "bindgen")]
    generate(&include);
}

#[cfg(feature = "bindgen")]
fn generate(include: &std::path::Path) {
    let target = std::env::var("TARGET").unwrap();
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg(format!("--target={target}"))
        .allowlist_function("deluge_.*")
        .allowlist_type("deluge_.*")
        .layout_tests(false)
        .generate()
        .expect("bindgen");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings.write_to_file(&out).unwrap();
    // `LIBDELUGE_SYS_UPDATE=1` also refreshes the checked-in copy.
    println!("cargo:rerun-if-env-changed=LIBDELUGE_SYS_UPDATE");
    if std::env::var_os("LIBDELUGE_SYS_UPDATE").is_some() {
        let src =
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("src/bindings.rs");
        std::fs::write(src, std::fs::read_to_string(&out).unwrap()).unwrap();
    }
}
