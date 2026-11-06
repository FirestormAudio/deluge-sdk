//! Find libdeluge and link it: through pkg-config (`deluge`) when it answers, otherwise under
//! `DELUGE_SDK_ROOT` (default `/usr/local`). docs.rs has no libdeluge, so there the build only
//! compiles the checked-in bindings.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=DELUGE_SDK_ROOT");
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    println!("cargo:rerun-if-changed=wrapper.h");
    if std::env::var_os("DOCS_RS").is_some() {
        return;
    }

    let (include, lib_dirs) = match pkg_config::Config::new()
        .cargo_metadata(false)
        .env_metadata(true)
        .probe("deluge")
    {
        Ok(lib) => (
            lib.include_paths.first().cloned().unwrap_or_default(),
            lib.link_paths,
        ),
        Err(_) => {
            let root = PathBuf::from(
                std::env::var("DELUGE_SDK_ROOT").unwrap_or_else(|_| "/usr/local".into()),
            );
            (root.join("include"), vec![root.join("lib")])
        }
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
    println!("cargo:rustc-link-lib=static=deluge");
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
