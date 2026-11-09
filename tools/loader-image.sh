#!/usr/bin/env bash
#
# The Deluge loader's boot-slot images, and an install image with stage-0, in target/loader-images/:
#
#   LOADER.SLT           the loader as a boot-slot image, for either slot (its shim is
#                        position-independent); copied to the card's /BOOT/, the running loader
#                        offers it as UPDATE LOADER
#   stage0+loader.bin    stage-0 and LOADER.SLT (as slot A) from flash offset 0, padded with 0xFF
#                        to the end of the 1 MB boot region, for a J-Link install
#
# A slot image's body is the shim (firmwares/loader-shim, executing in place at slot + 0x100,
# position-independent), padded with 0xFF to the payload, then the payload at slot + 0x1000: the app-loader's RAM image under a deluge_image::app header
# (`mkslot app`). The images are checked here, independently of mkslot: the slot's header and
# body CRC, and the payload's header and body CRC.
#
# Usage: tools/loader-image.sh [--sequence N]   (the slot images' sequence, default 1)
set -euo pipefail

cd "$(dirname "$0")/.."

SEQUENCE=1
while [ $# -gt 0 ]; do
  case $1 in
    --sequence)
      [ $# -ge 2 ] && [[ $2 =~ ^[0-9]+$ ]] || { echo "error: --sequence needs a number" >&2; exit 2; }
      SEQUENCE=$2
      shift 2
      ;;
    *) echo "usage: tools/loader-image.sh [--sequence N]" >&2; exit 2 ;;
  esac
done

OUT=target/loader-images
BIN=target/armv7a-none-eabihf/release
BUILD_STD=(-Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem)
HOST=$(rustc -vV | sed -n 's/^host: //p')
# deluge_loader::update::PAYLOAD.
PAYLOAD=0x1000

# llvm-objcopy, from the toolchain's llvm-tools-preview (rust-toolchain.toml), as
# tools/cargo-deluge finds it: on PATH first (cargo-binutils' rust-objcopy too), else the sysroot's.
OBJCOPY=$(command -v llvm-objcopy || command -v rust-objcopy || true)
if [ -z "$OBJCOPY" ]; then
  OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-objcopy"
  [ -x "$OBJCOPY" ] || { echo "error: llvm-objcopy not found (rustup component add llvm-tools-preview)" >&2; exit 1; }
fi

mkdir -p "$OUT"

mkslot() {
  cargo run -q --target "$HOST" -p deluge-image --example mkslot -- "$@"
}

# Build a package for the device, release: <package> [cargo args…]. Errors are shown.
build() {
  local pkg=$1 log
  shift
  if ! log=$(cargo build "${BUILD_STD[@]}" --release -p "$pkg" --target armv7a-none-eabihf "$@" 2>&1); then
    printf '%s\n' "$log" >&2
    echo "error: building $pkg $* failed" >&2
    return 1
  fi
}

echo "==> building"
build stage0
"$OBJCOPY" -O binary "$BIN/stage0" "$OUT/stage0.bin"
tools/check-xip-image.py --flash 0x18000000:0x10000 --ram 0x20020000:0x10000 \
  --require-symbol STAGE0_HEARTBEAT --require-symbol STAGE0_STATUS "$BIN/stage0"
build app-loader
python3 tools/check-boot-header.py "$BIN/app-loader"
mkslot app --elf "$BIN/app-loader" --variant-code 0 --out "$OUT/loader.app"

echo "==> slot image"
# The shim executes in place from whichever slot holds it, so it is built position-independent
# (ROPI: code and constants PC-relative; its RAM is fixed), in a target dir of its own so the
# flag doesn't reach anything else. Proof: linked for slot A and for slot B, the two binaries
# must be the same bytes.
SHIM_TARGET=target/shim-ropi
for slot in a b; do
  case $slot in
    a) feats=(); base=0x18010100 ;;
    b) feats=(--features slot-b); base=0x18080100 ;;
  esac
  if ! log=$(RUSTFLAGS="-C relocation-model=ropi" cargo build "${BUILD_STD[@]}" --release \
    -p loader-shim --target armv7a-none-eabihf --target-dir "$SHIM_TARGET" "${feats[@]}" 2>&1); then
    printf '%s\n' "$log" >&2
    echo "error: building the shim for slot $slot failed" >&2
    exit 1
  fi
  shim_elf="$SHIM_TARGET/armv7a-none-eabihf/release/loader-shim"
  tools/check-xip-image.py --flash "$base:0xf00" --ram 0x20010000:0x10000 \
    --require-symbol SHIM_STATUS "$shim_elf"
  "$OBJCOPY" -O binary "$shim_elf" "$OUT/shim-$slot.bin"
done
if ! cmp -s "$OUT/shim-a.bin" "$OUT/shim-b.bin"; then
  cmp -l "$OUT/shim-a.bin" "$OUT/shim-b.bin" | head -5 >&2
  echo "error: the shim isn't position-independent: linked for slot A and slot B it differs" >&2
  exit 1
fi
python3 - "$OUT/shim-a.bin" "$OUT/loader.app" "$OUT/body.bin" $((PAYLOAD)) <<'PY'
import sys
shim, app, out, payload = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
shim = open(shim, "rb").read()
room = payload - 0x100  # the body starts at slot + 0x100
if len(shim) > room:
    sys.exit(f"the shim is {len(shim)} bytes, more than its {room}")
open(out, "wb").write(shim + b"\xff" * (room - len(shim)) + open(app, "rb").read())
PY
mkslot wrap --body "$OUT/body.bin" --entry-offset 0x100 --sequence "$SEQUENCE" \
  --variant oled --out "$OUT/LOADER.SLT"

echo "==> install image"
mkslot compose --stage0 "$OUT/stage0.bin" --slot-a "$OUT/LOADER.SLT" --out "$OUT/stage0+loader.bin"
python3 -c 'import sys; p = sys.argv[1]; d = open(p, "rb").read()
assert len(d) <= 0x100000, f"{p} overruns the boot region"
open(p, "wb").write(d + b"\xff" * (0x100000 - len(d)))' "$OUT/stage0+loader.bin"

echo "==> checking"
tools/check-loader-image.py "$OUT"
echo "==> images in $OUT"
