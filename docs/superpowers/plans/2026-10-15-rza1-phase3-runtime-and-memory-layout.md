# RZ/A1 Phase 3: Runtime and Memory Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the runtime (vectors, reset, exception entry, linker script) out of `rza1l-hal` into a new `rza1-rt` crate, replace the 46 per-binary `memory.x`/`memory_rtt.x` files with one board layout in `deluge-bsp`, put the OSTM time driver behind a default feature, and rename `deluge-alloc` to `rza1-alloc`, without moving a single stack or heap boundary in any firmware.

**Architecture:** `rza1-rt` owns `startup.rs` (all of the HAL's `global_asm!`) and `link.x`, and its `build.rs` puts `link.x` (RTT or plain form, by its `rtt` feature) on the linker search path. `deluge-bsp`'s new `build.rs` puts the Deluge's `memory.x` (also by `rtt`) on the search path; `link.x` INCLUDEs it. Stack sizes in `memory.x` are `PROVIDE`d, so a binary overrides one with `--defsym` from its `build.rs`; each binary's `build.rs` shrinks to `-Tlink.x`. A layout-baseline script records every firmware's linker-defined layout symbols before the change and must match after it.

**Tech Stack:** Rust nightly, `armv7a-none-eabihf` with `-Zbuild-std=core,alloc`, rust-lld (GNU flavour) linker scripts, ARM `global_asm!`, Cargo build scripts, `arm-none-eabi-nm`/`objdump`, bash + python3 tooling.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 3 section; layering rules in §2).

## Global Constraints

- Layering rule 1: nothing chip-level (`crates/rza1l-hal`, and from this plan `crates/rza1-rt`, `crates/rza1-alloc`) names `deluge` in code or depends on a `deluge-*` crate.
- Layering rule 2: `wren/` and `examples/baremetal/` reach the platform only through the `deluge` facade (no `deluge_bsp`/`rza1l_hal` in their code or manifests).
- Layering rules 3 and 4 (Phase 2) stay green: `usb/`, `drivers/` board-agnostic; `deluge-bsp` never depends on the simulator.
- Phase 3 exit (spec): the identical `memory.x` copies are gone; the HAL contains no `global_asm!` startup.
- "Verify the overrides with `arm-none-eabi-nm` on the built ELF" (spec) — done by `tools/link-layout.sh check`.
- Monorepo: `deluge-sdk` builds against the sibling `../flare` checkout; work on a branch in this repo (or a sibling worktree), never a nested `.worktrees/`.
- Firmware builds: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p <pkg>` (the `build-fw` alias). zsh does not word-split a variable holding a multi-word command: write the command out.
- Full test suite: `tools/test.sh` (runs the layering check first). Never push; never publish to crates.io.

## Decisions against the spec's text

The spec is the authority; these four points follow its intent and depart from its wording, for the reason given. The executor treats them as settled.

1. **`unlock-bootloader` stays in `rza1l-hal`.** The spec lists it with the runtime, but the feature gates `spibsc`'s flash-unlock path (`spibsc.rs:658,679`), not startup code.
2. **No per-binary `memory.x` at all.** The spec has each binary keep `INCLUDE deluge_memory.x` plus overrides. `app-loader`'s two files differ from the default only in comments, and wren's only in `PROGRAM_STACK_SIZE`, which a `--defsym` from its `build.rs` overrides because `memory.x` `PROVIDE`s it (checked with rust-lld while writing this plan: `PROVIDE(X = 0x8000)` + `--defsym=X=0x40000` links with `X = 0x40000`). So the BSP writes `memory.x` itself and binaries carry none: all 46 files go.
3. **The scripts become `link.x`/`link_rtt.x`** (the Embassy/`cortex-m-rt` name), and `rza1-rt`'s `build.rs` publishes whichever matches its `rtt` feature as `link.x`, so binaries always pass `-Tlink.x`.
4. **The status LED becomes board data.** The spec is silent, but `startup.rs` hard-codes the Deluge's P6_7 LED in the boot and fault blink; moved unchanged, `rza1-rt` would carry Deluge wiring. `memory.x` sets `RZA1_STATUS_LED_PORT`/`_PIN`; `link.x` derives the register addresses (Task 4).

`deluge-alloc` is not on crates.io (checked with `cargo search`), so the rename needs no re-export crate. The facade keeps its `deluge::deluge_alloc` path as an alias.

## Review Focus

1. **A published crate ships without its script.** `cargo package` must include `crates/rza1-rt/link.x`, `link_rtt.x` and `crates/deluge-bsp/memory.x`, `memory_rtt.x`; otherwise every app built from crates.io fails to link. Pinned by `cargo package --list` steps in Tasks 2 and 3.
2. **A layout boundary moves silently**, above all wren's 256 KB / 192 KB program stack override, or the RTT reserve. Every firmware, plain and RTT, must keep its stack, heap-end, TTB and RTT addresses. Pinned by `tools/link-layout.sh check` against the baseline from Task 1, run in Tasks 2–7.
3. **A board's `memory.x` without the LED symbols, or with out-of-range ones**, must fail the link with a message naming the symbol, not produce an image whose fault blink writes some other register. Pinned in Task 4.
4. **A HAL built without default features** must still build and must not register an Embassy time driver; the BSP must keep the driver even if a dependent turns default features off. Pinned in Task 5.
5. **A new app from `cargo deluge new`** has no `memory.x` and must still link with the Deluge layout. Pinned in Task 3 by scaffolding an app and comparing its layout with blinky's.

---

## File Structure

| Path | Responsibility |
|---|---|
| `tools/link-layout.sh` (new) | Build every firmware (plain and RTT), print their linker-defined layout symbols, diff against the baseline |
| `tools/link-layout.baseline` (new) | The layout symbols on `main` before this plan; changes to it are deliberate |
| `crates/rza1-rt/` (new) | `Cargo.toml` (`links = "rza1-rt"`, features `rtt`, `app-fault-reporter`), `build.rs` (puts `link.x` on the search path), `link.x`, `link_rtt.x`, `src/lib.rs`, `src/startup.rs` (moved from the HAL), `README.md` |
| `crates/rza1l-hal/` | Loses `startup.rs`, `build.rs`, `rza1l.x`, `rza1l_rtt.x`, the `rtt`/`app-fault-reporter` features and the `rtt-target` dependency; gains `time-driver-ostm` |
| `crates/deluge-bsp/` | Gains `build.rs`, `memory.x`, `memory_rtt.x`, the `rza1-rt` dependency and `rtt`/`app-fault-reporter` features |
| 24 binary `build.rs` files | Shrink to `-Tlink.x` (wren adds its `--defsym`; additive_osc keeps its C++ build) |
| `crates/rza1-alloc/` (renamed from `crates/deluge-alloc/`) | Same code, chip-level name |
| `tools/cargo-deluge/src/new.rs` | The scaffold no longer writes `memory.x`/`memory_rtt.x` |
| `tools/check-layering.sh` | Rule 1 covers `rza1-rt` and `rza1-alloc`; the HAL may not contain `global_asm!` |

Binary packages (the list every multi-file step uses):

```
PLAIN: blinky oled_hello oled_menu oled_hmenu input_demo pad_paint midi_cv clock_jacks sd_demo
       audio_passthru audio_passthru_irq test_osc additive_osc button_leds usb_log
       demo-firmware deluge-controller msc-firmware app-loader wren-firmware
       uac-host-firmware sd-bench vbus-probe wp-probe
RTT:   blinky demo-firmware app-loader wren-firmware uac-host-firmware
```

---

### Task 1: Layout baseline

**Files:**
- Create: `tools/link-layout.sh`
- Create: `tools/link-layout.baseline` (generated)

**Interfaces:**
- Produces: `tools/link-layout.sh check` (exit 0 when every firmware's layout symbols equal the baseline, else prints a unified diff and exits 1), `tools/link-layout.sh update` (rewrites the baseline), `tools/link-layout.sh print <elf>...`. Later tasks run `check`; Task 4 runs `update` once and reviews the diff. Baseline lines are `<label> <symbol> <hex value>`, label = package name, `+rtt` suffix for RTT builds.

- [ ] **Step 1: Create the branch**

```bash
git checkout -b refactor/rza1-runtime
```

- [ ] **Step 2: Write the script**

`tools/link-layout.sh`:

```bash
#!/usr/bin/env bash
#
# The memory layout every firmware links with, as linker-defined symbols: stack sizes and
# bounds, the SRAM heap's end, the MMU table, BSS start and the RTT control block. Symbols that
# move with code size (`_start`, `end`, `__sram_heap_start`) are left out, so a code change
# doesn't touch the baseline; a layout change does, and has to be committed deliberately.
#
# Usage: tools/link-layout.sh check         build every firmware, diff against the baseline
#        tools/link-layout.sh update        build every firmware, rewrite the baseline
#        tools/link-layout.sh print <elf>…  print one ELF's layout symbols
set -euo pipefail

cd "$(dirname "$0")/.."

BASELINE=tools/link-layout.baseline
SYMS='(ABT|FIQ|IRQ|SVC|PROGRAM)_STACK_SIZE|(abt|fiq|irq|svc|program)_stack_(start|end)|__sram_heap_end|ttb_mmu1_base|__bss_start__|_SEGGER_RTT|INTERNAL_RAM_END|__rza1_status_led_(p|pm|pmc|pipc|mask)'
PLAIN=(blinky oled_hello oled_menu oled_hmenu input_demo pad_paint midi_cv clock_jacks sd_demo
       audio_passthru audio_passthru_irq test_osc additive_osc button_leds usb_log
       demo-firmware deluge-controller msc-firmware app-loader wren-firmware
       uac-host-firmware sd-bench vbus-probe wp-probe)
RTT=(blinky demo-firmware app-loader wren-firmware uac-host-firmware)

# Build one package and print the path of the executable it produced.
build_elf() {
  local elf
  elf=$(cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem \
          -p "$1" "${@:2}" --message-format=json 2>/dev/null \
        | python3 -c '
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("reason") == "compiler-artifact" and msg.get("executable"):
        print(msg["executable"])' | tail -1)
  if [ -z "$elf" ]; then
    echo "error: building $1 $* failed:" >&2
    cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p "$1" "${@:2}" >&2 || true
    exit 1
  fi
  echo "$elf"
}

print_layout() { # label elf
  arm-none-eabi-nm "$2" | awk -v label="$1" -v re="^($SYMS)\$" '$3 ~ re { print label, $3, $1 }' | sort -k2
}

collect() {
  local p elf
  for p in "${PLAIN[@]}"; do elf=$(build_elf "$p"); print_layout "$p" "$elf"; done
  for p in "${RTT[@]}"; do elf=$(build_elf "$p" --features rtt); print_layout "$p+rtt" "$elf"; done
}

case "${1:-}" in
  print)
    shift
    for elf in "$@"; do print_layout "$(basename "$elf")" "$elf"; done
    ;;
  check)
    got=$(mktemp)
    trap 'rm -f "$got"' EXIT
    collect > "$got"
    if diff -u "$BASELINE" "$got"; then
      echo "==> Link layout matches $BASELINE."
    else
      echo "error: the link layout moved (- baseline, + now); if that is intended, run '$0 update' and commit the baseline" >&2
      exit 1
    fi
    ;;
  update)
    collect > "$BASELINE"
    echo "==> Rewrote $BASELINE; review 'git diff $BASELINE' before committing."
    ;;
  *)
    echo "usage: $0 check | update | print <elf>..." >&2
    exit 2
    ;;
esac
```

```bash
chmod +x tools/link-layout.sh
```

- [ ] **Step 3: Record the baseline**

Run: `tools/link-layout.sh update && wc -l tools/link-layout.baseline && grep -c "^wren-firmware PROGRAM_STACK_SIZE 00040000$\|^wren-firmware+rtt PROGRAM_STACK_SIZE 00030000$\|^blinky PROGRAM_STACK_SIZE 00008000$" tools/link-layout.baseline`
Expected: about 29 × 19 lines (plain builds have no `_SEGGER_RTT`; nothing has `__rza1_status_led_*` yet), and the final count `3`.

- [ ] **Step 4: Check passes on the unchanged tree, and fails on a moved boundary**

Run: `tools/link-layout.sh check`
Expected: `==> Link layout matches tools/link-layout.baseline.`

Then prove it catches a change:

```bash
sed -i 's/PROGRAM_STACK_SIZE = 0x8000;/PROGRAM_STACK_SIZE = 0x9000;/' examples/baremetal/blinky/memory.x
tools/link-layout.sh check; echo "exit $?"
git checkout examples/baremetal/blinky/memory.x
```

Expected: a diff showing `-blinky PROGRAM_STACK_SIZE 00008000` / `+blinky PROGRAM_STACK_SIZE 00009000` (and the moved `program_stack_start`, `__sram_heap_end`, …), and `exit 1`.

- [ ] **Step 5: Commit**

```bash
git add tools/link-layout.sh tools/link-layout.baseline
git commit -m "chore(tools): record every firmware's link layout and check it against a baseline"
```

---

### Task 2: The `rza1-rt` crate

**Files:**
- Create: `crates/rza1-rt/Cargo.toml`, `crates/rza1-rt/build.rs`, `crates/rza1-rt/src/lib.rs`, `crates/rza1-rt/README.md`
- Move: `crates/rza1l-hal/src/startup.rs` → `crates/rza1-rt/src/startup.rs`; `crates/rza1l-hal/rza1l.x` → `crates/rza1-rt/rza1l.x`; `crates/rza1l-hal/rza1l_rtt.x` → `crates/rza1-rt/rza1l_rtt.x` (renamed to `link.x`/`link_rtt.x` in Task 3)
- Delete: `crates/rza1l-hal/build.rs`
- Modify: `crates/rza1l-hal/Cargo.toml`, `crates/rza1l-hal/src/lib.rs:18-20,41-42`, `crates/rza1l-hal/README.md:30`
- Modify: `crates/deluge-bsp/Cargo.toml`, `crates/deluge-bsp/src/lib.rs:8-9`
- Modify: `crates/deluge-sdk/Cargo.toml:103` (`rtt` feature)
- Modify: the `rtt` feature of `firmwares/{controller-firmware,demo-firmware,app-loader,msc-firmware}/Cargo.toml` and `hw-tests/{sd-bench,uac-host-firmware,vbus-probe,wp-probe}/Cargo.toml`; the comment at `hw-tests/uac-host-firmware/Cargo.toml:29-31`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Consumes: `tools/link-layout.sh check` (Task 1).
- Produces: crate `rza1-rt` (lib name `rza1_rt`), features `rtt` and `app-fault-reporter`; its `build.rs` puts `rza1l.x` and `rza1l_rtt.x` on the linker search path (Task 3 replaces them with one `link.x`). The image must define `main` and `gic_dispatch` (`rza1l_hal::gic` provides it). `deluge-bsp` features `rtt = ["rza1-rt/rtt"]`, `app-fault-reporter = ["rza1-rt/app-fault-reporter"]`, and `use rza1_rt as _;` on the device target.

- [ ] **Step 1: Write the failing check**

The exit criterion as a test: the HAL has no `global_asm!`.

Run: `grep -rln global_asm crates/rza1l-hal/src`
Expected: `crates/rza1l-hal/src/startup.rs` (the failing state).

- [ ] **Step 2: Create the crate and move the runtime**

```bash
mkdir -p crates/rza1-rt/src
git mv crates/rza1l-hal/src/startup.rs crates/rza1-rt/src/startup.rs
git mv crates/rza1l-hal/rza1l.x crates/rza1-rt/rza1l.x
git mv crates/rza1l-hal/rza1l_rtt.x crates/rza1-rt/rza1l_rtt.x
git rm -q crates/rza1l-hal/build.rs
cp crates/rza1l-hal/LICENSE-APACHE crates/rza1l-hal/LICENSE-MIT crates/rza1-rt/
```

`crates/rza1-rt/Cargo.toml`:

```toml
[package]
name = "rza1-rt"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
readme = "README.md"
description = "Runtime for the Renesas RZ/A1L (Cortex-A9): reset vector, exception entry and linker script."
categories = ["embedded", "no-std"]
keywords = ["rza1", "renesas", "cortex-a9", "runtime", "startup"]
# One vector table and one `_start` per image: Cargo refuses a second copy of this crate.
links = "rza1-rt"

# docs.rs builds on the x86_64 host by default; build the docs for the device.
[package.metadata.docs.rs]
default-target = "armv7a-none-eabihf"

[lib]
# Assembly only; nothing to unit-test.
test = false

[features]
## Each fault handler first dumps the fault registers to SEGGER RTT channel 0, and the linker
## script reserves the RTT buffer (the board's memory.x must then define RTT_RAM and
## NCACHE_RTT_RAM). The image must also link `rtt-target`, which defines `_SEGGER_RTT`.
rtt = []
## Hand every CPU fault to the application's `rza1_cpu_fault_report`; without it, faults blink
## the status LED. The application must then define the reporter, or the link fails.
app-fault-reporter = []
```

`crates/rza1-rt/build.rs`:

```rust
//! Puts the linker scripts on the linker search path; a binary selects one with `-T`, and the
//! script INCLUDEs the board's `memory.x`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    for script in ["rza1l.x", "rza1l_rtt.x"] {
        fs::copy(manifest_dir.join(script), out_dir.join(script)).unwrap();
        println!("cargo:rerun-if-changed={script}");
    }
    println!("cargo:rustc-link-search={}", out_dir.display());
}
```

`crates/rza1-rt/src/lib.rs`:

```rust
//! `rza1-rt` — the runtime for the Renesas RZ/A1L (Cortex-A9): the reset vector and bootloader
//! metadata, the reset handler, the fault handlers, the GIC IRQ entry stub and the linker script.
//!
//! Link it once per image: nothing calls into it by name, so the board crate does
//! `use rza1_rt as _;`. The image must define `main` (the reset handler calls it) and
//! `gic_dispatch` (the IRQ stub calls it; `rza1l_hal::gic` provides it), and the board crate puts
//! a `memory.x` on the linker search path for the script to INCLUDE.
#![no_std]

// ARM-mode assembly; nothing to build for a host target.
#[cfg(target_os = "none")]
mod startup;
```

`crates/rza1-rt/README.md`:

```markdown
# rza1-rt

Runtime for the Renesas RZ/A1L (ARM Cortex-A9, R7S721001): the reset vector with the boot
loader's metadata words, the reset handler (cache/TLB invalidate, VFP/NEON enable, per-mode
stacks, BSS zero, then `main`), the fault handlers and the GIC IRQ entry stub, plus the linker
script.

A board crate depends on it, links it with `use rza1_rt as _;`, and puts a `memory.x` on the
linker search path defining the `RAM` (and optionally `SDRAM`) regions, the per-mode stack sizes
and `INTERNAL_RAM_END`; see the script's header for the full list. The application's `build.rs`
passes `-T` with the script name.

| Feature | Effect |
|---|---|
| `rtt` | Fault handlers dump their registers to SEGGER RTT channel 0; the script reserves the RTT buffer (`RTT_RAM`, `NCACHE_RTT_RAM` regions) |
| `app-fault-reporter` | Faults call the application's `rza1_cpu_fault_report(fault_pc, fault_type, sys_lr, sys_sp)` instead of blinking the status LED |

Licensed under either of Apache-2.0 or MIT at your option.
```

Leave `crates/rza1-rt/src/startup.rs` byte-identical in this task (Task 4 edits it).

In both `crates/rza1-rt/rza1l.x` and `rza1l_rtt.x`, the header says `The binary crate (or BSP) must provide memory.x`; leave it. Replace the phrase `(from the rza1l-hal crate)` wherever it appears in them with `(from the rza1-rt crate)`:

Run: `grep -n "rza1l-hal" crates/rza1-rt/*.x`
Expected: no lines, or fix each one it prints to say `rza1-rt`.

- [ ] **Step 3: Strip the runtime out of the HAL**

`crates/rza1l-hal/Cargo.toml`: delete these lines (the `rtt` and `app-fault-reporter` features, the `unlock-bootloader` stays):

```toml
rtt = ["dep:rtt-target"]
# Hand every CPU fault to the application's `rza1_cpu_fault_report` (see `startup.rs`). The
# application must then define it; without the feature, faults blink the LED.
app-fault-reporter = []
```

and

```toml
[dependencies.rtt-target]
workspace = true
optional = true
```

`crates/rza1l-hal/src/lib.rs`: replace

```rust
pub mod adc;
// startup, mmu and cache use ARM coprocessor assembly and cannot compile on the host.
pub mod bsc;
```

with

```rust
pub mod adc;
// mmu and cache use ARM coprocessor instructions and cannot compile on the host. The reset
// vector and exception entry live in the `rza1-rt` crate.
pub mod bsc;
```

and delete

```rust
#[cfg(target_os = "none")]
pub mod startup;
```

`crates/rza1l-hal/README.md:30`: delete the `| \`startup\` | … |` row.

- [ ] **Step 4: Run the check from Step 1**

Run: `grep -rln global_asm crates/rza1l-hal/src; echo "exit $?"`
Expected: no file names, `exit 1`.

- [ ] **Step 5: Link the runtime through the BSP**

`Cargo.toml` (workspace): add `"crates/rza1-rt",` to `members` directly after `"crates/rza1l-hal",`.

`crates/deluge-bsp/Cargo.toml`, `[dependencies]`: add after the `rza1l-hal` line

```toml
rza1-rt = { path = "../rza1-rt", version = "0.1.0" }
```

and in `[features]`, after `default = []`:

```toml
## SEGGER RTT: fault dumps to RTT channel 0 and the RTT memory layout.
rtt = ["rza1-rt/rtt"]
## Hand CPU faults to the application's `rza1_cpu_fault_report` (see `rza1-rt`).
app-fault-reporter = ["rza1-rt/app-fault-reporter"]
```

`crates/deluge-bsp/src/lib.rs`: replace

```rust
// Startup code lives in `rza1l_hal::startup`; the linker script references
// `_start` and the vector table, so it is linked into every binary.
```

with

```rust
// The runtime (reset vector, exception entry, linker script). Nothing calls into it by name,
// so this is what links it into every image built on the BSP.
#[cfg(target_os = "none")]
use rza1_rt as _;
```

`crates/deluge-sdk/Cargo.toml:103`: `rtt = ["dep:rtt-target", "rza1l-hal/rtt"]` becomes `rtt = ["dep:rtt-target", "deluge-bsp/rtt"]`.

The eight firmware manifests:

```bash
sed -i 's|"rza1l-hal/rtt"|"deluge-bsp/rtt"|' \
  firmwares/controller-firmware/Cargo.toml firmwares/demo-firmware/Cargo.toml \
  firmwares/app-loader/Cargo.toml firmwares/msc-firmware/Cargo.toml \
  hw-tests/sd-bench/Cargo.toml hw-tests/uac-host-firmware/Cargo.toml \
  hw-tests/vbus-probe/Cargo.toml hw-tests/wp-probe/Cargo.toml
grep -rn '"rza1l-hal/rtt"\|rza1l-hal/app-fault' --include=Cargo.toml . | grep -v /target/
```

Expected: the final grep prints nothing.

In `hw-tests/uac-host-firmware/Cargo.toml:29-31` the comment says enabling `rza1l-hal/rtt` compiles "the hal's RTT panic/abort assembly (crates/rza1l-hal/src/…"; change those words to `deluge-bsp/rtt` and `the runtime's RTT fault-dump assembly (crates/rza1-rt/src/startup.rs)` and keep the rest of the comment.

- [ ] **Step 6: Build and compare the layout**

Run: `tools/link-layout.sh check`
Expected: `==> Link layout matches tools/link-layout.baseline.` (Same scripts, same `memory.x` files; only the crate that owns them moved.)

Run: `arm-none-eabi-nm $(ls -t target/armv7a-none-eabihf/debug/blinky | head -1) | grep -E " (_start|_reset_handler|_irq_handler|gic_dispatch)$"`
Expected: all four symbols present (`T`).

- [ ] **Step 7: The published crate carries its scripts**

Run: `cargo package -p rza1-rt --list --allow-dirty --no-verify | sort`
Expected: the list includes `build.rs`, `rza1l.x`, `rza1l_rtt.x`, `src/lib.rs`, `src/startup.rs`, `README.md`.

- [ ] **Step 8: Run the test suite**

Run: `tools/test.sh > /tmp/claude-1000/phase3-t2.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/phase3-t2.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `exit 0`, `failed 0`.

- [ ] **Step 9: Commit**

```bash
git add -A crates/rza1-rt crates/rza1l-hal crates/deluge-bsp crates/deluge-sdk/Cargo.toml \
  firmwares/*/Cargo.toml hw-tests/*/Cargo.toml Cargo.toml Cargo.lock
git commit -m "refactor(rza1-rt): the reset vector, exception entry and linker scripts move out of the HAL"
```

---

### Task 3: One board layout in `deluge-bsp`; binaries pass `-Tlink.x`

**Files:**
- Move: `examples/baremetal/blinky/memory.x` → `crates/deluge-bsp/memory.x`; `examples/baremetal/blinky/memory_rtt.x` → `crates/deluge-bsp/memory_rtt.x`
- Delete: the other 44 `memory.x`/`memory_rtt.x` files (the list in Step 4)
- Move: `crates/rza1-rt/rza1l.x` → `crates/rza1-rt/link.x`; `crates/rza1-rt/rza1l_rtt.x` → `crates/rza1-rt/link_rtt.x`
- Modify: `crates/rza1-rt/build.rs`, `crates/rza1-rt/README.md`
- Create: `crates/deluge-bsp/build.rs`
- Modify: all 24 binary `build.rs` files (list in Step 6)
- Modify: `tools/cargo-deluge/src/new.rs:10-12,81-83,279-281`

**Interfaces:**
- Consumes: `rza1-rt`'s `rtt` feature and the BSP's `rtt` forwarding (Task 2); `tools/link-layout.sh` (Task 1).
- Produces: `link.x` on the search path (the RTT form when `rza1-rt/rtt` is on); `memory.x` on the search path (the RTT form when `deluge-bsp/rtt` is on) with `PROVIDE`d `PROGRAM_STACK_SIZE`, `IRQ_STACK_SIZE`, `FIQ_STACK_SIZE`, `SVC_STACK_SIZE`, `ABT_STACK_SIZE`. Binary `build.rs` contract: `cargo:rustc-link-arg-bins=-Tlink.x`, plus `--defsym=<NAME>_STACK_SIZE=<value>` to override a stack.

- [ ] **Step 1: Write the failing check**

Run: `find . -name 'memory*.x' -not -path './target/*' -not -path '*/node_modules/*' | wc -l`
Expected: `48` (46 binary copies plus the two BSP will own). After this task: `2`.

- [ ] **Step 2: `rza1-rt` publishes one `link.x`**

```bash
git mv crates/rza1-rt/rza1l.x crates/rza1-rt/link.x
git mv crates/rza1-rt/rza1l_rtt.x crates/rza1-rt/link_rtt.x
```

In `crates/rza1-rt/link.x` replace the comment lines

```
 * For RTT-enabled builds use rza1l_rtt.x instead (requires RTT_RAM and
 * NCACHE_RTT_RAM regions in memory.x).
```

with

```
 * With rza1-rt's `rtt` feature, build.rs publishes link_rtt.x as link.x instead (it requires
 * RTT_RAM and NCACHE_RTT_RAM regions in memory.x).
```

and in `crates/rza1-rt/link_rtt.x` replace

```
 * For RTT-free builds use rza1l.x instead (no RTT_RAM/NCACHE_RTT_RAM needed).
```

with

```
 * Published as link.x when rza1-rt's `rtt` feature is on; otherwise link.x is the RTT-free form.
```

Also in both, the header lines naming the stack symbols:

```
 *   PROGRAM_STACK_SIZE = ...;
```

stay as they are (a `PROVIDE` satisfies them).

`crates/rza1-rt/build.rs` becomes:

```rust
//! Puts `link.x` on the linker search path: the RTT form when the `rtt` feature is on. A binary
//! links with `-Tlink.x`, and the script INCLUDEs the board's `memory.x`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=link.x");
    println!("cargo:rerun-if-changed=link_rtt.x");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let script = if env::var_os("CARGO_FEATURE_RTT").is_some() { "link_rtt.x" } else { "link.x" };

    fs::copy(manifest_dir.join(script), out_dir.join("link.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
}
```

In `crates/rza1-rt/README.md`, replace `The application's \`build.rs\`
passes \`-T\` with the script name.` with `The application's \`build.rs\`
passes \`-Tlink.x\` (\`cargo:rustc-link-arg-bins=-Tlink.x\`).`

- [ ] **Step 3: The BSP owns the Deluge's layout**

```bash
git mv examples/baremetal/blinky/memory.x crates/deluge-bsp/memory.x
git mv examples/baremetal/blinky/memory_rtt.x crates/deluge-bsp/memory_rtt.x
```

In `crates/deluge-bsp/memory.x`, replace the header comment

```
/* Board-specific memory layout for the Synthstrom Deluge (RZ/A1L) — RTT disabled.
 * This file is INCLUDE'd by rza1l.x (from the rza1l-hal crate) when the
 * firmware is built without the rtt feature.
 * Edit here to change RAM/SDRAM sizes or stack sizes. */
```

with

```
/* Memory layout of the Synthstrom Deluge (RZ/A1L), RTT disabled. deluge-bsp's build.rs puts it
 * on the linker search path as memory.x, and rza1-rt's link.x INCLUDEs it. */
```

and in `crates/deluge-bsp/memory_rtt.x` replace

```
/* Board-specific memory layout for the Synthstrom Deluge (RZ/A1L) — RTT enabled.
 * This file is INCLUDE'd by rza1l_rtt.x (from the rza1l-hal crate) when the
 * firmware is built with --features rtt (the default).
 * Edit here to change RAM/SDRAM sizes, RTT placement, or stack sizes. */
```

with

```
/* Memory layout of the Synthstrom Deluge (RZ/A1L) with the SEGGER RTT reserve. deluge-bsp's
 * build.rs puts it on the linker search path as memory.x when the `rtt` feature is on, and
 * rza1-rt's link.x (its RTT form) INCLUDEs it. */
```

In both files replace the stack block

```
/* Stack sizes */
PROGRAM_STACK_SIZE = 0x8000;   /* 32 KB — application / SYS mode */
IRQ_STACK_SIZE     = 0x2000;   /*  8 KB */
FIQ_STACK_SIZE     = 0x2000;   /*  8 KB */
SVC_STACK_SIZE     = 0x2000;   /*  8 KB */
ABT_STACK_SIZE     = 0x2000;   /*  8 KB */
```

with

```
/* Stack sizes. PROVIDE, so a binary can override one from its build.rs with
 * `cargo:rustc-link-arg-bins=--defsym=PROGRAM_STACK_SIZE=<bytes>`. */
PROVIDE(PROGRAM_STACK_SIZE = 0x8000);   /* 32 KB — application / SYS mode */
PROVIDE(IRQ_STACK_SIZE     = 0x2000);   /*  8 KB */
PROVIDE(FIQ_STACK_SIZE     = 0x2000);   /*  8 KB */
PROVIDE(SVC_STACK_SIZE     = 0x2000);   /*  8 KB */
PROVIDE(ABT_STACK_SIZE     = 0x2000);   /*  8 KB */
```

`crates/deluge-bsp/build.rs`:

```rust
//! Puts the Deluge's memory layout on the linker search path as `memory.x`, for `rza1-rt`'s
//! `link.x` to INCLUDE: the RTT form (with the RTT_RAM regions) when the `rtt` feature is on.
//! The same feature turns on `rza1-rt/rtt`, which selects the matching form of `link.x`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=memory_rtt.x");

    // Host builds (tests, the simulator) use the platform's own linker setup.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let layout = if env::var_os("CARGO_FEATURE_RTT").is_some() { "memory_rtt.x" } else { "memory.x" };

    fs::copy(manifest_dir.join(layout), out_dir.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
}
```

- [ ] **Step 4: Delete the per-binary layouts**

```bash
for d in examples/baremetal/{additive_osc,audio_passthru,audio_passthru_irq,button_leds,clock_jacks,input_demo,midi_cv,oled_hello,oled_hmenu,oled_menu,pad_paint,sd_demo,test_osc,usb_log} \
         firmwares/{controller-firmware,demo-firmware,msc-firmware,app-loader} \
         hw-tests/{sd-bench,uac-host-firmware,vbus-probe,wp-probe} wren/wren-firmware; do
  git rm -q "$d/memory.x" "$d/memory_rtt.x"
done
find . -name 'memory*.x' -not -path './target/*' -not -path '*/node_modules/*'
```

Expected: `./crates/deluge-bsp/memory.x` and `./crates/deluge-bsp/memory_rtt.x` only.

- [ ] **Step 5: Watch a binary fail to link with its old build.rs**

Run: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p blinky 2>&1 | grep -m3 -E "error|memory.x|rza1l.x"`
Expected: an error from blinky's build script (`fs::copy` of the deleted `memory.x` fails: "No such file or directory"). That is the old contract failing; Step 6 replaces it.

- [ ] **Step 6: Rewrite the binaries' build scripts**

The 21 plain binaries get exactly this `build.rs`:

```rust
fn main() {
    // rza1-rt puts `link.x` on the linker search path and deluge-bsp puts the Deluge's
    // `memory.x` next to it; this selects the script. Host (simulator) builds use the
    // platform's own linker setup.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
    }
}
```

Write it to each of them:

```bash
for d in examples/baremetal/{audio_passthru,audio_passthru_irq,blinky,button_leds,clock_jacks,input_demo,midi_cv,oled_hello,oled_hmenu,oled_menu,pad_paint,sd_demo,test_osc,usb_log} \
         firmwares/{controller-firmware,demo-firmware,msc-firmware,app-loader} \
         hw-tests/{sd-bench,uac-host-firmware,vbus-probe,wp-probe}; do
  cat > "$d/build.rs" <<'EOF'
fn main() {
    // rza1-rt puts `link.x` on the linker search path and deluge-bsp puts the Deluge's
    // `memory.x` next to it; this selects the script. Host (simulator) builds use the
    // platform's own linker setup.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
    }
}
EOF
done
```

(That is 22 directories: 14 examples, 4 firmwares, 4 hw-tests.)

`examples/baremetal/additive_osc/build.rs`: replace the whole `link_firmware` function

```rust
/// Device-only: place the memory layout and pick the linker script that matches
/// the `rtt` feature (the same as the other examples).
fn link_firmware(manifest_dir: &Path) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // rza1l-hal's build.rs places rza1l.x / rza1l_rtt.x on the search path; we
    // just need to put the matching memory.x alongside them.
    let rtt = env::var("CARGO_FEATURE_RTT").is_ok();
    let (memory_src, linker_script) = if rtt {
        ("memory_rtt.x", "rza1l_rtt.x")
    } else {
        ("memory.x", "rza1l.x")
    };

    fs::copy(manifest_dir.join(memory_src), out_dir.join("memory.x")).unwrap();

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg=-T{linker_script}");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=memory_rtt.x");
}
```

with

```rust
/// Device-only: select rza1-rt's `link.x` (deluge-bsp puts the Deluge's `memory.x` next to it).
fn link_firmware() {
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
}
```

change the call `link_firmware(&manifest_dir);` to `link_firmware();`, and in its module doc replace

```
//! 2. **Device linker setup** — copy the rtt/non-rtt `memory.x` and select the
//!    matching `rza1l.x` linker script (the same as the other examples).
```

with

```
//! 2. **Device linker setup** — select rza1-rt's `link.x` (the same as the other examples).
```

Then fix the imports the compiler reports as unused (`std::fs` goes; keep `Path`/`PathBuf` while `build_dsp_core` uses them).

`wren/wren-firmware/build.rs`: replace

```rust
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // ── Memory layout / linker script (mirrors demo-firmware) ───────────────
    let rtt = env::var("CARGO_FEATURE_RTT").is_ok();
    let (memory_src, linker_script) = if rtt {
        ("memory_rtt.x", "rza1l_rtt.x")
    } else {
        ("memory.x", "rza1l.x")
    };
    fs::copy(manifest_dir.join(memory_src), out_dir.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg=-T{linker_script}");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=memory_rtt.x");
```

with

```rust
    // ── Linker script and program stack ─────────────────────────────────────
    // The Deluge's memory.x (from deluge-bsp) PROVIDEs a 32 KB program stack. The wren VM's
    // compiler and interpreter recurse deeply on the executor's (program) stack, especially in
    // debug builds, so take 256 KB; with RTT, 192 KB, which still ends above the RTT reserve
    // (0x202B0000–0x202C0000).
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    let program_stack = if env::var_os("CARGO_FEATURE_RTT").is_some() { "0x30000" } else { "0x40000" };
    println!("cargo:rustc-link-arg-bins=--defsym=PROGRAM_STACK_SIZE={program_stack}");
```

In its module doc replace

```
//!  1. Select the memory layout + linker script that match the `rtt` feature
//!     (same pattern as `demo-firmware`).
```

with

```
//!  1. Select rza1-rt's `link.x` and raise the program stack for the VM.
```

and drop the `use std::fs;` and `use std::path::PathBuf;` imports if the compiler reports them unused.

- [ ] **Step 7: `cargo deluge new` stops scaffolding memory layouts**

`tools/cargo-deluge/src/new.rs`: delete the lines

```rust
const TPL_MEMORY_X: &str = include_str!("../../../examples/baremetal/blinky/memory.x");
const TPL_MEMORY_RTT_X: &str = include_str!("../../../examples/baremetal/blinky/memory_rtt.x");
```

```rust
    write(&dir.join("memory.x"), TPL_MEMORY_X)?;
    write(&dir.join("memory_rtt.x"), TPL_MEMORY_RTT_X)?;
```

and in `scaffold_writes_expected_file_set` the two entries

```rust
            "memory.x",
            "memory_rtt.x",
```

then add, after that test's `for` loop over expected files (inside the same test function):

```rust
        // The board crate provides the memory layout; a scaffolded app carries none.
        assert!(!app.join("memory.x").exists());
        assert!(!app.join("memory_rtt.x").exists());
```

Run: `cargo test --target x86_64-unknown-linux-gnu --manifest-path tools/cargo-deluge/Cargo.toml 2>&1 | grep -E "test result|FAILED|error"`
Expected: `test result: ok.` for each binary, no `FAILED`.

- [ ] **Step 8: The layout is unchanged**

Run: `find . -name 'memory*.x' -not -path './target/*' -not -path '*/node_modules/*' | wc -l`
Expected: `2`.

Run: `tools/link-layout.sh check`
Expected: `==> Link layout matches tools/link-layout.baseline.` — including `wren-firmware PROGRAM_STACK_SIZE 00040000` and `wren-firmware+rtt PROGRAM_STACK_SIZE 00030000`, now from `--defsym`.

- [ ] **Step 9: A freshly scaffolded app links with the Deluge layout**

```bash
S=/tmp/claude-1000/phase3-scaffold; rm -rf "$S"; mkdir -p "$S"
(cd "$S" && cargo run -q --manifest-path /home/kate/GitHub/deluge-sdk/tools/cargo-deluge/Cargo.toml \
   --target x86_64-unknown-linux-gnu -- deluge new probe-app)
# Outside the checkout it depends on the published crate; point it at this tree instead.
sed -i 's|^deluge = .*|deluge = { package = "deluge-sdk", path = "/home/kate/GitHub/deluge-sdk/crates/deluge-sdk" }|' "$S/probe-app/Cargo.toml"
printf '\n[workspace]\n' >> "$S/probe-app/Cargo.toml"
ls "$S/probe-app"
(cd "$S/probe-app" && cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem 2>&1 | tail -2)
diff <(tools/link-layout.sh print "$S/probe-app/target/armv7a-none-eabihf/debug/probe-app" | cut -d' ' -f2-) \
     <(grep '^blinky ' tools/link-layout.baseline | cut -d' ' -f2-) && echo SAME-LAYOUT-AS-BLINKY
```

Expected: `ls` shows no `memory.x`; the build ends `Finished`; `SAME-LAYOUT-AS-BLINKY`. (If the scaffolded `Cargo.toml` already contains a `[workspace]` table, skip the `printf`; if the `deluge = …` line has another shape, edit it by hand to the path form shown.)

- [ ] **Step 10: The published BSP carries its layout**

Run: `cargo package -p deluge-bsp --list --allow-dirty --no-verify | grep -E "^(build.rs|memory.x|memory_rtt.x)$"`
Expected: all three names.

Run: `cargo package -p rza1-rt --list --allow-dirty --no-verify | grep -E "^(link.x|link_rtt.x)$"`
Expected: both names.

- [ ] **Step 11: Run the test suite**

Run: `tools/test.sh > /tmp/claude-1000/phase3-t3.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/phase3-t3.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `exit 0`, `failed 0`.

- [ ] **Step 12: Commit**

```bash
git add -A crates/rza1-rt crates/deluge-bsp examples firmwares hw-tests wren/wren-firmware tools/cargo-deluge/src/new.rs
git commit -m "refactor: deluge-bsp provides the memory layout; binaries link with -Tlink.x and carry no memory.x"
```

---

### Task 4: The status LED comes from the board

**Files:**
- Modify: `crates/deluge-bsp/memory.x`, `crates/deluge-bsp/memory_rtt.x`
- Modify: `crates/rza1-rt/link.x`, `crates/rza1-rt/link_rtt.x`
- Modify: `crates/rza1-rt/src/startup.rs:8-13` (module doc), `:306-344` (`_boot_led_setup`, `_boot_led_write_high`, `_boot_led_write_low`)
- Modify: `crates/rza1-rt/README.md`
- Modify: `tools/link-layout.baseline` (via `update`)

**Interfaces:**
- Consumes: `link.x`/`memory.x` from Task 3.
- Produces: `memory.x` contract gains `RZA1_STATUS_LED_PORT` (1–11) and `RZA1_STATUS_LED_PIN` (0–15). `link.x` defines `__rza1_status_led_p`, `__rza1_status_led_pm`, `__rza1_status_led_pmc`, `__rza1_status_led_pipc` (register addresses) and `__rza1_status_led_mask` (`1 << pin`). The LED routines clobber `r0`–`r2` (they clobbered `r0`–`r1`; every caller has `r2` dead — the reset handler calls them first thing, and `_boot_fault_loop` keeps its state in `r4`/`r5` and passes the delay in `r0`).

- [ ] **Step 1: Write the failing check**

The derived symbols the new code will need, against the values the current code hard-codes (`startup.rs:306-344`: PMC6 `0xFCFE3418`, PM6 `0xFCFE3318`, PIPC6 `0xFCFE7218`, P6 `0xFCFE3018`, bit 7):

Run: `tools/link-layout.sh print target/armv7a-none-eabihf/debug/blinky | grep __rza1_status_led; echo "lines: $(tools/link-layout.sh print target/armv7a-none-eabihf/debug/blinky | grep -c __rza1_status_led)"`
Expected: `lines: 0` (the failing state; after this task, five lines with the values above).

- [ ] **Step 2: The board names its LED**

In both `crates/deluge-bsp/memory.x` and `crates/deluge-bsp/memory_rtt.x`, append:

```
/* The status LED rza1-rt's startup blinks to report a boot fault: P6_7 on the Deluge. */
RZA1_STATUS_LED_PORT = 6;
RZA1_STATUS_LED_PIN  = 7;
```

- [ ] **Step 3: The script derives the registers**

In both `crates/rza1-rt/link.x` and `crates/rza1-rt/link_rtt.x`, add to the header's list of what `memory.x` must define, after the `INTERNAL_RAM_END` line:

```
 *   RZA1_STATUS_LED_PORT = ...; -- GPIO port of the status LED (1-11)
 *   RZA1_STATUS_LED_PIN  = ...; -- its pin (0-15)
```

and insert, directly after the `TTB_SIZE = 0x4000;` line:

```
/* The status LED the reset and fault handlers drive (startup.rs). The port's GPIO registers are
 * 16 bits wide and 4 bytes apart per port (RZ/A1L hardware manual, ch. 54: Pn, PMn, PMCn,
 * PIPCn; matches rza1l_hal::gpio). Port 0 is input-only, so it has no PM/PIPC register. */
ASSERT(DEFINED(RZA1_STATUS_LED_PORT) && DEFINED(RZA1_STATUS_LED_PIN),
       "memory.x must define RZA1_STATUS_LED_PORT and RZA1_STATUS_LED_PIN")
ASSERT(RZA1_STATUS_LED_PORT >= 1 && RZA1_STATUS_LED_PORT <= 11,
       "RZA1_STATUS_LED_PORT must be 1-11 (P1-P11)")
ASSERT(RZA1_STATUS_LED_PIN <= 15, "RZA1_STATUS_LED_PIN must be 0-15")
__rza1_status_led_p    = 0xFCFE3000 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_pm   = 0xFCFE3300 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_pmc  = 0xFCFE3400 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_pipc = 0xFCFE7200 + 4 * RZA1_STATUS_LED_PORT;
__rza1_status_led_mask = 1 << RZA1_STATUS_LED_PIN;
```

- [ ] **Step 4: Startup uses them**

In `crates/rza1-rt/src/startup.rs`, replace the routines from `_boot_led_setup:` through the `bx lr` that ends `_boot_led_write_low` with:

```
_boot_led_setup:
    /* The board's status LED (memory.x: RZA1_STATUS_LED_PORT/PIN; link.x derives the
     * registers) as a software-controlled GPIO output: its PMC bit (port mode), PM bit
     * (output) and PIPC bit (software control) cleared. Clobbers r0-r2. */
    ldr r2, =__rza1_status_led_mask
    ldr r0, =__rza1_status_led_pmc
    ldrh r1, [r0]
    bic r1, r1, r2
    strh r1, [r0]

    ldr r0, =__rza1_status_led_pm
    ldrh r1, [r0]
    bic r1, r1, r2
    strh r1, [r0]

    ldr r0, =__rza1_status_led_pipc
    ldrh r1, [r0]
    bic r1, r1, r2
    strh r1, [r0]
    bx lr

_boot_led_write_high:
    ldr r2, =__rza1_status_led_mask
    ldr r0, =__rza1_status_led_p
    ldrh r1, [r0]
    orr r1, r1, r2
    strh r1, [r0]
    bx lr

_boot_led_write_low:
    ldr r2, =__rza1_status_led_mask
    ldr r0, =__rza1_status_led_p
    ldrh r1, [r0]
    bic r1, r1, r2
    strh r1, [r0]
    bx lr
```

Before saving, read every `bl _boot_led_` call site (`grep -n "_boot_led_\|_boot_fault_loop" crates/rza1-rt/src/startup.rs`) and confirm none of them reads `r2` after the call before writing it; if one does, stop and record a ruling.

In the module doc (lines 8-13) replace `it
//! blinks the P6_7 LED in a repeating pattern` with `it
//! blinks the board's status LED (named in its memory.x) in a repeating pattern`, and in the reset handler comment `Configure the heartbeat LED pin as a plain GPIO` read `Configure the board's status LED as a plain GPIO`.

In `crates/rza1-rt/README.md`, after the sentence listing what `memory.x` defines, add: `It also names the status LED the boot and fault handlers blink: \`RZA1_STATUS_LED_PORT\` (1–11) and \`RZA1_STATUS_LED_PIN\` (0–15).`

- [ ] **Step 5: The derived symbols equal the old literals**

Run: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p blinky 2>&1 | tail -1 && tools/link-layout.sh print target/armv7a-none-eabihf/debug/blinky | grep __rza1_status_led`
Expected:

```
blinky __rza1_status_led_mask 00000080
blinky __rza1_status_led_p fcfe3018
blinky __rza1_status_led_pipc fcfe7218
blinky __rza1_status_led_pm fcfe3318
blinky __rza1_status_led_pmc fcfe3418
```

Run: `arm-none-eabi-objdump -d target/armv7a-none-eabihf/debug/blinky | grep -A12 "<_boot_led_setup>:" | head -14`
Expected: three `ldrh`/`bic`/`strh` groups, each after an `ldr r0, [pc, …]` whose literal-pool comment shows `0xfcfe3418`, `0xfcfe3318`, `0xfcfe7218` in that order, and `ldr r2` of `0x00000080`.

- [ ] **Step 6: A bad or missing LED fails the link with a named message**

```bash
cp crates/deluge-bsp/memory.x /tmp/claude-1000/memory.x.bak
sed -i 's/RZA1_STATUS_LED_PIN  = 7;/RZA1_STATUS_LED_PIN  = 16;/' crates/deluge-bsp/memory.x
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p blinky 2>&1 | grep -m1 -o "RZA1_STATUS_LED_PIN must be 0-15"
sed -i 's/RZA1_STATUS_LED_PORT = 6;/RZA1_STATUS_LED_PORT = 0;/; s/RZA1_STATUS_LED_PIN  = 16;/RZA1_STATUS_LED_PIN  = 7;/' crates/deluge-bsp/memory.x
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p blinky 2>&1 | grep -m1 -o "RZA1_STATUS_LED_PORT must be 1-11 (P1-P11)"
grep -v "RZA1_STATUS_LED" /tmp/claude-1000/memory.x.bak > crates/deluge-bsp/memory.x
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p blinky 2>&1 | grep -m1 -o "RZA1_STATUS_LED_[A-Z]*"
cp /tmp/claude-1000/memory.x.bak crates/deluge-bsp/memory.x
git diff --stat crates/deluge-bsp/memory.x
```

Expected: the first two greps print their messages; the third prints `RZA1_STATUS_LED_PORT` or `RZA1_STATUS_LED_PIN` (lld reports either the ASSERT message or the undefined symbol, both naming it); the final `git diff --stat` shows the file only with Step 2's addition (no stray edits).

- [ ] **Step 7: Update the baseline, and check only the LED symbols changed**

Run: `tools/link-layout.sh update && git diff -U0 tools/link-layout.baseline | grep -E "^[-+][^-+]" | grep -vc __rza1_status_led`
Expected: `0` — every added or removed line is an `__rza1_status_led_*` line (29 configurations × 5 symbols added, none removed).

Run: `tools/link-layout.sh check`
Expected: matches.

- [ ] **Step 8: Run the test suite and commit**

Run: `tools/test.sh > /tmp/claude-1000/phase3-t4.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

```bash
git add crates/deluge-bsp/memory.x crates/deluge-bsp/memory_rtt.x crates/rza1-rt tools/link-layout.baseline
git commit -m "feat(rza1-rt): the board names its status LED in memory.x instead of startup hard-coding P6_7"
```

---

### Task 5: The OSTM time driver behind `time-driver-ostm`

**Files:**
- Modify: `crates/rza1l-hal/Cargo.toml` (`[features]`)
- Modify: `crates/rza1l-hal/src/lib.rs:44-49`
- Modify: `crates/deluge-bsp/Cargo.toml` (the `rza1l-hal` dependency)
- Modify: `crates/rza1l-hal/README.md:55`

**Interfaces:**
- Produces: `rza1l-hal` feature `time-driver-ostm`, in `default`; `rza1l_hal::time_driver` exists only with it (and `target_os = "none"`). `deluge-bsp` names the feature explicitly.

- [ ] **Step 1: Write the failing check**

```bash
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p rza1l-hal --no-default-features 2>&1 | tail -1
arm-none-eabi-nm --defined-only target/armv7a-none-eabihf/debug/librza1l_hal.rlib 2>/dev/null | grep -c " T _embassy_time_now$"
```

Expected: `Finished`, then `1` — the driver is registered even with default features off (the failing state).

- [ ] **Step 2: Add the feature and gate the module**

`crates/rza1l-hal/Cargo.toml`, `[features]`, add at the top of the table:

```toml
default = ["time-driver-ostm"]
## The Embassy time driver on OSTM0 (free-running count) and OSTM1 (alarm): `time_driver`.
## Turn default features off to bring your own driver.
time-driver-ostm = []
```

`crates/rza1l-hal/src/lib.rs`: replace

```rust
#[cfg(target_os = "none")]
pub mod time_driver;
```

with

```rust
#[cfg(all(target_os = "none", feature = "time-driver-ostm"))]
pub mod time_driver;
```

and append to the comment above it: `It is behind the \`time-driver-ostm\` feature (on by default) so an application can register its own driver instead.`

`crates/deluge-bsp/Cargo.toml`: the HAL dependency becomes

```toml
# `time-driver-ostm`: `system::init` starts the OSTM time driver, so the BSP needs it even if a
# dependent turns rza1l-hal's default features off.
rza1l-hal = { path = "../rza1l-hal", version = "0.1.0", features = ["time-driver-ostm"] }
```

`crates/rza1l-hal/README.md:55`: the `time_driver` row's text ends with ` Behind the \`time-driver-ostm\` feature (on by default).`

- [ ] **Step 3: Run the check again, both ways**

```bash
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p rza1l-hal --no-default-features 2>&1 | tail -1
arm-none-eabi-nm --defined-only target/armv7a-none-eabihf/debug/librza1l_hal.rlib 2>/dev/null | grep -c " T _embassy_time_now$"
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p rza1l-hal 2>&1 | tail -1
arm-none-eabi-nm --defined-only target/armv7a-none-eabihf/debug/librza1l_hal.rlib 2>/dev/null | grep -c " T _embassy_time_now$"
```

Expected: `Finished`, `0`, `Finished`, `1`.

- [ ] **Step 4: Layout, suite, commit**

Run: `tools/link-layout.sh check && tools/test.sh > /tmp/claude-1000/phase3-t5.log 2>&1; echo "exit $?"`
Expected: matches; `exit 0`.

```bash
git add crates/rza1l-hal crates/deluge-bsp/Cargo.toml Cargo.lock
git commit -m "feat(rza1l-hal): the OSTM time driver is the default-on time-driver-ostm feature"
```

---

### Task 6: `deluge-alloc` becomes `rza1-alloc`

**Files:**
- Move: `crates/deluge-alloc/` → `crates/rza1-alloc/`
- Modify: `crates/rza1-alloc/Cargo.toml`, `crates/rza1-alloc/README.md`, `crates/rza1-alloc/src/lib.rs` (doc text only)
- Modify: `Cargo.toml` (workspace member and `[workspace.dependencies]` entry if present)
- Modify: every manifest and source naming `deluge-alloc`/`deluge_alloc` (Step 3's grep lists them): `crates/deluge-sdk/{Cargo.toml,src/lib.rs,README.md}`, `firmwares/{app-loader,controller-firmware,msc-firmware}/{Cargo.toml,src/main.rs}`, `hw-tests/{sd-bench,uac-host-firmware,vbus-probe,wp-probe}/{Cargo.toml,src/main.rs}`, `wren/wren-sys/{Cargo.toml,src/lib.rs}`, `README.md`, `CONTRIBUTING.md`, `docs/advanced-guide.md`, `docs/audio-graph-vs-scsynth.md`, `docs/web-editor-plan.md`, `crates/rza1l-hal/README.md`
- Not modified: anything under `docs/superpowers/` (dated records)

**Interfaces:**
- Produces: crate `rza1-alloc` (lib `rza1_alloc`), same items (`CsHeap`, `SRAM`, `SDRAM`, `Spill`). The facade keeps `deluge::deluge_alloc` as `pub use rza1_alloc as deluge_alloc;`.

- [ ] **Step 1: Write the failing check**

Run: `cargo metadata --format-version 1 --no-deps | python3 -c "import json,sys; print(sorted(p['name'] for p in json.load(sys.stdin)['packages'] if 'alloc' in p['name']))"`
Expected: `['deluge-alloc']` (after this task: `['rza1-alloc']`).

- [ ] **Step 2: Rename the crate**

```bash
git mv crates/deluge-alloc crates/rza1-alloc
```

`crates/rza1-alloc/Cargo.toml`: `name = "deluge-alloc"` → `name = "rza1-alloc"`, and the description becomes `"Critical-section-guarded SRAM/SDRAM heap allocators for the Renesas RZ/A1L."`.

In `crates/rza1-alloc/README.md` and the crate docs in `src/lib.rs`, replace `deluge-alloc`/`deluge_alloc` with `rza1-alloc`/`rza1_alloc`. Where they describe the Deluge's own sizes or call the heaps "the Deluge's", keep the sentence but say "on the Deluge" as an example (rule 1 allows the Deluge in comments, not in code).

- [ ] **Step 3: Update every user**

```bash
grep -rl "deluge-alloc\|deluge_alloc" --exclude-dir=target --exclude-dir=.git --exclude-dir=superpowers . \
  | grep -v "Cargo.lock$" | sort
```

For each file printed, other than `crates/deluge-sdk/src/lib.rs`:

```bash
sed -i 's/deluge-alloc/rza1-alloc/g; s/deluge_alloc/rza1_alloc/g' <file>
```

`crates/deluge-sdk/src/lib.rs`: replace `deluge_alloc` with `rza1_alloc` everywhere except the facade re-export, which becomes

```rust
#[cfg(target_os = "none")]
pub use rza1_alloc as deluge_alloc;
```

with the comment above it gaining: `It stays \`deluge::deluge_alloc\` for apps; the crate is \`rza1-alloc\`.`

In `docs/advanced-guide.md`, the user-facing snippet keeps `use deluge::deluge_alloc::SDRAM;` (the facade path); revert the sed on that one line if it changed it, and make the surrounding prose say the heaps live in the `rza1-alloc` crate, reached through `deluge::deluge_alloc`.

`Cargo.toml` (workspace): the member `"crates/deluge-alloc"` → `"crates/rza1-alloc"`; if `[workspace.dependencies]` has `deluge-alloc = …`, rename the key and its path.

- [ ] **Step 4: Run the check again, and build**

Run: `cargo metadata --format-version 1 --no-deps | python3 -c "import json,sys; print(sorted(p['name'] for p in json.load(sys.stdin)['packages'] if 'alloc' in p['name']))"`
Expected: `['rza1-alloc']`.

Run: `grep -rn "deluge-alloc\|deluge_alloc" --exclude-dir=target --exclude-dir=.git --exclude-dir=superpowers . | grep -v "Cargo.lock:" | grep -v "as deluge_alloc\|deluge::deluge_alloc"`
Expected: no output.

Run: `tools/link-layout.sh check`
Expected: matches (it builds every firmware, including all that use the allocator).

- [ ] **Step 5: The facade path still resolves for an app**

The check uses the throwaway app from Task 3 Step 9 (recreate it with those commands if `/tmp/claude-1000/phase3-scaffold/probe-app` is gone), so no repo file carries test-only code:

```bash
grep -n "pub use rza1_alloc as deluge_alloc" crates/deluge-sdk/src/lib.rs
grep -rn "deluge::deluge_alloc" docs/advanced-guide.md
```

Expected: one line each. Then build the scaffolded app from Task 3 Step 9 after adding to its `src/main.rs`, at the end of the file:

```rust
#[allow(dead_code)]
fn facade_alloc_path_resolves() -> &'static deluge::deluge_alloc::CsHeap {
    &deluge::deluge_alloc::SDRAM
}
```

Run: `(cd /tmp/claude-1000/phase3-scaffold/probe-app && cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem 2>&1 | tail -1)`
Expected: `Finished`. (If `CsHeap` is not the type of `SDRAM`, use the type `crates/rza1-alloc/src/lib.rs:299` declares.)

- [ ] **Step 6: Refresh the excluded workspaces' lock files**

Run: `cargo tree --manifest-path wren/wren-web/Cargo.toml --target wasm32-wasip1 > /dev/null 2>&1; git status --short wren/wren-web/Cargo.lock`
Expected: `M wren/wren-web/Cargo.lock` with `deluge-alloc` replaced by `rza1-alloc` (`git diff wren/wren-web/Cargo.lock | grep alloc`). If `cargo tree` cannot resolve offline, leave the lock and record a ruling (it updates on the next `tools/test.sh` that has the wasm target).

- [ ] **Step 7: Suite and commit**

Run: `tools/test.sh > /tmp/claude-1000/phase3-t6.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

```bash
git add -A
git commit -m "refactor: deluge-alloc becomes rza1-alloc; the facade keeps deluge::deluge_alloc"
```

(Before `git add -A`, `git status --short` must show only the rename and the files from Step 3/6.)

---

### Task 7: Layering rules, docs and the final check

**Files:**
- Modify: `tools/check-layering.sh`
- Modify: `CHANGELOG.md` (`## [Unreleased]`)
- Modify: `README.md` (layout table), `crates/deluge-bsp/README.md` (features table, a line on `memory.x`), `docs/advanced-guide.md` (§2 startup path, §6 "Layout (`memory.x`)")
- Modify: `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (a "Done" note under Phase 3)

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Write the failing probes**

```bash
echo 'core::arch::global_asm!("nop");' >> crates/rza1l-hal/src/adc.rs
tools/check-layering.sh; echo "exit $?"
git checkout crates/rza1l-hal/src/adc.rs
echo 'const BOARD: &str = "deluge";' >> crates/rza1-rt/src/lib.rs
tools/check-layering.sh; echo "exit $?"
git checkout crates/rza1-rt/src/lib.rs
```

Expected: both runs `exit 0` (the failing state: neither violation is caught yet).

- [ ] **Step 2: Extend the rules**

In `tools/check-layering.sh`, replace the HAL block

```bash
# The chip HAL is board-agnostic: no Deluge names in its code (comments may cite the Deluge as an
# example). Its dependencies are checked against Cargo's graph at the end.
HAL_DIR=crates/rza1l-hal
# The whole crate directory, so code and linker scripts are covered wherever they live.
hal_code_hits="$(grep -rniE --include='*.rs' --include='*.x' 'deluge' "$HAL_DIR" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' || true)"
if [ -n "$hal_code_hits" ]; then
  printf '%s\n' "$hal_code_hits"
  echo "error: rza1l-hal names the Deluge; board facts belong in deluge-bsp" >&2
  exit 1
fi
```

with

```bash
# The chip crates are board-agnostic: no Deluge names in their code or linker scripts (comments
# may cite the Deluge as an example). Their dependencies are checked against Cargo's graph at the
# end.
CHIP_DIRS=(crates/rza1l-hal crates/rza1-rt crates/rza1-alloc)
chip_code_hits="$(grep -rniE --include='*.rs' --include='*.x' 'deluge' "${CHIP_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' || true)"
if [ -n "$chip_code_hits" ]; then
  printf '%s\n' "$chip_code_hits"
  echo "error: a chip crate (rza1l-hal, rza1-rt, rza1-alloc) names the Deluge; board facts belong in deluge-bsp" >&2
  exit 1
fi
# Startup and exception entry are rza1-rt's: the HAL has no global_asm! (spec, Phase 3 exit).
hal_asm_hits="$(grep -rn --include='*.rs' 'global_asm' crates/rza1l-hal | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$hal_asm_hits" ]; then
  printf '%s\n' "$hal_asm_hits"
  echo "error: rza1l-hal contains global_asm!; startup and exception entry belong in rza1-rt" >&2
  exit 1
fi
```

and in the Python `RULES` list, after the `crates/rza1l-hal/` entry, add:

```python
    ("crates/rza1-rt/", lambda n: n.startswith("deluge-"),
     "rza1-rt depends on a deluge-* crate; board facts belong in deluge-bsp"),
    ("crates/rza1-alloc/", lambda n: n.startswith("deluge-"),
     "rza1-alloc depends on a deluge-* crate; board facts belong in deluge-bsp"),
```

Update the script's header comment: the second bullet becomes `The chip crates (rza1l-hal, rza1-rt, rza1-alloc) never name the Deluge, and the HAL has no global_asm! (startup is rza1-rt's).`

- [ ] **Step 3: Run the probes again**

Repeat Step 1's commands.
Expected: the first prints the `adc.rs` line and `error: rza1l-hal contains global_asm!…`, `exit 1`; the second prints the `lib.rs` line and `error: a chip crate … names the Deluge…`, `exit 1`. Then `tools/check-layering.sh` on the clean tree: `==> Layering check passed.` (If a real line in `startup.rs` or `rza1-alloc` trips the Deluge rule, it is code, not a comment: reword it or move the fact to deluge-bsp, and say which in the commit message.)

- [ ] **Step 4: Docs**

`CHANGELOG.md`, under `## [Unreleased]` → `### Changed`, add at the top:

```markdown
- The runtime moved out of `rza1l-hal` into the new `rza1-rt` crate: the reset vector and
  bootloader metadata, the reset, fault and IRQ entry handlers, and the linker script, now
  `link.x`. **Breaking:** binaries link with `-Tlink.x` (`cargo:rustc-link-arg-bins=-Tlink.x`
  in `build.rs`) instead of `-Trza1l.x`/`-Trza1l_rtt.x`; the `rtt` and `app-fault-reporter`
  features moved from `rza1l-hal` to `rza1-rt` and are reached as `deluge-bsp/rtt` and
  `deluge-bsp/app-fault-reporter`.
- `deluge-bsp` provides the Deluge's memory layout: its `build.rs` puts `memory.x` (the RTT form
  with the `rtt` feature) on the linker search path. **Breaking:** delete your app's `memory.x`
  and `memory_rtt.x` and reduce its `build.rs` to the `-Tlink.x` line (`cargo deluge new` now
  scaffolds that). The stack sizes are `PROVIDE`d: override one with
  `cargo:rustc-link-arg-bins=--defsym=PROGRAM_STACK_SIZE=<bytes>`.
- The boot and fault blink use the status LED the board's `memory.x` names
  (`RZA1_STATUS_LED_PORT`/`_PIN`, P6_7 on the Deluge) instead of a hard-coded pin.
- `rza1l_hal::time_driver` is behind the `time-driver-ostm` feature, on by default.
- `deluge-alloc` is renamed `rza1-alloc`. Apps using `deluge::deluge_alloc` are unaffected.
```

`README.md`: in the crate layout table, add a row for `crates/rza1-rt` ("Runtime for the RZ/A1L: reset vector, exception entry, linker script") next to `rza1l-hal`, and rename the `deluge-alloc` row to `rza1-alloc`.

`crates/deluge-bsp/README.md`: in the features table add

```markdown
| `rtt` | SEGGER RTT: the RTT memory layout and fault dumps to RTT channel 0 (turns on `rza1-rt/rtt`) |
| `app-fault-reporter` | Hand CPU faults to the application's `rza1_cpu_fault_report` (turns on `rza1-rt/app-fault-reporter`) |
```

and after the table: `The crate's \`build.rs\` puts the Deluge's memory layout (\`memory.x\`, or \`memory_rtt.x\` with \`rtt\`) on the linker search path for \`rza1-rt\`'s \`link.x\`.`

`docs/advanced-guide.md` §2: the startup link becomes [`crates/rza1-rt/src/startup.rs`](../crates/rza1-rt/src/startup.rs). §6 "Layout (`memory.x`)": replace the first paragraph's "The scaffold's `memory.x` (and the firmware variants) place:" with "`deluge-bsp`'s `memory.x` places:" and the RTT paragraph with:

```markdown
With the `rtt` feature, `deluge-bsp` uses `memory_rtt.x` instead, which also reserves the RTT
buffer, and `rza1-rt` publishes the RTT form of `link.x`. An app's `build.rs` only selects the
script (`-Tlink.x`); to change a stack size, override the `PROVIDE`d symbol with
`cargo:rustc-link-arg-bins=--defsym=PROGRAM_STACK_SIZE=<bytes>`, as `wren-firmware` does. The
runtime reads `__sram_heap_start` / `__sram_heap_end` from the script.
```

Spec (`docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md`), directly after the Phase 3 **Exit:** line:

```markdown
**Done (2026-10-15 plan):** `rza1-rt` owns all of the former `startup.rs` (vectors, reset, fault
and IRQ entry) and `link.x`; `deluge-bsp`'s `build.rs` provides `memory.x`, and all 46 per-binary
`memory.x`/`memory_rtt.x` files are gone. Overrides are `--defsym`s of the `PROVIDE`d stack sizes
(wren's program stack), not per-binary `memory.x` files; app-loader's layout was the default.
`unlock-bootloader` stayed in the HAL: it gates `spibsc`'s flash unlock, not startup. The boot and
fault LED became board data (`RZA1_STATUS_LED_PORT`/`_PIN` in `memory.x`). `deluge-alloc` was not
on crates.io, so it was renamed to `rza1-alloc` without a re-export crate.
```

- [ ] **Step 5: Final verification**

Run: `tools/test.sh > /tmp/claude-1000/phase3-final.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/phase3-final.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `exit 0`, `failed 0`.

Run: `tools/link-layout.sh check`
Expected: `==> Link layout matches tools/link-layout.baseline.`

Run: `tools/check-manifests.sh; echo "exit $?"`
Expected: `exit 0`.

Run: `grep -rln global_asm crates/rza1l-hal; find . -name 'memory*.x' -not -path './target/*' -not -path '*/node_modules/*'`
Expected: no HAL files; only `crates/deluge-bsp/memory.x` and `crates/deluge-bsp/memory_rtt.x`.

Simulator unchanged: from `examples/baremetal/oled_hello`, run the headless snapshot

```bash
W=/tmp/claude-1000/phase3-sim; rm -rf $W; mkdir -p $W
(cd examples/baremetal/oled_hello && timeout 300 cargo run -q --manifest-path ../../../tools/cargo-deluge/Cargo.toml \
   --target x86_64-unknown-linux-gnu -- deluge sim --headless --out $W/now > $W/now.log 2>&1; echo "exit $?")
cat $W/now/frame.state
```

Expected: `exit 0`, and `frame.state` reads `cv 0 0 0 0` / `gate 0 0 0 0` / `synced 0`.

- [ ] **Step 6: Commit**

```bash
git add tools/check-layering.sh CHANGELOG.md README.md crates/deluge-bsp/README.md docs/advanced-guide.md \
  docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md
git commit -m "chore(layering): rza1-rt and rza1-alloc are chip crates; the HAL has no global_asm. Docs for Phase 3"
```

- [ ] **Step 7: Hardware smoke test (the human partner's merge gate)**

List these in the final message; the executor cannot run them:

1. `blinky` boots and blinks (reset path, stacks, BSS, the moved vector table).
2. `deluge-controller` boots, plays audio, and USB MIDI works (IRQ entry stub and `gic_dispatch`).
3. `wren-firmware` boots and runs a recursion-heavy script (its 256 KB program stack via `--defsym`).
4. An RTT build (`cargo build-fw -p blinky --features rtt`) shows RTT output on the probe.
5. A deliberate fault (for example, a test build that dereferences `0xFFFF_FFF0` in `main`) blinks the status LED 4 times (data abort) — the board-provided LED registers.
6. `app-loader` boots and its MSC mode mounts.
