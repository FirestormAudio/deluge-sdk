#!/usr/bin/env bash
#
# Layering check:
#   - The Wren subsystem and the bare-metal examples consume the SDK only through
#     the `deluge` facade, never the BSP/HAL crates beneath it, so SDK-internal
#     changes can't silently break them (and Wren can later move to its own repo).
#   - The chip HAL (rza1l-hal) never names the Deluge: board facts live in deluge-bsp.
#   - usb/ and drivers/ are board-agnostic: they never name the chip HAL or any deluge-* crate.
#   - deluge-bsp never depends on the simulator; deluge-sdk's sim backend plugs into
#     deluge_bsp::host_panel.
#
# Usage: tools/check-layering.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# Consumers that must reach the platform only through the `deluge` facade.
FACADE_ONLY_DIRS=(wren examples/baremetal)

# Code lines only: a `//` comment naming a BSP item for reference is fine.
code_hits="$(grep -rnE --include='*.rs' --exclude-dir=target --exclude-dir=node_modules \
  '\b(deluge_bsp|rza1l_hal)\b' "${FACADE_ONLY_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"

# Matches every manifest form that can pull in the BSP/HAL: plain `key = …`,
# dotted keys (`deluge-bsp.workspace = true`), table headers
# (`[dependencies.deluge-bsp]` / `[target.'cfg(...)'.dependencies.rza1l-hal]`),
# and renamed deps (`package = "deluge-bsp"`).
dep_hits="$(grep -rnE --include='Cargo.toml' --exclude-dir=target --exclude-dir=node_modules \
  '(^[[:space:]]*(deluge-bsp|rza1l-hal)[[:space:]]*[.=])|(\.(deluge-bsp|rza1l-hal)\])|(package[[:space:]]*=[[:space:]]*"(deluge-bsp|rza1l-hal)")' \
  "${FACADE_ONLY_DIRS[@]}" || true)"

if [ -n "$code_hits$dep_hits" ]; then
  printf '%s\n' "$code_hits" "$dep_hits" | sed '/^$/d'
  echo "error: facade-only code (wren/, examples/baremetal/) reaches past \`deluge\` into deluge-bsp/rza1l-hal" >&2
  exit 1
fi
# The chip HAL is board-agnostic: no Deluge names in its code (comments may cite the Deluge as an
# example), and no dependency on any deluge-* crate.
HAL_DIR=crates/rza1l-hal
# The whole crate directory, so code and linker scripts are covered wherever they live.
hal_code_hits="$(grep -rniE --include='*.rs' --include='*.x' 'deluge' "$HAL_DIR" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' || true)"
hal_dep_hits="$(grep -nE '^[[:space:]]*(\[[^]]*\.)?deluge-|package[[:space:]]*=[[:space:]]*"deluge-' "$HAL_DIR/Cargo.toml" || true)"
if [ -n "$hal_code_hits$hal_dep_hits" ]; then
  printf '%s\n' "$hal_code_hits" "$hal_dep_hits" | sed '/^$/d'
  echo "error: rza1l-hal names the Deluge; board facts belong in deluge-bsp" >&2
  exit 1
fi
# Board-agnostic crates (spec rule 3): usb/ and drivers/ never name the chip HAL, the board, or
# any deluge-* crate, in code or manifests.
AGNOSTIC_DIRS=(usb drivers)
agnostic_hits="$(grep -rnE --include='*.rs' --exclude-dir=target '\b(rza1l_hal|deluge_[a-z_]+)\b' "${AGNOSTIC_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
agnostic_dep_hits="$(grep -rnE --include='Cargo.toml' --exclude-dir=target '(rza1l-hal|deluge-[a-z-]+)' "${AGNOSTIC_DIRS[@]}" || true)"
if [ -n "$agnostic_hits$agnostic_dep_hits" ]; then
  printf '%s\n' "$agnostic_hits" "$agnostic_dep_hits" | sed '/^$/d'
  echo "error: usb/ or drivers/ depends on the chip or the board; they must stay board-agnostic" >&2
  exit 1
fi

# The BSP never depends on the simulator (spec rule 4); deluge-sdk's sim backend plugs into
# deluge_bsp::host_panel instead.
BSP_DIR=crates/deluge-bsp
bsp_sim_hits="$(grep -rnE --include='*.rs' --include='Cargo.toml' --exclude-dir=target 'deluge[-_]sim(_link|-link|ulator)' "$BSP_DIR" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|#)' || true)"
if [ -n "$bsp_sim_hits" ]; then
  printf '%s\n' "$bsp_sim_hits"
  echo "error: deluge-bsp depends on the simulator; plug in through deluge_bsp::host_panel" >&2
  exit 1
fi
echo "==> Layering check passed."
