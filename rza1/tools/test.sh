#!/usr/bin/env bash
#
# The RZ/A1 chip crates' whole suite: they build and test with nothing from deluge-sdk in reach.
# deluge-sdk's tools/test.sh runs this; CI also runs it on its own, from a checkout with no
# ../flare and no vendor manual.
#
# Usage: rza1/tools/test.sh
set -euo pipefail

cd "$(dirname "$0")/.."

QEMU=armv7-unknown-linux-gnueabihf
# The QEMU runner (see .cargo/config.toml for why it is set here). An environment variable replaces
# the configured value instead of merging with it.
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf"

echo "==> rza1: nothing outside rza1/ in the dependency graph"
# Every crate either comes from a registry/git source or lives in this workspace: a path dependency
# reaching back into deluge-sdk (whatever it is named) would build here and break at the split.
cargo metadata --format-version 1 --all-features | python3 -c '
import json, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"] + "/"
bad = sorted(
    p["name"] + " (" + p["manifest_path"] + ")"
    for p in meta["packages"]
    if p["source"] is None and not p["manifest_path"].startswith(root)
)
named = sorted({p["name"] for p in meta["packages"] if p["name"].startswith(("deluge", "flare"))})
if bad or named:
    sys.exit("error: the rza1 workspace reaches outside rza1/: " + ", ".join(bad + named))
'

tools/check-register-tables.sh
tools/check-pac.sh

echo "==> rza1: QEMU ARM tests"
cargo test --target "$QEMU" -p rza1-pac --features rza1l --lib
cargo test --target "$QEMU" -p rza1-pac --features rza1h --lib
cargo test --target "$QEMU" -p rza1l-hal --lib

echo "==> rza1: device build"
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem \
  -p rza1l-hal -p rza1-rt -p rza1-alloc

echo "==> rza1: all checks passed."
