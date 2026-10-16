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

echo "==> rza1: no deluge-* crate in the dependency graph"
cargo metadata --format-version 1 --all-features | python3 -c '
import json, sys
names = sorted({p["name"] for p in json.load(sys.stdin)["packages"]})
bad = [n for n in names if n.startswith("deluge") or n.startswith("flare")]
if bad:
    sys.exit("error: the rza1 workspace resolves " + ", ".join(bad))
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
