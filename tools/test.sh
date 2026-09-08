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

QEMU=armv7-unknown-linux-gnueabihf
HOST=x86_64-unknown-linux-gnu

echo "==> QEMU ARM bucket ($QEMU)"
# armv7-dsp-intrinsics has three code paths; firmware ships the raw-`asm!` one
# (no `nightly` feature), so test BOTH it and the `core::arch` intrinsic path
# under QEMU. The portable fallback is covered in the host bucket below.
cargo test --target "$QEMU" -p armv7-dsp-intrinsics --lib
cargo test --target "$QEMU" -p armv7-dsp-intrinsics --features nightly --lib
cargo test --target "$QEMU" -p deluge-fixedpoint --lib
# No --lib: also runs the cross-crate dsp_pipeline integration test.
cargo test --target "$QEMU" -p deluge-fft --features test-utils
cargo test --target "$QEMU" -p rza1l-hal --lib
cargo test --target "$QEMU" -p deluge-bsp --features usb-host --lib
cargo test --target "$QEMU" -p deluge-fonts --lib
# The audio engine. On 32-bit ARM specifically because the deploy target is
# 32-bit (`usize == u32`) and these crates are full of offset arithmetic —
# output-arena rows, pool chunks, the scheduled-command queue, wire-record
# offsets. An overflow there panics on device and CANNOT be reproduced on the
# 64-bit host bucket below, so both buckets are load-bearing, not redundant.
cargo test --target "$QEMU" -p deluge-dsp-kernels
cargo test --target "$QEMU" -p deluge-audio-graph
cargo test --target "$QEMU" -p deluge-wren-core
# Again with NEON. `simd` is default-OFF, so the runs above are the scalar
# oracle and these are the f32x8 fast path. Both are required: the kernels'
# scalar==simd equivalence tests only mean something if the simd config is
# actually built, and it was previously built by nothing at all.
cargo test --target "$QEMU" -p deluge-dsp-kernels --features simd
cargo test --target "$QEMU" -p deluge-audio-graph --features simd

echo "==> Host bucket ($HOST)"
# Portable-fallback / non-NEON paths of the DSP crates (the QEMU bucket above
# covers the ARM asm + intrinsic paths). deluge-fft here also runs the rustfft
# external-oracle test, which is gated to non-ARM.
cargo test --target "$HOST" -p armv7-dsp-intrinsics
cargo test --target "$HOST" -p deluge-fixedpoint
cargo test --target "$HOST" -p deluge-fft --features test-utils
cargo test --target "$HOST" -p deluge-image
cargo test --target "$HOST" -p deluge-ui-toolkit
cargo test --target "$HOST" -p deluge-sdk-macros
# The audio engine again, on the host: the kernels' portable (non-NEON) paths,
# and the scalar-oracle comparisons that only run off ARM.
cargo test --target "$HOST" -p deluge-dsp-kernels
cargo test --target "$HOST" -p deluge-audio-graph
cargo test --target "$HOST" -p deluge-wren-core
# The portable-SIMD path off ARM: `core::simd` lowers to SSE/AVX here, so this
# also proves the f32x8 code is not secretly NEON-specific.
cargo test --target "$HOST" -p deluge-dsp-kernels --features simd
cargo test --target "$HOST" -p deluge-audio-graph --features simd
# The SDK facade: `adapt_block` (the linux backend's frame adaptation) is pure
# logic and testable here. Needs an explicit backend feature — `sim` is the only
# one that builds on the host (`linux` requires the musl libdeluge sysroot).
# Host-only: `--features sim` pulls deluge-simulator -> cpal -> alsa-sys, which
# does not cross-compile to the QEMU ARM bucket's target.
cargo test --target "$HOST" -p deluge-sdk --features sim
cargo test --target "$HOST" --manifest-path tools/cargo-deluge/Cargo.toml
cargo test --target "$HOST" --manifest-path tools/wren-web-debug/Cargo.toml

# `tools/wren-web` is the web simulator's wasm core. It is excluded from the
# workspace and only builds for wasm, so nothing else here compiles it — which
# is exactly how it silently rotted through several `Cmd`-surface changes before
# anyone noticed. Its logic (the `Cmd` wire codec) lives in deluge-wren-core so
# it is unit-tested above; this is the build guard that catches API drift.
if rustup target list --installed | grep -q '^wasm32-wasip1$'; then
  cargo check --manifest-path tools/wren-web/Cargo.toml --target wasm32-wasip1
else
  echo "  (skipped: wasm32-wasip1 target not installed)"
fi

echo "==> All tests passed."
