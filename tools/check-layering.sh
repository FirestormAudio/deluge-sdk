#!/usr/bin/env bash
#
# Layering check: the Wren subsystem consumes the SDK only through the `deluge`
# facade, never the BSP/HAL crates beneath it, so SDK-internal changes can't
# silently break it (and it can later move to its own repo).
#
# Usage: tools/check-layering.sh
set -euo pipefail

cd "$(dirname "$0")/.."

WREN_DIRS=(wren-sys wren-firmware crates/deluge-wren-core tools/wren-web tools/wren-web-debug tools/wren-analyzer-wasm)

# Code lines only: a `//` comment naming a BSP item for reference is fine.
code_hits="$(grep -rnE --include='*.rs' --exclude-dir=target --exclude-dir=node_modules \
  '\b(deluge_bsp|rza1l_hal)\b' "${WREN_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
dep_hits="$(grep -rnE --include='Cargo.toml' --exclude-dir=target --exclude-dir=node_modules \
  '^[[:space:]]*(deluge-bsp|rza1l-hal)[[:space:]]*=' "${WREN_DIRS[@]}" || true)"

if [ -n "$code_hits$dep_hits" ]; then
  printf '%s\n' "$code_hits" "$dep_hits" | sed '/^$/d'
  echo "error: Wren code reaches past the \`deluge\` facade into deluge-bsp/rza1l-hal" >&2
  exit 1
fi
echo "==> Layering check passed."
