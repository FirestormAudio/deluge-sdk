#!/usr/bin/env bash
#
# Layering check:
#   - The Wren subsystem and the bare-metal examples consume the SDK only through
#     the `deluge` facade, never the BSP/HAL crates beneath it, so SDK-internal
#     changes can't silently break them (and Wren can later move to its own repo).
#   - The chip crates (rza1l-hal, rza1-rt, rza1-alloc) never name the Deluge, and the HAL has no
#     global_asm! (startup is rza1-rt's).
#   - usb/ and drivers/ are board-agnostic: they never name the chip HAL or any deluge-* crate.
#   - deluge-bsp never depends on the simulator; deluge-sdk's sim backend plugs into
#     deluge_bsp::host_panel.
#
# Usage: tools/check-layering.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# Consumers that must reach the platform only through the `deluge` facade.
FACADE_ONLY_DIRS=(wren examples/baremetal)

# Code lines only: a `//` comment naming a BSP item for reference is fine. The chip crates
# (`rza1l_hal`, `rza1_alloc`, `rza1_rt`, …) are all off limits; the facade re-exports what an
# app needs (the allocator as `deluge::deluge_alloc`).
code_hits="$(grep -rnE --include='*.rs' --exclude-dir=target --exclude-dir=node_modules \
  '\b(deluge_bsp|rza1l_hal|rza1_[a-z_]+)\b' "${FACADE_ONLY_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"

# Matches every manifest form that can pull in the BSP or a chip crate: plain `key = …`,
# dotted keys (`deluge-bsp.workspace = true`), table headers
# (`[dependencies.deluge-bsp]` / `[target.'cfg(...)'.dependencies.rza1l-hal]`),
# and renamed deps (`package = "deluge-bsp"`).
dep_hits="$(grep -rnE --include='Cargo.toml' --exclude-dir=target --exclude-dir=node_modules \
  '(^[[:space:]]*(deluge-bsp|rza1l-hal|rza1-[a-z-]+)[[:space:]]*[.=])|(\.(deluge-bsp|rza1l-hal|rza1-[a-z-]+)\])|(package[[:space:]]*=[[:space:]]*"(deluge-bsp|rza1l-hal|rza1-[a-z-]+)")' \
  "${FACADE_ONLY_DIRS[@]}" || true)"

if [ -n "$code_hits$dep_hits" ]; then
  printf '%s\n' "$code_hits" "$dep_hits" | sed '/^$/d'
  echo "error: facade-only code (wren/, examples/baremetal/) reaches past \`deluge\` into deluge-bsp or the rza1 chip crates" >&2
  exit 1
fi
# The chip crates are board-agnostic: no Deluge names in their code or linker scripts (comments
# may cite the Deluge as an example). Their dependencies are checked against Cargo's graph at the
# end.
CHIP_DIRS=(rza1)
chip_code_hits="$(grep -rniE --include='*.rs' --include='*.x' 'deluge' "${CHIP_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' || true)"
if [ -n "$chip_code_hits" ]; then
  printf '%s\n' "$chip_code_hits"
  echo "error: a chip crate (rza1-pac, rza1l-hal, rza1-rt, rza1-alloc) names the Deluge; board facts belong in deluge-bsp" >&2
  exit 1
fi
# Startup and exception entry are rza1-rt's: the HAL has no global_asm! (spec, Phase 3 exit).
hal_asm_hits="$(grep -rn --include='*.rs' 'global_asm' rza1/rza1l-hal | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$hal_asm_hits" ]; then
  printf '%s\n' "$hal_asm_hits"
  echo "error: rza1l-hal contains global_asm!; startup and exception entry belong in rza1-rt" >&2
  exit 1
fi
# Board-agnostic crates (spec rule 3): usb/ and drivers/ never name the chip HAL, the board, or
# any deluge-* crate in code. Their dependencies are checked against Cargo's graph at the end.
AGNOSTIC_DIRS=(usb drivers)
agnostic_hits="$(grep -rnE --include='*.rs' --exclude-dir=target '\b(rza1l_hal|deluge_[a-z_]+)\b' "${AGNOSTIC_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$agnostic_hits" ]; then
  printf '%s\n' "$agnostic_hits"
  echo "error: usb/ or drivers/ depends on the chip or the board; they must stay board-agnostic" >&2
  exit 1
fi

# The BSP never names the simulator in code (spec rule 4); deluge-sdk's sim backend plugs into
# deluge_bsp::host_panel instead. Its dependencies are checked against Cargo's graph at the end.
BSP_DIR=crates/deluge-bsp
bsp_sim_hits="$(grep -rnE --include='*.rs' --exclude-dir=target '\bdeluge_sim(_link|ulator)\b' "$BSP_DIR" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$bsp_sim_hits" ]; then
  printf '%s\n' "$bsp_sim_hits"
  echo "error: deluge-bsp depends on the simulator; plug in through deluge_bsp::host_panel" >&2
  exit 1
fi

# Dependencies, from Cargo's resolved graph rather than the manifests' text: that sees renamed and
# transitive dependencies, and comments or descriptions can't trip it. Every feature is enabled,
# so an optional dependency counts; dev-dependencies don't ship and are not checked. Both
# workspaces: the root one and the chip crates' own (rza1/).
for manifest in Cargo.toml rza1/Cargo.toml; do
cargo metadata --format-version 1 --all-features --locked --manifest-path "$manifest" | python3 -c '
import json, os, sys

meta = json.load(sys.stdin)
# Paths relative to the repository root (the script changes there first), in either workspace.
root = os.getcwd() + "/"
pkgs = {p["id"]: p for p in meta["packages"]}
graph = {
    n["id"]: [d["pkg"] for d in n["deps"] if any(k["kind"] != "dev" for k in d["dep_kinds"])]
    for n in meta["resolve"]["nodes"]
}

def closure(start):
    seen, todo = set(), list(graph[start])
    while todo:
        pid = todo.pop()
        if pid not in seen:
            seen.add(pid)
            todo.extend(graph[pid])
    return {pkgs[pid]["name"] for pid in seen}

# (crates under this directory, what they must not depend on, the error)
RULES = [
    ("rza1/", lambda n: n.startswith("deluge-") or n.startswith("flare"),
     "an rza1 chip crate depends on a deluge-* or flare crate; board facts belong in deluge-bsp"),
    ("usb/", lambda n: n == "rza1l-hal" or n.startswith("deluge-"),
     "usb/ depends on the chip or the board; it must stay board-agnostic"),
    ("drivers/", lambda n: n == "rza1l-hal" or n.startswith("deluge-"),
     "drivers/ depends on the chip or the board; it must stay board-agnostic"),
    ("crates/deluge-bsp/", lambda n: n in ("deluge-sim-link", "deluge-simulator"),
     "deluge-bsp depends on the simulator; plug in through deluge_bsp::host_panel"),
]

failed = False
for pid in meta["workspace_members"]:
    rel = pkgs[pid]["manifest_path"][len(root):]
    for prefix, banned, message in RULES:
        if rel.startswith(prefix):
            bad = sorted(n for n in closure(pid) if banned(n))
            if bad:
                names = ", ".join(bad)
                print(f"{rel}: depends on {names}")
                print(f"error: {message}", file=sys.stderr)
                failed = True
sys.exit(1 if failed else 0)
'
done

echo "==> Layering check passed."
