#!/usr/bin/env bash
#
# Canonical test runner for the deluge-sdk workspace.
#
# Tests fall into two buckets, split by hard toolchain constraints:
#
#   QEMU ARM bucket (armv7-unknown-linux-gnueabihf, run under qemu-arm):
#     crates that use ARM inline asm or ARM/NEON intrinsics. They cannot build
#     for the host triple, and cannot use the test harness on the bare-metal
#     `armv7a-none-eabihf` firmware target (which has no std).
#
#   Host bucket (x86_64-unknown-linux-gnu):
#     pure-logic / std crates, and crates whose dev-deps don't cross-compile to
#     ARM (e.g. deluge-ui-toolkit's embedded-graphics-simulator pulls in
#     `image` -> `simd-adler32`, which needs unstable NEON on armv7).
#
# Prerequisites (Arch: pacman; Debian/Ubuntu: apt):
#   - rustup targets: armv7-unknown-linux-gnueabihf, x86_64-unknown-linux-gnu
#   - qemu-user (provides qemu-arm)            [apt: qemu-user]
#   - arm-linux-gnueabihf-gcc (cross linker)   [apt: gcc-arm-linux-gnueabihf]
#   - libudev (serialport, via cargo-deluge)   [apt: libudev-dev; pacman: systemd]
#
# The qemu-arm runner + cross linker are configured in .cargo/config.toml.
#
# Usage: tools/test.sh
set -euo pipefail

cd "$(dirname "$0")/.."

tools/check-layering.sh

echo "==> one pinned nightly: the toolchain files, the app template and every CI workflow agree"
pinned="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
for f in rza1/rust-toolchain.toml tools/cargo-deluge/templates/rust-toolchain.toml; do
  if [ "$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$f")" != "$pinned" ]; then
    echo "error: $f does not name $pinned" >&2
    exit 1
  fi
done
if grep -n 'toolchain:' .github/workflows/*.yml | grep -v "toolchain: $pinned\$"; then
  echo "error: a workflow installs a toolchain other than $pinned" >&2
  exit 1
fi

echo "==> rustfmt-clean (the workspace and the crates it excludes; rza1/ checks its own)"
cargo fmt --all -- --check
for d in tools/cargo-deluge crates/deluge-simulator linux/libdeluge-sys \
  linux/libdeluge linux/deluge-linux-ui examples/linux/rust-app examples/linux/snake \
  examples/linux/terminal; do
  (cd "$d" && cargo fmt -- --check)
done

QEMU=armv7-unknown-linux-gnueabihf
HOST=x86_64-unknown-linux-gnu

echo "==> QEMU ARM bucket ($QEMU)"
# arm-dsp has three code paths; firmware ships the raw-`asm!` one
# (no `nightly` feature), so test BOTH it and the `core::arch` intrinsic path
# under QEMU. The portable fallback is covered in the host bucket below.
cargo test --target "$QEMU" -p arm-dsp --lib
cargo test --target "$QEMU" -p arm-dsp --features nightly --lib
# The inline asm must also assemble as Thumb-2 for Cortex-M: Cortex-M4/M7, and Cortex-M33
# with its DSP extension.
cargo build --target thumbv7em-none-eabihf -p arm-dsp
RUSTFLAGS="-C target-cpu=cortex-m33" cargo build --target thumbv8m.main-none-eabihf -p arm-dsp
# No --lib: also runs the cross-crate dsp_pipeline integration test.
cargo test --target "$QEMU" -p deluge-fixedpoint
# The chip crates are their own workspace; their suite (tests, PAC checks, register-table check)
# runs from rza1/tools/test.sh, which CI also runs on its own.
rza1/tools/test.sh
cargo test --target "$QEMU" -p deluge-bsp --features usb-host-uac,usb-host-overbridge,flash --lib
cargo test --target "$QEMU" -p deluge-usb-classes --features device-audio,device-midi,host-midi,host-uac,host-overbridge,device-overbridge --lib
cargo test --target "$QEMU" -p deluge-fonts --lib

echo "==> the vendored async FAT stack builds for the device, LFN on, no alloc"
cargo check --manifest-path crates/embedded-fatfs/Cargo.toml --target armv7a-none-eabihf \
  --no-default-features --features lfn
cargo check --manifest-path crates/block-device-adapters/Cargo.toml --target armv7a-none-eabihf
echo "==> the FAT forks build for a bare-metal target at their default features"
cargo build --manifest-path crates/embedded-fatfs/Cargo.toml --target thumbv7em-none-eabihf
cargo build --manifest-path crates/block-device-adapters/Cargo.toml --target thumbv7em-none-eabihf \
  --features log,defmt

echo "==> Host bucket ($HOST)"
# Portable-fallback / non-NEON paths of the fixed-point crates (the QEMU bucket
# above covers the ARM asm + intrinsic paths).
cargo test --target "$HOST" -p arm-dsp
cargo test --target "$HOST" -p deluge-fixedpoint
cargo test --target "$HOST" -p deluge-image
echo "==> the flash protection core (host)"
cargo test --target "$QEMU" -p deluge-flash-protect
cargo test --target "$HOST" -p deluge-ui-toolkit
cargo test --target "$HOST" -p deluge-sdk-macros
# The panel wire protocol, against the golden vectors the web simulator is also pinned to.
cargo test --target "$HOST" -p deluge-panel-protocol --features alloc
cargo check --target armv7a-none-eabihf -p deluge-panel-protocol
cargo test --target "$HOST" --manifest-path crates/deluge-simulator/Cargo.toml
# The SDK facade: `adapt_block` (the linux backend's frame adaptation) is pure
# logic and testable here. Needs an explicit backend feature — `sim` is the only
# one that builds on the host (`linux` requires the musl libdeluge sysroot).
# Host-only: `--features sim` pulls deluge-simulator -> cpal -> alsa-sys, which
# does not cross-compile to the QEMU ARM bucket's target.
cargo test --target "$HOST" -p deluge-sdk --features sim
# An app that turns on `deluge-sdk/sim` but declares no `sim` feature of its own still gets a
# host `main` from `#[deluge::app]`.
cargo build --target "$HOST" --manifest-path crates/deluge-sdk/tests/host-app/Cargo.toml \
  --target-dir target/host-app
cargo test --target "$HOST" --manifest-path tools/cargo-deluge/Cargo.toml
# libdeluge-sys builds from its checked-in bindings with no libdeluge present, as on docs.rs. On
# the host: CI has no musl target, and the bindings compile for either.
DOCS_RS=1 cargo doc --no-deps --target "$HOST" \
  --manifest-path linux/libdeluge-sys/Cargo.toml --target-dir target/libdeluge-docs
tools/test-libdeluge-sys-root.sh

echo "==> All tests passed."
