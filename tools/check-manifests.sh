#!/usr/bin/env bash
#
# Resolve every Cargo manifest in the repo — the workspace plus each excluded
# crate — so a moved directory's stale `path =` dependency or workspace
# membership fails fast. Resolves only; builds nothing (the excluded crates need
# wasm / musl-bundle toolchains a plain checkout may lack).
#
# Usage: tools/check-manifests.sh
set -euo pipefail

cd "$(dirname "$0")/.."

status=0
while read -r m; do
  if ! out="$(cargo metadata --format-version 1 --manifest-path "$m" 2>&1 >/dev/null)"; then
    echo "FAIL $m"
    echo "$out" | tail -5
    status=1
  fi
done < <(git ls-files --cached --others --exclude-standard -- 'Cargo.toml' '**/Cargo.toml')

[ "$status" -eq 0 ] && echo "==> All manifests resolve."
exit "$status"
