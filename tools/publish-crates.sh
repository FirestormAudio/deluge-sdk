#!/usr/bin/env bash
#
# Publishes the SDK's crates to crates.io in dependency order. Without arguments it is a dry
# run: every crate is packaged and, where it can be built here, verified from its tarball, with
# the crates it depends on that aren't on crates.io yet patched in from this checkout. With
# `--publish` it uploads for real, in the same order; cargo waits for each upload to reach the
# index before the next crate that needs it.
#
# deluge-sys and deluge-hal-linux link libdeluge, so they are packaged without the build check
# unless DELUGE_SDK_ROOT points at a prefix that has it.
#
# Usage: tools/publish-crates.sh [--publish]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# A dry run allows the lockfile edits the patches make; the tree is checked clean below.
mode=(--dry-run --allow-dirty)
if [ "${1:-}" = "--publish" ]; then
  mode=()
elif [ -n "${1:-}" ]; then
  echo "usage: tools/publish-crates.sh [--publish]" >&2
  exit 2
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "error: the working tree has uncommitted changes" >&2
  exit 1
fi

# For a dry run, crates not yet on crates.io resolve to this checkout. Restore the standalone
# lockfiles afterwards: cargo records unused patches in them.
patch="$(mktemp)"
trap 'rm -f "$patch"; git checkout -q -- "*Cargo.lock"' EXIT
{
  echo '[patch.crates-io]'
  for entry in \
    rza1-pac:rza1/rza1-pac rza1-rt:rza1/rza1-rt rza1-rt-macros:rza1/rza1-rt-macros \
    rza1-hal:rza1/rza1-hal rza1-alloc:rza1/rza1-alloc \
    deluge-embedded-fatfs:crates/embedded-fatfs \
    deluge-block-device-adapters:crates/block-device-adapters \
    deluge-protocol:crates/deluge-protocol deluge-sim-link:crates/deluge-sim-link \
    deluge-simulator:crates/deluge-simulator \
    deluge-sys:linux/deluge-sys deluge-hal-linux:linux/deluge-hal-linux; do
    echo "${entry%%:*} = { path = \"$root/${entry#*:}\" }"
  done
} > "$patch"
config=()
if [ ${#mode[@]} -ne 0 ]; then
  config=(--config "$patch")
fi
host=x86_64-unknown-linux-gnu

# Multi-crate publishes verify against a local overlay registry that cargo extracts under
# registry/src/-<hash>/ and never refreshes for an unchanged version, so a crate packaged by an
# earlier run would be verified in its old form. Those dash-named directories hold only such
# overlays (crates.io's are index.crates.io-*); clear them first.
rm -rf "${CARGO_HOME:-$HOME/.cargo}"/registry/src/-*/

linux_verify=(--no-verify)
if [ -n "${DELUGE_SDK_ROOT:-}" ] && [ -f "$DELUGE_SDK_ROOT/lib/libdeluge.a" ]; then
  linux_verify=()
fi

step() { echo; echo "==> $*"; }

step "rza1 chip crates"
(cd rza1 && cargo publish "${mode[@]}" --workspace --features rza1-pac/rza1l,rza1-hal/rza1l)

step "FAT forks"
(cd crates/embedded-fatfs && cargo publish "${mode[@]}" --target "$host")
(cd crates/block-device-adapters && cargo publish "${mode[@]}" --target "$host")

step "simulator link and wire protocol"
(cd crates/deluge-sim-link && cargo publish "${mode[@]}" --target "$host")
cargo publish "${mode[@]}" "${config[@]}" -p deluge-protocol --target "$host"

step "simulator"
(cd crates/deluge-simulator && cargo publish "${mode[@]}" "${config[@]}" --target "$host")

step "Linux backend"
(cd linux/deluge-sys && cargo publish "${mode[@]}" "${linux_verify[@]}")
(cd linux/deluge-hal-linux && cargo publish "${mode[@]}" "${config[@]}" "${linux_verify[@]}")

step "SDK"
cargo publish "${mode[@]}" "${config[@]}" -Zbuild-std=core,alloc \
  -p arm-dsp -p deluge-fixedpoint -p deluge-sdk-macros -p max5136 \
  -p deluge-usb-classes -p deluge-bsp -p deluge-sdk

step "cargo deluge"
(cd tools/cargo-deluge && cargo publish "${mode[@]}" --target "$host")

echo
echo "==> done"
