#!/usr/bin/env bash
#
# Publishes the SDK's crates to crates.io in dependency order. Without arguments it is a dry
# run: every crate is packaged and, where it can be built here, verified from its tarball, with
# the crates it depends on that aren't on crates.io yet patched in from this checkout. With
# `--publish` it uploads for real, one crate at a time in the same order. That mode can be rerun:
# it skips every crate whose version is already on crates.io, and when crates.io's limit on new
# crate names stops it, it waits until the time crates.io gives and carries on.
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
    rza1-hal:rza1/rza1-hal \
    deluge-embedded-fatfs:crates/embedded-fatfs \
    deluge-block-device-adapters:crates/block-device-adapters \
    deluge-panel-protocol:crates/deluge-panel-protocol \
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

if [ ${#mode[@]} -eq 0 ]; then
  # Is version $2 of crate $1 on crates.io? Asks the sparse index, which isn't rate limited.
  published() {
    local n="$1" p
    case ${#n} in
      1) p="1/$n" ;; 2) p="2/$n" ;; 3) p="3/${n:0:1}/$n" ;; *) p="${n:0:2}/${n:2:2}/$n" ;;
    esac
    curl -sf "https://index.crates.io/$p" | grep -q "\"vers\":\"$2\""
  }

  # Publish crate $1 from directory $2 (relative to the root), with any further arguments
  # passed to cargo publish. Skips it if its version is already up.
  publish() {
    local name="$1" dir="$2"; shift 2
    local version log until wait
    version="$(cargo metadata --no-deps --format-version 1 --manifest-path "$dir/Cargo.toml" |
      python3 -c 'import json,sys; n=sys.argv[1]; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]==n))' "$name")"
    if published "$name" "$version"; then
      echo "==> $name $version is already on crates.io"
      return
    fi
    while :; do
      echo; echo "==> $name $version"
      log="$(mktemp)"
      if (cd "$dir" && cargo publish "$@") 2>&1 | tee "$log"; [ "${PIPESTATUS[0]}" -eq 0 ]; then
        rm -f "$log"
        return
      fi
      if grep -q "already exists" "$log"; then
        rm -f "$log"
        return
      fi
      if grep -q "too many new crates" "$log"; then
        until="$(grep -o 'try again after [^.]*GMT' "$log" | head -1 | sed 's/^try again after //')"
        wait=600
        if [ -n "$until" ]; then
          wait=$(( $(date -d "$until" +%s) - $(date +%s) + 15 ))
          [ "$wait" -gt 0 ] || wait=15
        fi
        rm -f "$log"
        echo "==> crates.io's new-crate limit: waiting $((wait / 60)) min $((wait % 60)) s"
        sleep "$wait"
        continue
      fi
      rm -f "$log"
      echo "error: publishing $name failed" >&2
      exit 1
    done
  }

  sdk=(-Zbuild-std=core,alloc)
  publish rza1-pac rza1/rza1-pac --features rza1l
  publish rza1-rt-macros rza1/rza1-rt-macros
  publish rza1-rt rza1/rza1-rt
  publish rza1-hal rza1/rza1-hal --features rza1l
  publish deluge-embedded-fatfs crates/embedded-fatfs --target "$host"
  publish deluge-block-device-adapters crates/block-device-adapters --target "$host"
  publish deluge-panel-protocol crates/deluge-panel-protocol --target "$host"
  publish deluge-simulator crates/deluge-simulator --target "$host"
  publish deluge-sys linux/deluge-sys "${linux_verify[@]}"
  publish deluge-hal-linux linux/deluge-hal-linux "${linux_verify[@]}"
  publish arm-dsp crates/arm-dsp "${sdk[@]}"
  publish deluge-fixedpoint crates/deluge-fixedpoint "${sdk[@]}"
  publish deluge-sdk-macros crates/deluge-sdk-macros "${sdk[@]}"
  publish max5136 drivers/max5136 "${sdk[@]}"
  publish deluge-usb-classes usb/usb-classes "${sdk[@]}"
  publish deluge-bsp crates/deluge-bsp "${sdk[@]}"
  publish deluge-sdk crates/deluge-sdk "${sdk[@]}"
  publish cargo-deluge tools/cargo-deluge --target "$host"

  echo
  echo "==> done"
  exit 0
fi

step() { echo; echo "==> $*"; }

step "rza1 chip crates"
(cd rza1 && cargo publish "${mode[@]}" --workspace --features rza1-pac/rza1l,rza1-hal/rza1l)

step "FAT forks"
(cd crates/embedded-fatfs && cargo publish "${mode[@]}" --target "$host")
(cd crates/block-device-adapters && cargo publish "${mode[@]}" --target "$host")

step "wire protocol"
cargo publish "${mode[@]}" "${config[@]}" -p deluge-panel-protocol --target "$host"

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
