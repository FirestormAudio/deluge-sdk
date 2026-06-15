#!/usr/bin/env bash
#
# Canonical test runner for the deluge-embassy workspace.
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
#
# The qemu-arm runner + cross linker are configured in .cargo/config.toml.
#
# Usage: tools/test.sh
set -euo pipefail

cd "$(dirname "$0")/.."

QEMU=armv7-unknown-linux-gnueabihf
HOST=x86_64-unknown-linux-gnu

echo "==> QEMU ARM bucket ($QEMU)"
cargo test --target "$QEMU" -p fixedpoint --lib
cargo test --target "$QEMU" -p armv7-dsp-intrinsics --features nightly --lib
# No --lib: also runs the cross-crate dsp_pipeline integration test.
cargo test --target "$QEMU" -p deluge-fft --features test-utils
cargo test --target "$QEMU" -p rza1l-hal --lib
cargo test --target "$QEMU" -p deluge-bsp --lib
cargo test --target "$QEMU" -p embedded-fonts-deluge --lib

echo "==> Host bucket ($HOST)"
cargo test --target "$HOST" -p deluge-image
cargo test --target "$HOST" -p deluge-ui-toolkit
cargo test --target "$HOST" -p deluge-macros
cargo test --target "$HOST" --manifest-path tools/elf2uf2/Cargo.toml
cargo test --target "$HOST" --manifest-path tools/cargo-deluge/Cargo.toml

echo "==> All tests passed."
