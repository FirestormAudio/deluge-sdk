# rza1-hal Phase 7: rza1-rt Macros, Boot Header and Fault Hook Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rza1-rt` gets cortex-m-rt-style attribute macros: `#[entry]`, `#[irq]` and `#[fault]`. The Deluge's boot header (the FSB words and the `.BootLoad_ValidProgramTest.` signature) and its status-LED fault blink move out of the runtime into `deluge-bsp`, through a user-provided `.boot_header` linker section and the fault hook.

**Architecture:**
- **Macros.** A new proc-macro crate, `rza1/rza1-rt-macros`, type-checks and exports the symbols the assembly already calls: `main`, `gic_dispatch` and `rza1_cpu_fault_report`. `rza1-rt` re-exports the macros along with a `FaultInfo` type.
- **Runtime.** The vector table becomes 8 relative branches, so no literal pool can sit between it and the header. `link.x` places `*(.boot_header)` straight after it, at `_start + 0x20`. The fault path always calls `rza1_cpu_fault_report`; a weak default parks the core.
- **Board.** `deluge-bsp` emits the Deluge header from `global_asm!` and, behind a default feature, blinks P6_7 from a `#[fault]` handler.

**Tech Stack:** Rust 2024; `syn` 2 / `quote` / `proc-macro2`, whichever versions `crates/deluge-sdk-macros` already locks; ARM `global_asm!`; GNU ld scripts; python3 for the image check.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md`. Relevant parts:
- decision 4;
- problem table row 3;
- §5 item 7: "Bench check: cold boot through the app-loader, fault blink".

## Global Constraints

- **Decision 4, verbatim:**
  - "`#[entry]` (type-checked `fn() -> !`), an IRQ/exception attribute and a fault-handler hook."
  - "The image signature or boot header becomes a user-provided linker section."
  - "`deluge-bsp`, or the app-loader, supplies the `.BootLoad_ValidProgramTest.` header, the bootloader words and the status-LED fault blink."
- **Layering:** nothing under `rza1/` names the Deluge in code (`tools/check-layering.sh`). This phase adds rules that fail if `rza1/` contains `BootLoad_ValidProgramTest` or `RZA1_STATUS_LED`.
- **Preserved, byte for byte, at `_start + 0x20`** (Deluge images):
  - `.word _start` (code_start);
  - `.word __metadata_code_end` (code_end);
  - `.word _start` (code_execute);
  - `.asciz ".BootLoad_ValidProgramTest."`, then `.align 4`.
  - `deluge-image` (`FSB_*` constants, `validate_fsb_metadata`) and the app-loader (`flashboot.rs`) read exactly this.
- **Preserved ABIs**, because DelugeFirmware links against them in phase 9:
  - `main` is called by `bl main` after BSS is zeroed, in SYS mode, with the MMU and caches off and IRQ/FIQ/abort masked.
  - `gic_dispatch(icciar: u32)` is C ABI.
  - `rza1_cpu_fault_report(fault_pc: u32, fault_type: u32, sys_lr: u32, sys_sp: u32) -> !` is C ABI, runs on the abort stack, and fault_type is 0 undef, 1 prefetch, 2 data, 3 reserved, 0xF svc.
  - The memory.x contract minus `RZA1_STATUS_LED_*`.
  - The output-section names `.bss` and `.ARM.exidx`.
  - `__sram_heap_base`, and the stack and heap symbols.
- **Features:**
  - `rza1-rt`'s `app-fault-reporter` goes, because the hook is always called.
  - `deluge-bsp` gets `fault-blink` (default on), defining the `#[fault]` handler.
  - `deluge-bsp`'s `app-fault-reporter` stays as an empty, documented compatibility feature until phase 9. DelugeFirmware passes it and turns default features off, so it gets no blink and keeps its own C handler.
- **Done when:**
  - `verify.sh` prints PASS for every line, including the new image check below;
  - `rza1/tools/test.sh` passes, including the macro crate's host tests;
  - the Task 4 bench check passes.

## Review Focus

1. **The boot header's place in every image.** Any change to the vector table (a literal pool, alignment, section order) moves the header off `_start + 0x20`, and the FSB then refuses the flash image. Expected: `tools/check-boot-header.py` checks every built Deluge ELF and runs from `tools/link-layout.sh check`. Test: the Task 2 image check over all 23 + 4 binaries.
2. **An `#[entry]`, `#[irq]` or `#[fault]` with the wrong signature** (arguments on entry, a returning entry, async, generics, `unsafe` missing on `#[irq]`, a fault handler that can return). Expected: a `compile_error!` naming the expected signature, not a link error or silent ABI mismatch. Tests: `entry_rejects_*`, `irq_rejects_*`, `fault_rejects_*` (Task 1).
3. **Two fault handlers in one image** (the BSP's blink and an app's own `#[fault]`). Expected: a link error on the duplicate `rza1_cpu_fault_report`, and the `fault-blink` doc says to turn the feature off. Test: none automatable; documented on the feature (Task 3).
4. **A prefetch abort with `rtt` on.** Today it bypasses the hook (`startup.rs:771`). Expected: it reaches `rza1_cpu_fault_report` like the others. Test: `tools/check-boot-header.py --rtt` checks that `_pabt_rtt` branches to `_fault_report` in every RTT ELF (Task 2).
5. **An image with no boot header** (a non-Deluge board). Expected: it links: `.boot_header` is optional and nothing ASSERTs it. Test: none; every image today is built on `deluge-bsp`, so the review checks `link.x` by reading it.

---

### Task 1: `rza1-rt-macros` — `#[entry]`, `#[irq]`, `#[fault]`

**Files:**
- Create:
  - `rza1/rza1-rt-macros/{Cargo.toml, src/lib.rs}`, with `[lib] proc-macro = true`, edition 2024, MIT OR Apache-2.0 like its siblings;
  - in `rza1-rt`: `src/fault.rs` (`FaultInfo`, `FaultKind`).
- Modify:
  - `rza1/Cargo.toml` (members);
  - `rza1/rza1-rt/{Cargo.toml, src/lib.rs}` (depend on the macros, re-export them);
  - `rza1/tools/test.sh` (`cargo test -p rza1-rt-macros` on the host target).

**Interfaces:**
- Produces:
  - `#[rza1_rt::entry]` on `fn NAME() -> !`. It expands to the user function unchanged (renamed `__rza1_rt_entry_NAME`, private) plus:
    ```rust
    #[unsafe(export_name = "main")]
    pub extern "C" fn __rza1_rt_main() -> ! {
        let f: fn() -> ! = __rza1_rt_entry_NAME;
        f()
    }
    ```
    It refuses arguments, a return type other than `!`, `async`, `unsafe`, generics, and `extern`.
  - `#[rza1_rt::irq]` on `unsafe fn NAME(icciar: u32)`. It exports `gic_dispatch` (C ABI) calling it. For an image that turns rza1-hal's `rt` feature off and dispatches interrupts itself. It refuses a missing `unsafe`, any other parameter list, and a return type.
  - `#[rza1_rt::fault]` on `fn NAME(info: &rza1_rt::FaultInfo) -> !`. It exports `rza1_cpu_fault_report(pc, kind, lr, sp)` (C ABI), building the `FaultInfo` and calling it. It refuses a return type other than `!` and any other parameter list.
  - `rza1-rt`:
    - `pub use rza1_rt_macros::{entry, fault, irq};`
    - `#[repr(C)] pub struct FaultInfo { pub pc: u32, pub kind: FaultKind, pub lr: u32, pub sp: u32 }`
    - `#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum FaultKind { Undefined, PrefetchAbort, DataAbort, Reserved, Svc, Unknown(u32) }` with `pub fn from_raw(raw: u32) -> Self` (0, 1, 2, 3 and 0xF; anything else is `Unknown`).
    - `impl FaultInfo { pub fn fault_address(&self) -> Option<u32> }`: DFAR for a data abort and IFAR for a prefetch abort (CP15 reads, target only; `None` on the host).
  - The macro crate's own testable core: `fn expand_entry(attr: TokenStream2, item: TokenStream2) -> TokenStream2`, with matching `expand_irq` and `expand_fault`. The `#[proc_macro_attribute]` fns forward to them.
- [ ] **Step 1: Write the failing tests** in `rza1-rt-macros/src/lib.rs` (a host `#[cfg(test)]` module calling `expand_*` on `quote!` input and asserting on `to_string()`):
  - `entry_exports_main_and_type_checks`: the output of `fn main() -> ! { loop {} }` contains `export_name = "main"` and `fn () -> !`.
  - `entry_rejects_arguments`, `entry_rejects_a_return_value` (`fn main() {}`), `entry_rejects_async`: each output contains `compile_error` and the text `` `#[entry]` expects `fn() -> !` ``.
  - `irq_exports_gic_dispatch`; `irq_rejects_a_safe_fn` (the error text contains `unsafe fn(u32)`).
  - `fault_exports_the_report_symbol`: it contains `export_name = "rza1_cpu_fault_report"` and `extern "C"`. `fault_rejects_a_returning_handler`.
  - `fault_kind_from_raw` in `rza1-rt/src/fault.rs` (host): `from_raw(2) == DataAbort`, `from_raw(0xF) == Svc`, `from_raw(7) == Unknown(7)`. Set `test = true` in `rza1-rt/Cargo.toml`; `startup` is already target-only.
- [ ] **Step 2: Run them to verify they fail**
  Run: `cd rza1 && cargo test -p rza1-rt-macros --target x86_64-unknown-linux-gnu`
  Expected: the package is not found, or the `expand_*` functions are not found.
- [ ] **Step 3: Implement** as in Interfaces. Use `syn` 2 and `quote`. Check `crates/deluge-sdk-macros/Cargo.toml` for the locked versions and reuse them, so `--offline` resolves.
- [ ] **Step 4: Run the tests and verify**
  Run: Step 2's command, `cd rza1 && cargo test -p rza1-rt --target x86_64-unknown-linux-gnu`, then `verify.sh`.
  Expected: PASS. Nothing uses the macros yet.
- [ ] **Step 5: Commit** `rza1-rt: entry, irq and fault attribute macros`

### Task 2: The boot header and LED leave `rza1-rt`; `deluge-bsp` supplies the header

**Files:**
- Modify:
  - `rza1/rza1-rt/src/startup.rs`;
  - `rza1/rza1-rt/{link.x, link_rtt.x, Cargo.toml (drop app-fault-reporter), README.md}`;
  - `crates/deluge-bsp/src/lib.rs` (a new `boot_header` module, target-only, `global_asm!`);
  - `crates/deluge-bsp/{memory.x, memory_rtt.x}` (drop `RZA1_STATUS_LED_*`);
  - `crates/deluge-bsp/Cargo.toml` (`app-fault-reporter = []`, documented as kept for DelugeFirmware until phase 9);
  - `tools/check-layering.sh`;
  - `tools/link-layout.sh`: drop the `__rza1_status_led_*` symbols from `SYMS`; regenerate `tools/link-layout.baseline` with `tools/link-layout.sh update` (or the script's documented regenerate mode). The diff must show only the LED lines removed.
- Create: `tools/check-boot-header.py`, run by `tools/link-layout.sh check` on every ELF it builds.

**Interfaces:**
- `startup.rs`:
  - The vector table is `b _reset_handler`, `b _undef_handler`, …, `b _fiq_handler`: 8 branches, 0x20 bytes, no literal pool in `.vector_table`. The FSB words, the signature, the `.ltorg` and the comment block describing them go.
  - `_boot_led_setup`/`_boot_led_write_*`/`_boot_delay`/`_boot_fault_loop` go, and so do the reset handler's two LED calls. P6_7 stays a reset-default input until the board drives it; the Task 4 bench confirms the LED is dark from power-on to `Board::init`.
  - `_fault_report` always loads `rza1_cpu_fault_report` (the `app-fault-reporter` cfg goes).
  - A weak default: `.weak rza1_cpu_fault_report` defined as `wfi; b` (park). The reset handler's other steps are unchanged.
  - `_pabt_rtt` ends in `b _fault_report` (fixing the bypass).
  - The `r4` blink counts are dropped, and r5/r6 are kept as the hook's arguments.
  - The comment "Branch to rust_main" becomes "Branch to `main` (`#[rza1_rt::entry]`)".
- `link.x` / `link_rtt.x`:
  - `.vector_table : ALIGN(0x20) { KEEP(*(.vector_table)) KEEP(*(.boot_header)) } > RAM`, with a comment: "a board's boot header lands at `_start + 0x20`; optional".
  - The `RZA1_STATUS_LED_*` requirement, its ASSERTs and the `__rza1_status_led_*` symbols go.
- `deluge-bsp`'s `boot_header` module emits, in `.section .boot_header, "a"`, the four items in Global Constraints, then `.align 4`. Every image built on the BSP gets it; `deluge-bsp` already links `rza1-rt`.
- `tools/check-boot-header.py ELF [--rtt]`: plain-python ELF32 parsing (section headers, symtab). It reads the bytes at `_start`'s file offset + 0x20 and checks:
  - word 0 == `_start`;
  - word 1 == `__metadata_code_end`;
  - word 2 == `_start`;
  - then `.BootLoad_ValidProgramTest.\0`;
  - the first 8 words are ARM `b` instructions (`0xEA` in the top byte).
  - With `--rtt`, it also checks that the instruction at the end of `_pabt_rtt`'s block branches to `_fault_report`: find the last `b` before the next label symbol and resolve its target.
  - Exit 1 with a message on any mismatch.
- [ ] **Step 1: Write the failing checks**
  - Add to `tools/check-layering.sh`: `rza1/` (`*.rs`, `*.x`) must not contain `BootLoad_ValidProgramTest` or `RZA1_STATUS_LED` outside comments. Run it: FAIL.
  - Write `tools/check-boot-header.py`, then run it on today's `blinky` ELF. The header checks pass, the branch check fails (today's table is `ldr pc, =`), and `--rtt` on the RTT `blinky` fails on `_pabt_rtt`.
- [ ] **Step 2: Run them to verify they fail**
  Run:
  - `tools/check-layering.sh`
  - `python3 tools/check-boot-header.py target/armv7a-none-eabihf/debug/blinky`
  Expected: both FAIL as described.
- [ ] **Step 3: Implement** as in Interfaces. Wire the check into `link-layout.sh check`: every ELF, and `--rtt` for the 4 RTT variants.
- [ ] **Step 4: Run the full verify**
  Run: `verify.sh`.
  Expected: every line PASS. `link-layout` shows only the LED symbols removed from the baseline.
- [ ] **Step 5: Commit** `rza1-rt: the boot header and status LED are the board's; deluge-bsp supplies the Deluge header`

### Task 3: Entries on `#[entry]`; the BSP's fault blink

**Files:**
- Modify:
  - `crates/deluge-bsp/src/lib.rs` (`pub use rza1_rt::entry;`, `pub mod fault_blink` behind `fault-blink`);
  - `crates/deluge-bsp/Cargo.toml` (`fault-blink` in `default`);
  - `crates/deluge-sdk-macros/src/lib.rs` (the target `main` becomes `#[::deluge::__rt::entry] fn __deluge_main() -> ! { … }`);
  - `crates/deluge-sdk/src/lib.rs` (`__rt` re-exports `deluge_bsp::entry`);
  - the ten `#[unsafe(no_mangle)] pub extern "C" fn main() -> !` binaries:
    - `firmwares/{app-loader, controller-firmware, msc-firmware}/src/main.rs`;
    - `hw-tests/{loopback-probe, overbridge-device-firmware, overbridge-host-firmware, sd-bench, uac-host-firmware, vbus-probe, wp-probe}/src/main.rs`.
  - Each becomes `#[deluge_bsp::entry] fn main() -> !`.

**Interfaces:**
- `deluge_bsp::fault_blink`: `#[rza1_rt::fault] fn blink_status_led(info: &rza1_rt::FaultInfo) -> !`.
  - It counts pulses by kind, as the assembly did: undef 1, svc 2, prefetch 3, data 4, other 5.
  - The LED is P6_7, taken with `unsafe { rza1_hal::peripherals::P6_7::steal() }` into `rza1_hal::gpio::Output`, as `deluge-sdk`'s panic path does (`crates/deluge-sdk/src/lib.rs:736-739`). The SAFETY note: the core is in the fault handler for good.
  - Busy-wait delays are counted `core::hint::spin_loop` loops with the assembly's counts (`0x0080_0000` per half-pulse, `0x0200_0000` between groups). Task 2 deletes the assembly, so take the counts from this plan.
  - No allocation, no locks, no logging.
- [ ] **Step 1: Write the failing check.** Add to `tools/check-layering.sh`: no `pub extern "C" fn main` under `firmwares/`, `hw-tests/`, `examples/`, or the generated code in `crates/deluge-sdk-macros/src`. Run it: FAIL, naming the ten binaries and the macro.
- [ ] **Step 2: Run it to verify it fails**
  Run: `tools/check-layering.sh`
  Expected: FAIL.
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Run the full verify**
  Run: `verify.sh` and `rza1/tools/test.sh`.
  Expected: every line PASS, including `check-boot-header` on every image.
- [ ] **Step 5: Commit** `firmwares: entries on #[entry]; deluge-bsp blinks the status LED on a fault`

### Task 4: Docs, CHANGELOG, bench

**Files:**
- Modify:
  - `rza1/rza1-rt/README.md` and its crate docs (the three macros, the `.boot_header` section, the hook, the weak default);
  - `crates/deluge-image/src/elf.rs:437` and `firmwares/app-loader/src/flashboot.rs:11` (point at `deluge-bsp`'s `boot_header` instead of the old `rza1-hal/src/startup.rs`);
  - `CHANGELOG.md` (a phase 7 entry, with DelugeFirmware's migration: `#[deluge_bsp::entry]` or keep `bl main`; its C `rza1_cpu_fault_report` still links; drop `app-fault-reporter` in phase 9).
- Create: `hw-tests/fault-probe` (copy `loopback-probe`'s skeleton). After 2 s it runs an undefined instruction (`core::arch::asm!("udf #0")`), with `fault-blink` on.
- [ ] **Step 1: Docs and CHANGELOG** as listed.
- [ ] **Step 2: Full verify**
  Run: `verify.sh`.
  Expected: all PASS, including the new package.
- [ ] **Step 3: Bench, RAM runs** (pi-top, `deluge_jlink.py`).
  - **Boot checks:** the SYNC LED stays dark until the app drives it; demo-firmware, controller-firmware and `blinky` boot; demo-firmware's audio still passes the phase 6 check (`speaker-test` 24-bit, an `arecord` of exactly 793,844 bytes).
  - **Fault blink:** run `fault-probe`. Over J-Link, sample PPR6 bit 7 (`0xFCFE3218`) every ~50 ms for 6 s with `JLinkExe` `mem16` and `Sleep`, without halting if possible; halt/go per sample if not. The level must toggle in single pulses separated by the long gap (an undefined instruction blinks once).
  Expected: those results.
- [ ] **Step 4: Bench, cold boot through the app-loader.**
  - The app-loader in flash must be rebuilt with this phase's runtime.
  - **Ask the user before flashing** (it overwrites the board's boot code).
  - On approval: flash with the documented app-loader flashing procedure (`docs/app-loader.md`), power-cycle through a J-Link reset with no load, and confirm from RTT, or from the OLED/LED, that the app-loader starts and launches an app.
  - If the user declines, list it as open.
- [ ] **Step 5: Commit** `rza1-rt phase 7: docs, CHANGELOG, fault-probe`
