#!/usr/bin/env bash
#
# Regenerate rza1-pac: transforms/rza1l/absent.yaml from the RZ/A1L manual, then
# src/chips/{rza1l,rza1h}.rs from vendor/rza1.svd and each chip's transforms, then check the
# RZ/A1L PAC against the manual's register list. chiptool is pinned and installed under target/ on
# first use.
#
# Usage: tools/gen-pac.sh          regenerate
#        tools/gen-pac.sh --check  fail if a checked-in file differs from a fresh generation
set -euo pipefail

cd "$(dirname "$0")/.."

CHIPTOOL_REV=bcf538a2e7b8584ae874ee9ab72efb1576fc6152
ROOT="target/chiptool-$CHIPTOOL_REV"
BIN="$ROOT/bin/chiptool"
if [ ! -x "$BIN" ]; then
  cargo install --git https://github.com/embassy-rs/chiptool --rev "$CHIPTOOL_REV" --locked --root "$ROOT" \
    || cargo install --git https://github.com/embassy-rs/chiptool --rev "$CHIPTOOL_REV" --locked --root "$ROOT" --offline
fi

SVD=vendor/rza1.svd
MANUAL=vendor/docs/rza1/rza1l-hw-manual/ch46-list-of-registers.md
PAC=crates/rza1-pac
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

stale=0
emit() { # emit <generated file> <checked-in path>
  if [ "${MODE:-}" = check ]; then
    if ! cmp -s "$1" "$2"; then
      echo "error: $2 is stale; run tools/gen-pac.sh" >&2
      stale=1
    fi
  else
    cp "$1" "$2"
    echo "==> wrote $2"
  fi
}
[ "${1:-}" = --check ] && MODE=check

python3 tools/pac-a1l-absent.py "$SVD" "$MANUAL" "$OUT/absent.yaml" > /dev/null
emit "$OUT/absent.yaml" "$PAC/transforms/rza1l/absent.yaml"
# Generate from the transforms as they are on disk; in --check mode that's the checked-in
# absent.yaml, which was just compared.
for chip in rza1l rza1h; do
  mkdir -p "$OUT/$chip"
  "$BIN" generate --svd "$SVD" --transform "$PAC/transforms/$chip/chip.yaml" \
    --output "$OUT/$chip" --no-defmt --common-module 'crate::common' \
    --debug-ir-output "$OUT/$chip/ir.yaml"
  rustfmt --edition 2024 "$OUT/$chip/lib.rs"
  python3 tools/pac-postprocess.py "$OUT/$chip/lib.rs" "$OUT/$chip/chip.rs"
  emit "$OUT/$chip/chip.rs" "$PAC/src/chips/$chip.rs"
done
python3 tools/pac-check-a1l.py "$OUT/rza1l/ir.yaml" "$MANUAL"
[ "$stale" -eq 0 ] && [ "${MODE:-}" = check ] && echo "==> rza1-pac is up to date."
exit "$stale"
