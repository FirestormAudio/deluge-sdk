#!/usr/bin/env bash
#
# Layering check: the Wren subsystem consumes the SDK only through the `deluge`
# facade, never the BSP/HAL crates beneath it, so SDK-internal changes can't
# silently break it (and it can later move to its own repo).
#
# Usage: tools/check-layering.sh
set -euo pipefail

cd "$(dirname "$0")/.."

WREN_DIRS=(wren)

# Code lines only: a `//` comment naming a BSP item for reference is fine.
code_hits="$(grep -rnE --include='*.rs' --exclude-dir=target --exclude-dir=node_modules \
  '\b(deluge_bsp|rza1l_hal)\b' "${WREN_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"

# Matches every manifest form that can pull in the BSP/HAL: plain `key = …`,
# dotted keys (`deluge-bsp.workspace = true`), table headers
# (`[dependencies.deluge-bsp]` / `[target.'cfg(...)'.dependencies.rza1l-hal]`),
# and renamed deps (`package = "deluge-bsp"`).
dep_hits="$(grep -rnE --include='Cargo.toml' --exclude-dir=target --exclude-dir=node_modules \
  '(^[[:space:]]*(deluge-bsp|rza1l-hal)[[:space:]]*[.=])|(\.(deluge-bsp|rza1l-hal)\])|(package[[:space:]]*=[[:space:]]*"(deluge-bsp|rza1l-hal)")' \
  "${WREN_DIRS[@]}" || true)"

if [ -n "$code_hits$dep_hits" ]; then
  printf '%s\n' "$code_hits" "$dep_hits" | sed '/^$/d'
  echo "error: Wren code reaches past the \`deluge\` facade into deluge-bsp/rza1l-hal" >&2
  exit 1
fi
# The chip HAL is board-agnostic: no Deluge names in its code (comments may cite the Deluge as an
# example), and no dependency on any deluge-* crate.
HAL_DIR=crates/rza1l-hal
hal_code_hits="$(grep -rniE --include='*.rs' --include='*.x' 'deluge' "$HAL_DIR/src" "$HAL_DIR/build.rs" "$HAL_DIR"/*.x \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' || true)"
hal_dep_hits="$(grep -nE '^[[:space:]]*(\[[^]]*\.)?deluge-|package[[:space:]]*=[[:space:]]*"deluge-' "$HAL_DIR/Cargo.toml" || true)"
if [ -n "$hal_code_hits$hal_dep_hits" ]; then
  printf '%s\n' "$hal_code_hits" "$hal_dep_hits" | sed '/^$/d'
  echo "error: rza1l-hal names the Deluge; board facts belong in deluge-bsp" >&2
  exit 1
fi
echo "==> Layering check passed."
