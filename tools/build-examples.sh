#!/usr/bin/env bash
#
# Compile-prove every example app for the real firmware target
# (armv7a-none-eabihf). This is the coverage the `deluge` facade + the
# `#[deluge::app]` macro get — they can't be unit-tested on a host target
# (embassy-executor ARM bring-up), so building every example exercises the
# whole capability surface end to end (testing plan §4.7).
#
# Usage: tools/build-examples.sh   (needs the nightly toolchain + rust-src)
set -euo pipefail

cd "$(dirname "$0")/.."

# Examples that use the GPL deluge-ui-toolkit need a global allocator, so they
# build with `-Zbuild-std=core,alloc` (the `build-fw-alloc` alias).
ALLOC_EXAMPLES=(oled_menu oled_hmenu)

is_alloc() {
    local name="$1"
    for a in "${ALLOC_EXAMPLES[@]}"; do
        [ "$a" = "$name" ] && return 0
    done
    return 1
}

for dir in examples/*/; do
    name="$(basename "$dir")"
    if is_alloc "$name"; then
        echo "==> build-fw-alloc -p $name"
        cargo build-fw-alloc -p "$name"
    else
        echo "==> build-fw -p $name"
        cargo build-fw -p "$name"
    fi
done

echo "==> All examples built."
