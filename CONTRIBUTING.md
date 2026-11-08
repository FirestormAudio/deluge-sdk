# Contributing to the Deluge SDK

Thanks for your interest in hacking on the SDK! This document covers how to
build, test, and submit changes.

## Getting set up

The toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml) — one dated
nightly plus the `armv7a-none-eabihf` target. `rustup show` from the
repo root installs both.

To move to a newer nightly, change the date in `rust-toolchain.toml`, `rza1/rust-toolchain.toml`,
`tools/cargo-deluge/templates/rust-toolchain.toml` and every `toolchain:` line under
`.github/workflows/` together; `tools/test.sh` fails if they disagree. Run the full suite on the
new nightly before committing.

```sh
rustup show
cargo install cargo-binutils
cargo install --target x86_64-unknown-linux-gnu --path tools/cargo-deluge   # the `cargo deluge` host subcommand
cargo build-app-loader-bin
```

To build firmware images and examples, use the build aliases defined in
`.cargo/config.toml` (they pass the required `-Zbuild-std` flags) rather than a
bare `cargo build`. See the [README](README.md#working-on-the-sdk-itself) and
[Advanced developer guide](docs/advanced-guide.md) for the full workflow.

## Running the tests

The default target is bare-metal, so tests run on two host-side targets (QEMU
ARM for crates using ARM asm/intrinsics, and the host triple for pure-logic
crates). Run the whole suite with:

```sh
./tools/test.sh
```

One-time setup (rustup targets + QEMU + the ARM cross-linker) is documented at
the top of `tools/test.sh` and in the README.

CI (`.github/workflows/ci.yml`) additionally compile-proves every example on the
firmware target. Please make sure `./tools/test.sh` passes and any example you
touch still builds (`./tools/build-examples.sh`) before opening a PR.

## Submitting changes

1. Branch off `main`.
2. Keep commits focused; write a clear commit message explaining the *why*.
3. Run `cargo fmt` and `./tools/test.sh`.
4. Open a pull request describing the change and how you verified it.

## Releases

A release is a `v*` tag on `main`. The tag, without the `v`, must start with the workspace
`version` in [`Cargo.toml`](Cargo.toml) (`v0.1.0`, or `v0.1.0-preview.1` for a
pre-release), and `CHANGELOG.md` must have a matching `## [0.1.0-preview.1] - <date>`
section. Pushing the tag runs the [release workflow](.github/workflows/release.yml),
which builds the images and publishes a GitHub Release with `app-loader.bin`, its
`SHA256SUMS`, and that changelog section as the notes. A tag with a `-` suffix is
published as a pre-release.

## Licensing of contributions

This repository is **dual-licensed**, and which license applies depends on the
crate you are touching:

- The SDK and core libraries (`crates/deluge-sdk`, `deluge-bsp`, `rza1-hal`,
  `deluge-image`, `deluge-sdk-macros`, `deluge-fixedpoint`,
  `arm-dsp`, the firmwares and examples) are **`MIT OR Apache-2.0`**.
- The OLED UI toolkit (`crates/deluge-ui-toolkit`), its fonts
  (`crates/deluge-fonts`) and the pad-grid toolkit
  (`crates/deluge-grid-toolkit`) are **`GPL-3.0-or-later`**.

Unless you state otherwise, contributions you submit to a given crate are
understood to be offered under that crate's existing license(s). Don't copy code
from a GPL crate into a permissively-licensed one (or vice-versa) — the
permissive `deluge` facade deliberately does **not** depend on the GPL toolkit.

By contributing, you certify that you have the right to submit the work under the
applicable license.
