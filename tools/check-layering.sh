#!/usr/bin/env bash
#
# Layering check:
#   - The bare-metal examples consume the SDK only through the `deluge` facade, never
#     the BSP or chip crates beneath it, so SDK-internal changes can't silently break
#     them. (deluge-wren holds its own Wren code to the same rule.)
#   - The chip crates (rza1-hal, rza1-rt, rza1-alloc) never name the Deluge, and the HAL has no
#     global_asm! (startup is rza1-rt's).
#   - usb/ and drivers/ are board-agnostic: they never name the chip HAL or any deluge-* crate.
#   - deluge-bsp never depends on the simulator; deluge-sdk's sim backend plugs into
#     deluge_bsp::host_panel.
#
# Usage: tools/check-layering.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# Consumers that must reach the platform only through the `deluge` facade.
FACADE_ONLY_DIRS=(examples/baremetal)

# Code only: a `//` comment or a string literal naming a BSP item is fine, so each hit is
# re-checked with its strings and comment stripped. The chip crates (`rza1_hal`,
# `rza1_alloc`, `rza1_rt`, …) are all off limits; the facade re-exports what an app needs
# (the allocator as `deluge::deluge_alloc`).
code_hits="$(grep -rnE --include='*.rs' --exclude-dir=target --exclude-dir=node_modules \
  '\b(deluge_bsp|rza1_hal|rza1_[a-z_]+)\b' "${FACADE_ONLY_DIRS[@]}" \
  | awk '{
      code = $0
      sub(/^[^:]+:[0-9]+:/, "", code)
      gsub(/"([^"\\]|\\.)*"/, "\"\"", code)
      sub(/\/\/.*/, "", code)
      if (code ~ /(^|[^A-Za-z0-9_])(deluge_bsp|rza1_hal|rza1_[a-z_]+)([^A-Za-z0-9_]|$)/) print
    }' || true)"

# Matches every manifest form that can pull in the BSP or a chip crate: plain `key = …`,
# dotted keys (`deluge-bsp.workspace = true`), table headers
# (`[dependencies.deluge-bsp]` / `[target.'cfg(...)'.dependencies.rza1-hal]`),
# and renamed deps (`package = "deluge-bsp"`).
dep_hits="$(grep -rnE --include='Cargo.toml' --exclude-dir=target --exclude-dir=node_modules \
  '(^[[:space:]]*(deluge-bsp|rza1-hal|rza1-[a-z-]+)[[:space:]]*[.=])|(\.(deluge-bsp|rza1-hal|rza1-[a-z-]+)\])|(package[[:space:]]*=[[:space:]]*"(deluge-bsp|rza1-hal|rza1-[a-z-]+)")' \
  "${FACADE_ONLY_DIRS[@]}" || true)"

if [ -n "$code_hits$dep_hits" ]; then
  printf '%s\n' "$code_hits" "$dep_hits" | sed '/^$/d'
  echo "error: facade-only code (examples/baremetal/) reaches past \`deluge\` into deluge-bsp or the rza1 chip crates" >&2
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
  echo "error: a chip crate (rza1-pac, rza1-hal, rza1-rt, rza1-alloc) names the Deluge; board facts belong in deluge-bsp" >&2
  exit 1
fi
# The boot header and the status LED are the board's (spec decision 4): rza1-rt only places a
# `.boot_header` section; deluge-bsp emits the Deluge's header and blinks its LED on a fault.
chip_board_hits="$(grep -rnE --include='*.rs' --include='*.x' --exclude-dir=target 'BootLoad_ValidProgramTest|RZA1_STATUS_LED' "${CHIP_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' || true)"
if [ -n "$chip_board_hits" ]; then
  printf '%s\n' "$chip_board_hits"
  echo "error: a chip crate holds the board's boot header or status LED; they belong in deluge-bsp" >&2
  exit 1
fi
# Startup and exception entry are rza1-rt's: the HAL has no global_asm! (spec, Phase 3 exit).
hal_asm_hits="$(grep -rn --include='*.rs' 'global_asm' rza1/rza1-hal | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$hal_asm_hits" ]; then
  printf '%s\n' "$hal_asm_hits"
  echo "error: rza1-hal contains global_asm!; startup and exception entry belong in rza1-rt" >&2
  exit 1
fi
# The USB iso hooks are gone (spec decision 3): iso endpoints become `usb::iso` streams.
iso_hook_hits="$(grep -rnE --include='*.rs' --exclude-dir=target 'register_iso_(in|out)_hook' rza1 crates firmwares hw-tests examples \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$iso_hook_hits" ]; then
  printf '%s\n' "$iso_hook_hits"
  echo "error: a USB iso hook is still registered; bind an iso stream (rza1_hal::usb::iso) instead" >&2
  exit 1
fi

# Entry points are `#[entry]` (spec decision 4), which type-checks `fn() -> !`; a hand-exported
# `extern "C" fn main` is not.
entry_hits="$(grep -rnE --include='*.rs' --exclude-dir=target 'extern "C" fn main\b' firmwares hw-tests examples crates/deluge-sdk-macros/src \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$entry_hits" ]; then
  printf '%s\n' "$entry_hits"
  echo "error: an entry point is a hand-exported \`extern \"C\" fn main\`; use \`#[deluge_bsp::entry]\` (or \`#[deluge::app]\`)" >&2
  exit 1
fi

# The BSP's `fault-blink` fault handler is the image's to turn off (an application with its own
# `rza1_cpu_fault_report`): the library crates over the BSP leave its default features alone, so
# only an image's own defaults turn it on.
for lib in deluge-sdk deluge-grid-toolkit; do
  if cargo tree --locked -q -p "$lib" --no-default-features -e features --target armv7a-none-eabihf -i deluge-bsp 2>/dev/null \
     | grep -q 'deluge-bsp feature "fault-blink"'; then
    echo "error: $lib turns on deluge-bsp's fault-blink with its default features off; depend on deluge-bsp with default-features = false" >&2
    exit 1
  fi
done

# DelugeFirmware's hidden paths into the USB internals are retired (phase 9): the pipe and register
# layers are the drivers' alone.
usb_hidden_hits="$(grep -n 'doc(hidden)' rza1/rza1-hal/src/usb/mod.rs rza1/rza1-hal/src/usb/pipe.rs rza1/rza1-hal/src/usb/regs.rs || true)"
if [ -n "$usb_hidden_hits" ]; then
  printf '%s\n' "$usb_hidden_hits"
  echo "error: rza1-hal's USB pipe/register internals are public again (#[doc(hidden)]); keep them crate-private" >&2
  exit 1
fi

# Board-agnostic crates (spec rule 3): usb/ and drivers/ never name the chip HAL, the board, or
# any deluge-* crate in code. Their dependencies are checked against Cargo's graph at the end.
AGNOSTIC_DIRS=(usb drivers)
# Audio rings are lent by the caller (spec decision 2): the SSI and SCUX drivers hold no static
# buffers or descriptors of their own.
audio_static_hits="$(grep -rnE --include='*.rs' 'static mut' rza1/rza1-hal/src/ssi.rs rza1/rza1-hal/src/scux.rs rza1/rza1-hal/src/scux \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$audio_static_hits" ]; then
  printf '%s\n' "$audio_static_hits"
  echo "error: rza1-hal's SSI/SCUX drivers hold a static buffer; audio rings are lent by the caller" >&2
  exit 1
fi

agnostic_hits="$(grep -rnE --include='*.rs' --exclude-dir=target '\b(rza1_hal|deluge_[a-z_]+)\b' "${AGNOSTIC_DIRS[@]}" \
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
    ("usb/", lambda n: n == "rza1-hal" or n.startswith("deluge-"),
     "usb/ depends on the chip or the board; it must stay board-agnostic"),
    ("drivers/", lambda n: n == "rza1-hal" or n.startswith("deluge-"),
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
