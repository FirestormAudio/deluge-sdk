# rza1-hal Phase 8: Docs and Lint Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The five `rza1/` crates (`rza1-pac`, `rza1-rt`, `rza1-rt-macros`, `rza1-hal`, `rza1-alloc`) meet the spec's docs-hygiene bar, and `rza1/tools/test.sh` enforces it:
- `#![deny(missing_docs)]`;
- no crate-wide `allow(dead_code)`;
- no public raw `unsafe fn` channel APIs;
- `cargo doc` warning-free across the feature matrix;
- `#![no_std]` unconditionally;
- a README per crate that matches its features;
- a CHANGELOG entry.

**Architecture:** Mostly deletions and visibility changes, made enforceable:
- `test.sh` gains a `-D warnings` doc pass over the chip and logging feature matrix.
- `test.sh` gains a README-features check.
- `test.sh` gains an integration test pinning the `#[doc(hidden)]` paths DelugeFirmware still uses until phase 9.
- `-D unused` (already in `test.sh`) starts biting once `#![allow(dead_code)]` is gone.

**Tech Stack:** Rust 2024 nightly (the pinned toolchain); rustdoc; bash; python3.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md`. Relevant parts:
- §4 "Docs hygiene";
- §5 item 8: "**Docs and lint sweep.** `missing_docs`, `cargo doc` clean, README per crate, CHANGELOG entry."

## Global Constraints

- **Spec §4 "Docs hygiene", verbatim:**
  - "`#![deny(missing_docs)]` on all four crates";
  - "remove `#![allow(dead_code)]`";
  - "public raw `unsafe fn` channel APIs become `pub(crate)` or methods on drivers";
  - "`cargo doc` is warning-free";
  - "`#![no_std]` unconditionally, with `#[cfg(test)] extern crate std`".
- **"All four crates"** now means all five: `rza1-rt-macros` arrived in phase 7. A proc-macro crate is `std` by nature, so the `no_std` rule skips it.
- **Starting counts** (2026-10-30, `-W missing_docs`, rza1l):
  - `rza1-pac`: 6,829 missing docs, 6,820 of them in the generated `src/chips/rza1l.rs`.
  - `rza1-hal`: 139 missing docs (123 in `usb/regs.rs`, 10 `usb/driver.rs`, 4 `usb/pipe.rs`, 1 `sdhi.rs`, 1 `memmap.rs`) and 19 dead-code items once the allow goes.
  - `rza1-rt`, `rza1-alloc`: 0 missing docs.
  - `cargo doc`: the same manifest warning in every crate (`package.homepage` is redundant with `package.repository`); in `rza1-hal`, two links to private items (`time_driver` → `TickRatio`, `xfer_out` → `SHARED_SEND_TIMEOUT_MS`) and two unclosed HTML tags (`<NUMBER>`, `<CH>`).
- **Generated code stays generated.** `rza1-pac/src/chips/*.rs` is chiptool output that `tools/check-pac.sh` regenerates and diffs. It is never hand-edited. Its missing docs are allowed at the module that includes it, and denied everywhere else.
- **DelugeFirmware's paths until phase 9:**
  - It pins `rza1_hal::usb::pipe::{pipe_reset, pipe_enable, pipe_brdy_enable, pipe_iso_in_activate}` and `rza1_hal::usb::regs::usb`.
  - These stay reachable at those paths, `#[doc(hidden)]`.
  - Phase 9 removes them.
- **Public system-level `unsafe fn`s stay.** These are the `cache`, `mmu`, `gic`, `bsc`, `spibsc`, `adc` and `time_driver::stop` functions and `dmac::stop_all`. The BSP and the app-loader call them, and they are not channel APIs. Each has a `# Safety` section (Task 3 checks).
- **Unchanged:** the layering rules, link layout and bench behaviour. This phase has no bench check.
- **Done when:** `verify.sh` prints PASS on every line, and `rza1/tools/test.sh` passes with the new doc, README and path checks.

## Review Focus

1. **Docs clean for the default feature set only.**
   - Failure: items behind `rza1h`, `defmt`, `log` or `--no-default-features` (time driver off) carry undocumented items or broken links, which only show in those configurations.
   - Expected: every documented configuration builds its docs with `-D warnings`.
   - Test: Task 4's doc matrix in `test.sh` (rza1l and rza1h; `log`; `defmt`; `--no-default-features --features rza1l,rt,critical-section-impl`).
2. **The visibility cut breaks a consumer.**
   - Failure: a function made `pub(crate)` is used by deluge-bsp, a firmware, a hw-test or DelugeFirmware's pinned paths.
   - Expected: every in-repo build passes, and the DelugeFirmware paths still resolve.
   - Tests: `verify.sh` (every firmware, hw-test and example builds), and the `unstable_paths` integration test (Task 2).
3. **`#![no_std]` on a host build that used std implicitly.**
   - Failure: the HAL or allocator built for the host (deluge-bsp's `sim`/`linux` paths, the host tests) and quietly used the std prelude.
   - Expected: host builds still compile, with tests importing what they use.
   - Tests: `tools/test.sh` (the host builds) and `rza1/tools/test.sh` (the QEMU test runs), both inside `verify.sh`.
4. **A dead-code removal that was a hardware value or a test helper.**
   - Failure: deleting an enum variant that encodes a register-field value, or a function only the tests call.
   - Expected: register-field encodings stay, with a reasoned `#[expect(dead_code, reason = "…")]`; test-only helpers become `#[cfg(test)]`; truly dead code is deleted.
   - Test: Task 3 records each of the 19 items' disposition in its commit message, and the QEMU suite still passes.
5. **README prose that names an API which no longer exists** (`steal()`, the iso hooks, `app-fault-reporter`, `RZA1_STATUS_LED_*`).
   - Expected: each README describes the current API.
   - Test: Task 4's `check-readme-features.py` covers feature tables. The README prose is checked by review: `grep -nE 'steal\(|iso_(in|out)_hook|app-fault-reporter|RZA1_STATUS_LED' rza1/*/README.md` must print nothing (Task 4, Step 3).

---

### Task 1: `rza1-pac` — `deny(missing_docs)` and `no_std`

**Files:**
- Modify: `rza1/rza1-pac/src/lib.rs`, `rza1/rza1-pac/src/common.rs`, `rza1/rza1-pac/src/mmio.rs`.

**Interfaces:**
- Produces:
  - `#![no_std]`, `#![deny(missing_docs)]` and `#[cfg(test)] #[macro_use] extern crate std;`.
  - The chip file is included inside a private `#[allow(missing_docs)] mod chip { include!("chips/rza1l.rs"); }` (and likewise for rza1h), followed by `pub use chip::*;`.
  - Every public path (`rza1_pac::CPG`, `rza1_pac::cpg::regs::Stbcr12`, …) is unchanged.
- [ ] **Step 1: Write the failing check.** Add `#![deny(missing_docs)]` and `#![no_std]` (no other change).
  Run: `cd rza1 && cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p rza1-pac --features rza1l`
  Expected: FAIL with about 6,829 `missing documentation` errors.
- [ ] **Step 2: Implement.**
  - Wrap the generated include in the private `chip` module, as in Interfaces. If the generated code uses `super::` paths that the extra module level breaks, use `#[path]` modules instead and ledger it.
  - Document the 9 handwritten items in `common.rs`, plus anything else the deny names outside `chip`.
  - Add `#[cfg(test)] #[macro_use] extern crate std;` and import what the tests use (`std::vec::Vec`, …).
- [ ] **Step 3: Verify.**
  Run:
  - Step 1's command;
  - the same with `--features rza1h`;
  - `cd rza1 && tools/check-pac.sh`;
  - `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1l --lib`, with the QEMU runner from `test.sh`, then the same for `rza1h`.
  Expected: all pass; `check-pac.sh` reports the generated files unchanged.
- [ ] **Step 4: Commit** `rza1-pac: deny missing docs (generated chip code excepted), no_std`

### Task 2: `rza1-hal` — no public raw channel APIs; `deny(missing_docs)`

**Files:**
- Modify:
  - `rza1/rza1-hal/src/lib.rs`;
  - `rza1/rza1-hal/src/usb/{mod.rs, fifo.rs, pipe.rs, regs.rs, driver.rs, host.rs}`;
  - `rza1/rza1-hal/src/{sdhi.rs, memmap.rs}` (their missing docs);
  - `hw-tests/vbus-probe/src/tasks/probe.rs` (off `usb::regs`).
- Create: `rza1/rza1-hal/tests/unstable_paths.rs`.
- Modify: `rza1/tools/test.sh` (run that test).

**Interfaces:**
- Produces:
  - `usb::fifo` → `pub(crate) mod`.
  - `usb::regs` → `pub(crate) mod`, except `#[doc(hidden)] pub use regs::usb` re-exported as `usb::regs::usb`. To keep that path: make `regs` a `#[doc(hidden)] pub mod regs` whose items are all `pub(crate)` except `usb`.
  - `usb::pipe` → `#[doc(hidden)] pub mod pipe`: every item `pub(crate)` except the four DelugeFirmware functions, which stay `pub` and `#[doc(hidden)]`.
  - `usb::driver::dcd_int_handler`, `usb::host::hcd_int_handler`, `usb::{module_clock_enable, module_clock_disable, int_enable, int_disable}` become `pub(crate)`. Any of them that an in-repo crate still needs after the change gets a documented method on the driver instead; ledger which.
  - `vbus-probe` reads `INTSTS0.VBSTS` through `rza1_hal::pac` (its `unstable-pac` feature), not `usb::regs`.
  - `#![deny(missing_docs)]` in `lib.rs`.
- [ ] **Step 1: Write the failing tests.**
  - Create `rza1/rza1-hal/tests/unstable_paths.rs`:
    ```rust
    //! The `#[doc(hidden)]` paths DelugeFirmware still uses until phase 9 (CHANGELOG, phase 6).
    #[allow(unused_imports)]
    use rza1_hal::usb::pipe::{pipe_brdy_enable, pipe_enable, pipe_iso_in_activate, pipe_reset};
    #[allow(unused_imports)]
    use rza1_hal::usb::regs::usb;
    ```
    The file has no `#[test]`, so compiling it is the check: `harness = false` is not needed, and an empty test binary passes.
  - Add `#![deny(missing_docs)]` to `lib.rs`.
  - Add to `test.sh`, after the HAL's `--lib` QEMU run: `cargo test --target "$QEMU" -p rza1-hal --features rza1l --test unstable_paths`.
- [ ] **Step 2: Run to verify it fails.**
  Run: `cd rza1 && cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p rza1-hal --features rza1l`
  Expected: FAIL with about 139 `missing documentation` errors. (The `unstable_paths` test passes before and after; it guards Step 3's cut.)
- [ ] **Step 3: Implement** the visibility changes in Interfaces, then document what the deny still names.
- [ ] **Step 4: Verify.**
  Run:
  - Step 2's command, with `rza1l` and then `rza1h`;
  - the `unstable_paths` test;
  - `verify.sh`.
  Expected: all PASS, and `grep -rn "usb::regs" hw-tests crates firmwares` prints nothing.
- [ ] **Step 5: Commit** `rza1-hal: USB pipe/FIFO/register internals are crate-private; deny missing docs`

### Task 3: `rza1-hal` — dead code, `no_std`, `# Safety` sections

**Files:**
- Modify: `rza1/rza1-hal/src/lib.rs`, plus the files of the 19 items:
  - `dma_buf.rs:84` `bus_addr`;
  - `gic.rs:352` `handler`;
  - `ostm.rs:22,97` `CTL_FREE_RUN`, `start_free_running`;
  - `rspi.rs:515` `Unknown`;
  - `scux.rs:143,200,221,245` (`Sync`, `BYPASS`, `None`/`SsiToSrcAsync`/`FfdToSrcSync`, `None`);
  - `scux/regs.rs:51` `DESC_HEADER`;
  - `sdhi.rs:243,245,1265,1273,1333,1393` (`SDHI0_IRQS`, `SDHI1_IRQS`, `POLL_TIMEOUT`, `send_cmd_poll`, `read_blocks_poll`, `write_blocks_poll`);
  - `uart.rs:182,183` `ERI_BASE`, `RXI_BASE`;
  - `usb/host.rs:221` (field `0`).
- Also `usb/driver.rs:654` (`fetch_update` → `try_update`).

**Interfaces:**
- Produces:
  - `#![no_std]` and `#[cfg(test)] #[macro_use] extern crate std;` in `lib.rs`, replacing `#![cfg_attr(target_os = "none", no_std)]`.
  - No `#![allow(dead_code)]`.
- Each dead item gets exactly one disposition, in this order of preference:
  1. **Deleted:** unused in the library and the tests.
  2. **`#[cfg(test)]`:** only the tests use it.
  3. **`#[expect(dead_code, reason = "<the TRM field it encodes>")]`:** an enum variant or constant that spells out a register field's encoding, so the type stays complete (the SCUX path and sync enums are this kind).
  4. **Wired up:** dead only because a driver forgot to use it. Ledger it.
- [ ] **Step 1: Write the failing check.** Delete `#![allow(dead_code)]` and switch to `#![no_std]`.
  Run: `cd rza1 && cargo rustc -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p rza1-hal --lib --features rza1l -- -D unused -D deprecated`
  Expected: FAIL naming the 19 items and the `fetch_update` deprecation.
- [ ] **Step 2: Implement** each item's disposition as in Interfaces; `fetch_update` becomes `try_update`, with the same closure and orderings.
  - Fix the QEMU test modules' std imports.
  - Give every public `unsafe fn` a `# Safety` doc section. Check with `cargo clippy -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p rza1-hal --features rza1l -- -A clippy::all -D clippy::missing_safety_doc`, which must pass. Add it to `test.sh` next to the `-D unused` build.
- [ ] **Step 3: Verify.**
  Run:
  - Step 1's command, for `rza1l` and `rza1h`, and with `log` and with `defmt`;
  - `rza1/tools/test.sh`;
  - `verify.sh`.
  Expected: all PASS.
- [ ] **Step 4: Commit** `rza1-hal: no dead code, no_std, every public unsafe fn documents its safety`. The message lists the 19 dispositions, one line each.

### Task 4: The doc matrix, READMEs, the remaining crates, CHANGELOG

**Files:**
- Modify:
  - `rza1/rza1-rt/src/lib.rs`, `rza1/rza1-rt-macros/src/lib.rs`, `rza1/rza1-alloc/src/lib.rs`: `#![deny(missing_docs)]`; `rza1-alloc` gets `#![no_std]` with `#[cfg(test)] #[macro_use] extern crate std;`.
  - All five `rza1/*/Cargo.toml`: drop `homepage.workspace = true`, which `cargo doc` flags as redundant with `repository`.
  - `rza1/rza1-hal/src/time_driver.rs`, `usb/driver.rs` (or wherever `xfer_out` is), and the two files with `<NUMBER>`/`<CH>`: fix the four rustdoc warnings.
  - `rza1/tools/test.sh`;
  - each `rza1/*/README.md` as Step 3 finds;
  - `CHANGELOG.md`.
- Create:
  - `rza1/rza1-rt-macros/README.md` (the three attributes, their signatures, the symbols they export; points at `rza1-rt`'s README; MIT OR Apache-2.0 line like its siblings);
  - `rza1/tools/check-readme-features.py`.

**Interfaces:**
- `check-readme-features.py`:
  - For each crate directory under `rza1/`, read `[features]` from its `Cargo.toml` (`tomllib`).
  - Fail, naming crate and feature, if a feature other than `default` is missing from the crate's `README.md` as `` `name` ``.
  - Features listed in `HIDDEN = {"unstable-driver-steal"}` (documented as hidden) are exempt.
  - Exit 0 when clean.
- The doc pass added to `test.sh`, under `RUSTDOCFLAGS="-D warnings"`, with `cargo doc --no-deps` for:
  - `rza1-pac` with `rza1l`, and with `rza1h`;
  - `rza1-hal` with `rza1l`; `rza1h`; `rza1l,log`; `rza1l,defmt`; and `--no-default-features --features rza1l,rt,critical-section-impl`;
  - `rza1-rt`, `rza1-rt-macros` (host target) and `rza1-alloc`.
  - It replaces the existing single `cargo doc` for the steal check, which keeps its grep afterwards.
- [ ] **Step 1: Write the failing checks.** Add the doc pass and `python3 tools/check-readme-features.py` to `test.sh`.
  Run: `cd rza1 && tools/test.sh`
  Expected:
  - FAIL at the doc pass on the manifest and four rustdoc warnings;
  - with those fixed by hand in a scratch run, `check-readme-features.py` names at least `rza1-rt-macros`, which has no README.
- [ ] **Step 2: Implement** the Files list. `rza1-rt` and `rza1-alloc` already pass `missing_docs`; `rza1-rt-macros` documents its three attributes. `rza1-hal`'s README feature table must list every feature in its manifest.
- [ ] **Step 3: README prose check.**
  Run: `grep -nE 'steal\(|iso_(in|out)_hook|app-fault-reporter|RZA1_STATUS_LED|rza1l-hal' rza1/*/README.md`
  Expected: no output. Fix any hit against the current API.
- [ ] **Step 4: CHANGELOG.** Add a "phase 8" entry: the lints now enforced; the visibility changes (`usb::fifo`, `usb::regs` and `usb::pipe` internals crate-private, the DelugeFirmware exceptions `#[doc(hidden)]`); the dead items removed; `no_std` unconditional.
- [ ] **Step 5: Verify.**
  Run: `verify.sh`.
  Expected: every line PASS.
- [ ] **Step 6: Commit** `rza1: docs warning-free across the feature matrix; README per crate; CHANGELOG`
