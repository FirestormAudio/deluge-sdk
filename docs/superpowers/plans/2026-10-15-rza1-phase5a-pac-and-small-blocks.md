# RZ/A1 Phase 5a: the PAC and the Small Blocks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rza1-pac`, a chiptool-generated register crate with one feature per chip (`rza1l`, `rza1h`), whose every access goes through the MMIO test seam. The RZ/A1L PAC is checked against the RZ/A1L manual's register list. Then move the HAL's smaller blocks onto it: CPG/STB, BSC, OSTM, ADC, the L2 cache, MTU2, SCIF, RSPI, GPIO and the GIC. Their hand-written address tables go away, and so do the HAL's RZ/A1H assumptions.

**Architecture:**
- **The PAC crate.** `crates/rza1-pac` holds:
  - `src/chips/{rza1l,rza1h}.rs`: chiptool output from `vendor/rza1.svd` plus YAML transforms, one file per chip, checked in. The chip features `rza1l` and `rza1h` pick one (exactly one; both or neither is a compile error).
  - The RZ/A1L file is the vendor SVD (an RZ/A1H SVD) minus everything the RZ/A1L lacks, plus registers it has that the SVD omits. The deletions are generated from the RZ/A1L manual's Table 46.1 ("List of Registers"), not hand-maintained.
  - `src/common.rs`: a hand-written register handle that chiptool is told to use (`--common-module crate::common`). It routes every read and write through `src/mmio.rs`.
  - `src/mmio.rs`: the HAL's MMIO seam, moved here.
- **The HAL** depends on the PAC and re-exports it as `rza1l_hal::pac`. It also keeps `rza1l_hal::mmio` as a re-export, so every existing seam test and the BSP's test imports are unchanged.
- **Regeneration and the manual check.** `tools/gen-pac.sh` regenerates both chips with a pinned chiptool revision, and checks the RZ/A1L PAC against the manual both ways (every PAC register is an RZ/A1L register at an allowed width; every RZ/A1L register is in the PAC, apart from named exceptions). `tools/test.sh` fails if a checked-in file has drifted or the check fails.
- **The HAL is the RZ/A1L's** and enables `rza1-pac/rza1l`. That removes its RZ/A1H-isms: the `RSPI3`/`RSPI4` singletons, a GIC sized for 587 interrupt IDs (the RZ/A1L has 538), and GPIO docs naming ports 10–11.
- **One module per task.** Each driver task first pins the PAC's addresses and widths against the values the old table used, then replaces the table with PAC accessors. The module's existing register-sequence tests must pass unchanged.

**Tech Stack:** Rust nightly (edition 2024), chiptool `bcf538a2e7b8584ae874ee9ab72efb1576fc6152` (embassy-rs), Python 3 with PyYAML for the generator's helpers, the HAL's MMIO test seam, QEMU ARM host tests (`armv7-unknown-linux-gnueabihf`), `armv7a-none-eabihf` firmware builds.

**Reference:** the RZ/A1L hardware manual (R01UH0437EJ0700 Rev.7.00), as `vendor/docs/rza1/rza1l-hw-manual/*.md`. Table 46.1 is `ch46-list-of-registers.md`, and its legend reads "Note 1. RZ/A1L only, Note 2. RZ/A1LU only". The Deluge's chip is the RZ/A1L R7S721020. `vendor/rza1.svd` is the RZ/A1H's (`<name>RZ/A1H</name>`); there is no RZ/A1H manual in the repo.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md`, Phase 5 and §3 Decisions. Phase 5 is split by the human partner:
- **5a** (this plan): the PAC, plus CPG/STB, BSC, OSTM, ADC, L2C, MTU2, SCIF, RSPI, GPIO and GIC.
- **5b** (next plan): DMAC, SSI, SCUX, SDHI (absent from the SVD: a hand-written YAML block), SPIBSC and USB (a full migration of the `#[repr(C)]` register struct, by the human partner's decision).

## Global Constraints

- **Spec, Phase 5:**
  - "Generate `rza1-pac` with chiptool from `vendor/rza1.svd`, fixing the SVD with YAML transforms. Hand-write YAML for any block the SVD gets wrong."
  - "Post-process the generated register-access module so reads and writes go through `mmio`, keeping the host-side register-sequence tests."
  - "Before deleting each hand-written offset table, add a test that the PAC's addresses equal the old constants."
  - "Migrate one driver per task."
- **Spec Phase 5 exit:** "no `const OFF_*`/`*_BASE` register tables left in `rza1l-hal`." 5a meets it for the modules it migrates. 5b finishes it.
- **Spec §3:**
  - "Keep the MMIO test seam (`mmio.rs`) through every phase. In Phase 5 the generated PAC's register access is routed through it."
  - "Chiptool, not svd2rust."
  - "Keep the name `rza1l-hal`."
- **Layering (spec §2, rule 1):** nothing under the chip crates names `deluge` in code or depends on a `deluge-*` crate. `rza1-pac` joins `rza1l-hal`, `rza1-rt` and `rza1-alloc` in `tools/check-layering.sh`'s chip-crate rules.
- **Human partner's decisions for Phase 5:**
  - The PAC describes both chips behind features: `rza1h` is the vendor SVD with the shared shape transforms, and `rza1l` is the same minus what the RZ/A1L lacks, plus what it has that the SVD omits. Blocks no driver uses keep the SVD's layout and are not reshaped.
  - The generated source is checked in, with a drift check.
  - USB gets a full PAC migration, in 5b.
- **No memory-layout change:** `tools/link-layout.sh check` passes after every task.
- **Checks:**
  - PAC tests: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --lib`
  - HAL tests: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib`
  - Suite: `tools/test.sh` (exit 0).
  - zsh does not word-split a variable holding a command, so write commands out.
- **Firmware build check**, run at the end of every task:
  ```bash
  for p in app-loader deluge-controller demo-firmware msc-firmware wp-probe vbus-probe sd-bench uac-host-firmware wren-firmware; do printf '%s: ' $p; cargo build-fw -p $p 2>&1 | tail -1; done
  tools/build-examples.sh > /tmp/claude-1000/p5a-examples.log 2>&1; echo "examples exit $?"
  ```
  Expected: nine `Finished` lines and `examples exit 0`, with no new warnings in files the task touched.
- **Branch:** `refactor/rza1-pac-5a`, cut from `main` (`a3be46ed`, the 4c-2 merge, local). Never push.
- **Formatting:** format only the lines a task touches. Pre-existing rustfmt drift elsewhere stays. The generated file is formatted by `tools/gen-pac.sh` itself, with the same `rustfmt --edition 2024` that `cargo fmt` uses.

## Decisions settled before execution

1. **`common.rs` replaces chiptool's built-in register handle; nothing post-processes the register accessors.** chiptool's `--common-module` option makes the generated code use `crate::common::Reg<T, A>`. Our `Reg` keeps chiptool's interface (`from_ptr`, `as_ptr`, `read`, `write_value`, `write`, `modify`). `read`/`write_value` dispatch on `size_of::<T>()` to `mmio::{read,write}{8,16,32}`, and the dispatch folds away at compile time. `Reg` also gains `addr()` and `width()` for the address-pinning tests.
2. **The only post-processing strips chiptool's Cortex-M glue.** chiptool always emits:
   - an `Interrupt` enum implementing `cortex_m::interrupt::InterruptNumber`;
   - an `rt` vector table and a `cortex_m_rt` re-export;
   - crate-level `#![...]` attributes.

   `tools/pac-postprocess.py` removes them. The RZ/A1's interrupts go through the GIC and `rza1l_hal::interrupt`, not an NVIC table. The script asserts that each block it removes is present, so a chiptool change fails loudly.
3. **`mmio.rs` moves into `rza1-pac`** (`git mv`): the PAC sits below the HAL, and its `Reg` must reach the seam. The HAL does `pub use rza1_pac::mmio;`, so `crate::mmio::…` in the HAL and `rza1l_hal::mmio::test` in the BSP still resolve. The seam's thread-local shadow is one static in one crate, so HAL tests that drive PAC registers see the same log.
4. **Shape transforms only where a driver needs a different shape** (`transforms/common/`, plus each chip's GPIO block). The SVD's names and offsets were checked against every 5a constant (see each task's pinning tests).
   - **GPIO:** replaced by a hand-written block per chip. The SVD's base is `0xFCFE_3004` (port 1) with one register per port. The new block is based at `0xFCFE_3000` with port-indexed arrays (`GPIO.p(n)`, `GPIO.pmc(n)`, …), so register `n` is port `n`'s: ten ports (0–9) on the RZ/A1L, twelve on the RZ/A1H. It also carries `SNCR` and the JTAG-port registers (`JPPR0`, `JPMC0`, `JPMCSR0`, `JPIBC0`).
   - **INTC (the GIC):** the per-word distributor registers become arrays (`INTC.icdiser(n)`, …), and their single-use fieldsets are dropped. On the RZ/A1L the arrays come out shorter (17 words of enable bits, 135 of priorities, 34 of configuration) because the absent words are deleted first. There is no byte view of the priority registers: the RZ/A1L manual gives them 32-bit access only, so the dispatcher reads a word and shifts.
   - **RSPI:** gains `SPDR_BYTE` and `SPDR_HALF`, the 8- and 16-bit views of `SPDR` that the frame width selects.
   - **ADC:** `ADDRA..H` become `ADC.dr(n)`.
   - **Names:** blocks are renamed to their family (`SCIF0::SCIF0` → `scif::Scif`), and `!Sanitize` gives idiomatic Rust casing (`CPG.stbcr12()`, `cpg::regs::Stbcr12`).
   - **MTU2** stays flattened (`MTU2.tcr_2()`, `MTU2.tgra_3()`): the HAL's per-channel tables become `const` arrays of those handles.
   - All of these were exercised while planning. The generated crate builds for `armv7a-none-eabihf` in about 9 s (2 s incremental), and a host test sees PAC writes in the seam log.
5. **Fieldset newtypes are used as plain values.** Where the SVD gives a register a fieldset (`cpg::regs::Stbcr12(pub u8)`), drivers write `CPG.stbcr12().write_value(cpg::regs::Stbcr12(v))` and read `.read().0`. They keep their existing bit constants. Moving drivers onto field accessors is out of scope: it changes no addresses and would hide the sequence the tests pin.
6. **`rza1-rt`'s IRQ shim keeps its three `.equ` GICC addresses.** `global_asm!` const operands can't be computed from a pointer. Task 9 adds a HAL test that pins `INTC.icciar()`, `icceoir()` and `icchpir()` to the same three literals, citing `rza1-rt/src/startup.rs`.
7. **`tools/check-register-tables.sh` enforces the exit criterion module by module.**
   - It lists the migrated modules and fails on any non-comment, non-test line in them that declares an `OFF_*`/`*_BASE` constant or names a raw peripheral address (`0xE8……`, `0xFC……`, `0x3F……`).
   - Task 1 creates it with an empty list and wires it into `tools/test.sh`. Each driver task appends its module.
   - 5b appends the rest; the spec's exit holds when the list is every module that touches a register.
8. **The RZ/A1L PAC is derived from the manual, then checked against it.** Planning compared the SVD with Table 46.1 register by register:
   - **Agreement.** Wherever both list a register, address and access width agree.
   - **24 SVD peripherals are absent on the RZ/A1L:** DISC0/1, DVDEC0/1, FLCTL, JCU, LIN1, LVDS, PFV0/1, PWM, RSPI3/4, SCIF5–7, SDG0–3, SPIBSC1, SSIF4/5, VDC51.
   - **Eight blocks lose registers:**
     - CEU: 48 registers
     - CPG: FRQCR2, SWRSTCR2 (A1LU only), SWRSTCR3 and STBCR13
     - GPIO: ports 10–11
     - INB: 12 registers
     - INTC: distributor words for IDs above 543
     - RSCAN0: channels 2–4
     - SPIBSC0: four A1LU-only DDR registers
     - VDC50: 143 registers
   - **The RZ/A1L adds** SDHI (absent from the SVD), PFC0/PFCE0/PFCAE0 (in the GPIO block), five Ethernet TSU registers and the CoreSight debug block.

   `tools/pac-a1l-absent.py` writes `transforms/rza1l/absent.yaml` from that comparison. A peripheral goes when none of its addresses is an RZ/A1L register; a register goes when no kept instance of its block has it at an RZ/A1L address. `tools/pac-check-a1l.py` then checks the generated RZ/A1L IR against Table 46.1 both ways, with two named exceptions:
   - **Modules not covered yet:** the debugger interface, and SD host interface until 5b.
   - **Index 0 of the port-indexed GPIO arrays** for registers port 0 lacks.

   The prototype run reported 2 586 PAC registers against 2 573 covered manual registers. The difference is those seven holes plus the six `SPDR` views, and nothing else.
9. **The HAL's RZ/A1H-isms go in the task that owns each module.**
   - **RSPI (Task 7):** drop the `RSPI3`/`RSPI4` singletons.
   - **GPIO (Task 8):** ports 0–9.
   - **GIC (Task 9):** `INT_ID_TOTAL` becomes 538, since Table 7.3's highest ID is 537 and the highest typed interrupt is 307. `ICDICFR_INIT` keeps its first 34 words; the last three described IDs 544–586.
   - **Docs (Task 10):** the part number R7S721001 (an RZ/A1H part) becomes R7S721020.

   With the `rza1l` PAC these can't come back: the A1L PAC has no `RSPI3`, no `GPIO.p(10)`, and no `ICDISER17`.

## Review Focus

1. **A register's width in the SVD differs from the width the driver used.** The driver would then issue a 32-bit store where the hardware expects 8 or 16, which can write neighbouring registers or be ignored. Every task's pinning test asserts both address and width (`(r.addr(), r.width())`), and the existing sequence tests now go through the PAC.
2. **A plain write turned into a read-modify-write, or the reverse.** chiptool's `write(|r| …)` starts from zero and `modify` reads first; using the wrong one changes the bus sequence. Drivers use `write_value` / `read` exactly where they used `mmio::write*` / `mmio::read*`. The unchanged sequence tests catch a stray read or a missing dummy read; STB's `each_write_is_followed_by_a_dummy_read_of_same_reg` pins the dummy reads.
3. **The checked-in PAC drifts from its transforms or the manual.** Someone edits a YAML file and forgets to regenerate, or hand-edits `absent.yaml`. `tools/test.sh` runs `tools/gen-pac.sh --check` whenever the pinned chiptool is installed; Task 1's step shows it failing on a hand-edited chip file.
4. **An RZ/A1H-only register reaches RZ/A1L firmware.** The HAL compiles against the `rza1l` PAC, which lacks every A1H-only peripheral and register. `tools/pac-check-a1l.py` fails `tools/test.sh` if a transform or SVD change lets one back in. Task 1 shows it failing on a planted A1H-only register.
5. **The IRQ shim's hard-coded GICC addresses disagree with the PAC's.** The shim acknowledges and ends every interrupt. Task 9's `the_irq_shims_gicc_addresses_are_the_pacs` pins them. (`Reg`'s width dispatch costing instructions on a hot path is covered by Task 9's disassembly step and the link-layout check.)

---

## File Structure

| Path | Change |
|------|--------|
| `crates/rza1-pac/{Cargo.toml,README.md}` | New crate |
| `crates/rza1-pac/src/lib.rs` | `no_std` on device; chip-feature guard; `pub mod common; pub mod mmio;` and the selected `include!("chips/<chip>.rs")`; the seam test |
| `crates/rza1-pac/src/common.rs` | Seam-routed `Reg<T, A>` |
| `crates/rza1-pac/src/mmio.rs` | Moved from `crates/rza1l-hal/src/mmio.rs` |
| `crates/rza1-pac/src/chips/{rza1l,rza1h}.rs` | chiptool output (generated, checked in) |
| `crates/rza1-pac/transforms/common/{intc,names,rspi,adc}.yaml` | Shapes both chips share |
| `crates/rza1-pac/transforms/rza1l/{chip,gpio,ether}.yaml`, `absent.yaml` (generated) | The RZ/A1L |
| `crates/rza1-pac/transforms/rza1h/{chip,gpio}.yaml` | The RZ/A1H |
| `tools/gen-pac.sh`, `tools/pac-postprocess.py` | Generator and post-processor |
| `tools/pac_manual.py`, `tools/pac-a1l-absent.py`, `tools/pac-check-a1l.py` | Table 46.1 parser, the RZ/A1L deletion generator, the two-way check |
| `tools/check-register-tables.sh` | Exit-criterion check for migrated modules |
| `tools/test.sh`, `tools/check-layering.sh`, `Cargo.toml` | Wire in the PAC |
| `crates/rza1l-hal/{Cargo.toml,src/lib.rs}` | Depend on and re-export the PAC; drop `mod mmio` |
| `crates/rza1l-hal/src/{cpg,stb,bsc}.rs` | Task 2 |
| `crates/rza1l-hal/src/{ostm,adc}.rs` | Task 3 |
| `crates/rza1l-hal/src/cache.rs` | Task 4 |
| `crates/rza1l-hal/src/mtu2.rs` | Task 5 |
| `crates/rza1l-hal/src/uart.rs` | Task 6 |
| `crates/rza1l-hal/src/rspi.rs` | Task 7 |
| `crates/rza1l-hal/src/gpio.rs` | Task 8 |
| `crates/rza1l-hal/src/gic.rs` | Task 9 |
| `CHANGELOG.md`, `crates/rza1l-hal/README.md`, the spec | Task 10 |

---

### Task 1: The `rza1-pac` crate, its two chips, its generator and checks, and the seam

**Files:**
- Create:
  - everything under `crates/rza1-pac/`
  - `tools/gen-pac.sh`, `tools/pac-postprocess.py`, `tools/pac_manual.py`, `tools/pac-a1l-absent.py`, `tools/pac-check-a1l.py`
  - `tools/check-register-tables.sh`
- Move: `crates/rza1l-hal/src/mmio.rs` → `crates/rza1-pac/src/mmio.rs`
- Modify: `Cargo.toml` (workspace member + dependency), `crates/rza1l-hal/Cargo.toml`, `crates/rza1l-hal/src/lib.rs`, `tools/test.sh`, `tools/check-layering.sh`

**Interfaces:**
- Produces:
  - `rza1_pac` with features `rza1l` and `rza1h` (exactly one).
  - Under `rza1l`: `CPG, BSC, OSTM0, OSTM1, ADC, L2C, MTU2, SCIF0..SCIF4, RSPI0..RSPI2, GPIO, INTC, …`. These are the RZ/A1L's peripherals only, each a `Copy` block handle.
  - Under `rza1h`: all 71 of the SVD's.
  - `rza1_pac::common::Reg<T, A>` with `read() -> T`, `write_value(T)`, `write(|&mut T|)`, `modify(|&mut T|)`, `as_ptr() -> *mut T`, `addr() -> usize` and `width() -> usize`.
  - `rza1_pac::mmio` (the seam, unchanged API).
  - `rza1l_hal::pac` (re-export of `rza1_pac`, built with `rza1l`) and `rza1l_hal::mmio` (re-export of `rza1_pac::mmio`).
  - `tools/gen-pac.sh [--check]`.
  - `tools/check-register-tables.sh` with a `MIGRATED=()` list that later tasks extend.

- [ ] **Step 1: Branch, and write the PAC's seam test first**

```bash
cd /home/kate/GitHub/deluge-sdk && git checkout -b refactor/rza1-pac-5a main
mkdir -p crates/rza1-pac/src/chips crates/rza1-pac/transforms/common crates/rza1-pac/transforms/rza1l crates/rza1-pac/transforms/rza1h
git mv crates/rza1l-hal/src/mmio.rs crates/rza1-pac/src/mmio.rs
```

`crates/rza1-pac/Cargo.toml`:

```toml
[package]
name = "rza1-pac"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true
readme = "README.md"
description = "Peripheral access crate for the Renesas RZ/A1L and RZ/A1H, generated by chiptool from the vendor SVD; every access goes through a host-testable MMIO seam."
categories = ["embedded", "no-std", "hardware-support"]
keywords = ["rza1", "renesas", "cortex-a9", "pac", "registers"]

[package.metadata.docs.rs]
default-target = "armv7a-none-eabihf"
features = ["rza1l"]

[lib]
test = false

[features]
## The RZ/A1L (R7S72102x): the RZ/A1H's registers minus what the RZ/A1L lacks, checked against
## the RZ/A1L manual's register list.
rza1l = []
## The RZ/A1H (R7S72100x): the vendor SVD.
rza1h = []
```

`crates/rza1-pac/src/lib.rs`:

```rust
//! Register access for the Renesas RZ/A1L and RZ/A1H, generated by chiptool from `vendor/rza1.svd`
//! and the transforms in `transforms/` (`tools/gen-pac.sh` regenerates `src/chips/`). Enable exactly
//! one chip feature: `rza1l` or `rza1h`.
//!
//! Every peripheral is a `Copy` handle (`CPG`, `SCIF0`, `INTC`, …); its methods return
//! [`common::Reg`] handles whose reads and writes go through [`mmio`], so a host test can drive
//! register sequences and inspect them.
#![cfg_attr(target_os = "none", no_std)]
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
#![allow(clippy::all)]

#[cfg(all(feature = "rza1l", feature = "rza1h"))]
compile_error!("rza1-pac: enable exactly one chip feature, `rza1l` or `rza1h`, not both");
#[cfg(not(any(feature = "rza1l", feature = "rza1h")))]
compile_error!("rza1-pac: enable a chip feature, `rza1l` or `rza1h`");

pub mod common;
pub mod mmio;

#[cfg(feature = "rza1l")]
include!("chips/rza1l.rs");
#[cfg(feature = "rza1h")]
include!("chips/rza1h.rs");

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn register_accesses_go_through_the_seam_at_their_width() {
        mmio::test::reset();
        GPIO.pmc(6).write_value(0x0005);
        CPG.stbcr12().write_value(cpg::regs::Stbcr12(0xF0));
        INTC.icdiser(1).write_value(1 << 13);
        let log = mmio::test::log();
        let seen: Vec<_> = log.iter().map(|a| (a.addr, a.width)).collect();
        assert_eq!(seen, [(0xFCFE_3418, 2), (0xFCFE_0444, 1), (0xE820_1104, 4)]);
        assert_eq!(
            mmio::test::writes(),
            [(0xFCFE_3418, 0x0005), (0xFCFE_0444, 0xF0), (0xE820_1104, 1 << 13)]
        );
        mmio::test::poke16(0xFCFE_3218, 0x00A0);
        assert_eq!(GPIO.ppr(6).read(), 0x00A0);
    }

    #[test]
    fn a_register_knows_its_address_and_width() {
        assert_eq!((CPG.frqcr().addr(), CPG.frqcr().width()), (0xFCFE_0010, 2));
        assert_eq!((RSPI0.spdr_byte().addr(), RSPI0.spdr_byte().width()), (0xE800_C804, 1));
        assert_eq!((INTC.icdipr(37).addr(), INTC.icdipr(37).width()), (0xE820_1494, 4));
    }

    /// The RZ/A1L has ten ports, the RZ/A1H twelve.
    #[test]
    fn the_chip_feature_picks_the_port_count() {
        let ports = if cfg!(feature = "rza1l") { 10 } else { 12 };
        let _ = GPIO.p(ports - 1);
        assert!(std::panic::catch_unwind(|| GPIO.p(ports)).is_err());
    }
}
```

The array accessors `assert!(n < len)`, so out of range panics.

- [ ] **Step 2: Run it to see it fail**

Add `"crates/rza1-pac",` to the workspace `members` after `"crates/rza1-alloc",`. Then:

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1l --lib 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: errors for the missing `common` module and `chips/rza1l.rs`.

- [ ] **Step 3: The register handle**

`crates/rza1-pac/src/common.rs`:

```rust

```rust
//! The register handle the generated code uses (chiptool `--common-module crate::common`).
//!
//! chiptool's built-in handle with one change: every read and write goes through
//! [`crate::mmio`], real volatile accesses on the device and a shadow memory with an access log
//! on the host. `size_of::<T>()` is a constant, so the width dispatch compiles to one load or
//! store.

use core::marker::PhantomData;

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct RW;
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct R;
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct W;

mod sealed {
    use super::*;
    pub trait Access {}
    impl Access for R {}
    impl Access for W {}
    impl Access for RW {}
}

pub trait Access: sealed::Access + Copy {}
impl Access for R {}
impl Access for W {}
impl Access for RW {}

pub trait Read: Access {}
impl Read for RW {}
impl Read for R {}

pub trait Write: Access {}
impl Write for RW {}
impl Write for W {}

/// One memory-mapped register of type `T` (`u8`, `u16`, `u32` or a fieldset newtype of one).
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Reg<T: Copy, A: Access> {
    ptr: *mut u8,
    phantom: PhantomData<*mut (T, A)>,
}
unsafe impl<T: Copy, A: Access> Send for Reg<T, A> {}
unsafe impl<T: Copy, A: Access> Sync for Reg<T, A> {}

impl<T: Copy, A: Access> Reg<T, A> {
    /// # Safety
    /// `ptr` is a register of type `T`.
    #[inline(always)]
    pub const unsafe fn from_ptr(ptr: *mut T) -> Self {
        Self {
            ptr: ptr as _,
            phantom: PhantomData,
        }
    }

    #[inline(always)]
    pub const fn as_ptr(&self) -> *mut T {
        self.ptr as _
    }

    /// The register's bus address.
    #[inline(always)]
    pub fn addr(&self) -> usize {
        self.ptr as usize
    }

    /// The register's width in bytes.
    #[inline(always)]
    pub const fn width(&self) -> usize {
        core::mem::size_of::<T>()
    }
}

impl<T: Copy, A: Read> Reg<T, A> {
    #[inline(always)]
    pub fn read(&self) -> T {
        let addr = self.ptr as usize;
        // SAFETY: the handle names one register of `T`'s width, and `T` is that many bytes of
        // plain data.
        unsafe {
            match core::mem::size_of::<T>() {
                1 => core::mem::transmute_copy(&crate::mmio::read8(addr)),
                2 => core::mem::transmute_copy(&crate::mmio::read16(addr)),
                4 => core::mem::transmute_copy(&crate::mmio::read32(addr)),
                _ => unreachable!("RZ/A1L registers are 8, 16 or 32 bits wide"),
            }
        }
    }
}

impl<T: Copy, A: Write> Reg<T, A> {
    #[inline(always)]
    pub fn write_value(&self, val: T) {
        let addr = self.ptr as usize;
        // SAFETY: as in `read`.
        unsafe {
            match core::mem::size_of::<T>() {
                1 => crate::mmio::write8(addr, core::mem::transmute_copy(&val)),
                2 => crate::mmio::write16(addr, core::mem::transmute_copy(&val)),
                4 => crate::mmio::write32(addr, core::mem::transmute_copy(&val)),
                _ => unreachable!("RZ/A1L registers are 8, 16 or 32 bits wide"),
            }
        }
    }
}

impl<T: Default + Copy, A: Write> Reg<T, A> {
    /// Write `f`'s changes to the register's zero value.
    #[inline(always)]
    pub fn write(&self, f: impl FnOnce(&mut T)) {
        let mut val = Default::default();
        f(&mut val);
        self.write_value(val);
    }
}

impl<T: Copy, A: Read + Write> Reg<T, A> {
    /// Read, apply `f`, write back.
    #[inline(always)]
    pub fn modify(&self, f: impl FnOnce(&mut T)) {
        let mut val = self.read();
        f(&mut val);
        self.write_value(val);
    }
}
```

- [ ] **Step 4: The transforms**

**Shared shapes,** `crates/rza1-pac/transforms/common/`:

`intc.yaml`:

```yaml
transforms:
  - !DeleteFieldsets
    from: '^INTC::regs::ICD(ISR|ISER|ICER|ISPR|ICPR|ABR|IPR|IPTR|ICFR)\d+$'
  - !MakeRegisterArray
    blocks: ^INTC::INTC$
    from: '^ICD(ISR|ISER|ICER|ISPR|ICPR|ABR|IPR|IPTR|ICFR)\d+$'
    to: ICD$1
```

`names.yaml` names each multi-instance family's module after the family. For each pair below, write two `!Rename` entries. With `SCIF0`→`SCIF` as the example:

```yaml
transforms:
  # One module per peripheral family, named for the family rather than its first instance.
  - !Rename
    from: '^SCIF0::SCIF0$'
    to: 'SCIF::SCIF'
  - !Rename
    from: '^SCIF0::(.+)$'
    to: 'SCIF::$1'
```

Write the same pair, in this order, for:
- `RSPI0`→`RSPI`, `OSTM0`→`OSTM`, `SSIF0`→`SSIF`, `RIIC0`→`RIIC`, `SPIBSC0`→`SPIBSC`
- `USB200`→`USB`, `DISC0`→`DISC`, `DVDEC1`→`DVDEC`, `LIN0`→`LIN`, `PFV0`→`PFV`
- `RSCAN0`→`RSCAN`, `SCIM0`→`SCIM`, `SDG0`→`SDG`, `VDC50`→`VDC5`

A rename whose block the RZ/A1L deleted matches nothing there, which is fine.

`rspi.yaml`:

```yaml
transforms:
  # SPDR is accessed at the width of the frame (TRM §16.3.4): add the byte and halfword views the
  # SVD leaves out, overlapping the 32-bit register.
  - !AddRegisters
    block: '^RSPI::RSPI$'
    registers:
      - {name: SPDR_BYTE, description: "SPDR, byte access (8-bit frames).", byte_offset: 0x04, bit_size: 8}
      - {name: SPDR_HALF, description: "SPDR, halfword access (16-bit frames).", byte_offset: 0x04, bit_size: 16}
```

`adc.yaml`:

```yaml
transforms:
  # ADDRA..ADDRH are one register per channel: index them by channel.
  - !DeleteFieldsets
    from: '^ADC::regs::DR[A-H]$'
  - !MakeRegisterArray
    blocks: '^ADC::ADC$'
    from: '^DR[A-H]$'
    to: DR
```

**The RZ/A1L,** `crates/rza1-pac/transforms/rza1l/`:

`chip.yaml`:

```yaml
# The RZ/A1L PAC: the RZ/A1H SVD minus what the RZ/A1L lacks (absent.yaml, generated from the
# manual), plus what it has that the SVD omits, then the shapes both chips share.
includes:
  - absent.yaml
  - gpio.yaml
  - ether.yaml
  - ../common/intc.yaml
  - ../common/names.yaml
  - ../common/rspi.yaml
  - ../common/adc.yaml
transforms:
  - !Sanitize {}
```

`gpio.yaml`:

```yaml
transforms:
  # The SVD bases GPIO at port 1 (0xFCFE_3004) with one register per port. Replace it with a block
  # at 0xFCFE_3000 whose arrays are indexed by port number: the RZ/A1L has ports 0 to 9.
  - !DeletePeripherals
    devices: .*
    from: '^GPIO$'
  - !Delete
    from: '^GPIO::'
  - !Add
    ir:
      "block/GPIO::GPIO":
        description: "General I/O ports, indexed by port number: register n is port n's (RZ/A1L: ports 0 to 9). Port 0 is input-only, so index 0 of P, PSR, PM, PNOT, PMSR, PBDC and PIPC is not a register."
        items:
          - {name: P, byte_offset: 0x0000, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: PSR, byte_offset: 0x0100, bit_size: 32, access: Write, array: {len: 10, stride: 4}}
          - {name: PPR, byte_offset: 0x0200, bit_size: 16, access: Read, array: {len: 10, stride: 4}}
          - {name: PM, byte_offset: 0x0300, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: PMC, byte_offset: 0x0400, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: PFC, byte_offset: 0x0500, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: PFCE, byte_offset: 0x0600, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: PNOT, byte_offset: 0x0700, bit_size: 16, access: Write, array: {len: 10, stride: 4}}
          - {name: PMSR, byte_offset: 0x0800, bit_size: 32, access: Write, array: {len: 10, stride: 4}}
          - {name: PMCSR, byte_offset: 0x0900, bit_size: 32, access: Write, array: {len: 10, stride: 4}}
          - {name: PFCAE, byte_offset: 0x0A00, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: SNCR, byte_offset: 0x0C00, bit_size: 32}
          - {name: PIBC, byte_offset: 0x4000, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: PBDC, byte_offset: 0x4100, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: PIPC, byte_offset: 0x4200, bit_size: 16, array: {len: 10, stride: 4}}
          - {name: JPPR0, byte_offset: 0x4B20, bit_size: 16, access: Read}
          - {name: JPMC0, byte_offset: 0x4B40, bit_size: 16}
          - {name: JPMCSR0, byte_offset: 0x4B90, bit_size: 32, access: Write}
          - {name: JPIBC0, byte_offset: 0x4F00, bit_size: 16}
  - !AddPeripherals
    devices: .*
    peripherals:
      - {name: GPIO, base_address: 0xFCFE3000, block: "GPIO::GPIO"}
```

`ether.yaml`:

```yaml
transforms:
  # Ethernet TSU registers the RZ/A1L manual lists (Table 46.1) and the SVD omits.
  - !AddRegisters
    block: '^ETHER::ETHER$'
    registers:
      - {name: TSU_FWSLC, byte_offset: 0x1838}
      - {name: TSU_POST1, byte_offset: 0x1870}
      - {name: TSU_POST2, byte_offset: 0x1874}
      - {name: TSU_POST3, byte_offset: 0x1878}
      - {name: TSU_POST4, byte_offset: 0x187C}
```

`absent.yaml` is generated in Step 5.

**The RZ/A1H,** `crates/rza1-pac/transforms/rza1h/`:

`chip.yaml`:

```yaml
# The RZ/A1H PAC: the vendor SVD as published, with the shapes both chips share.
includes:
  - gpio.yaml
  - ../common/intc.yaml
  - ../common/names.yaml
  - ../common/rspi.yaml
  - ../common/adc.yaml
transforms:
  - !Sanitize {}
```

`gpio.yaml` is the RZ/A1L's with three changes:
- every `len: 10` becomes `len: 12`;
- "ports 0 to 9" becomes "ports 0 to 11", in both the comment and the description;
- `RZ/A1L` becomes `RZ/A1H`.

YAML gotcha: a regex ending in `::` or containing `\d` must be single-quoted. chiptool wants tagged entries (`- !Rename`), not maps.

- [ ] **Step 5: The generator, post-processor and manual tools**

`tools/pac_manual.py`:

```python
"""Parse the RZ/A1L manual's Table 46.1 (List of Registers) from its Markdown conversion."""
import re


def a1l_registers(md_path, include_a1lu_only=False):
    """{address: (module, abbreviation, access sizes)} for every register the RZ/A1L has.

    Table 46.1's legend: note *1 = RZ/A1L only, *2 = RZ/A1LU only. A module heading's note applies
    to its continuation rows.
    """
    regs = {}
    module, module_note = None, ''
    for line in open(md_path, encoding='utf-8'):
        if line.startswith('## 46.2'):
            break
        if not line.startswith('|') or line.startswith('|---') or '**Module**' in line:
            continue
        cells = [c.strip() for c in line.strip()[1:-1].split('|')]
        if len(cells) < 6:
            continue
        m, rname, abbr, _bits, addr, acc = cells[:6]
        if m:
            module_note = ' '.join(re.findall(r'\*\d+', m))
            module = re.sub(r'\*+\d*|<br>', ' ', m).strip()
        mm = re.match(r"H['’‘]([0-9A-Fa-f]{8})", addr)
        if not mm:
            continue
        note = module_note + ' ' + ' '.join(re.findall(r'\*\d+', rname + abbr))
        if '*2' in note and not include_a1lu_only:
            continue
        sizes = tuple(sorted({int(x) for x in re.findall(r'\d+', acc)}))
        regs[int(mm.group(1), 16)] = (module, re.sub(r'\*\d+', '', abbr).strip(), sizes)
    return regs
```

`tools/pac-a1l-absent.py`:

```python
#!/usr/bin/env python3
"""Write crates/rza1-pac/transforms/rza1l/absent.yaml: delete from the RZ/A1H SVD what the RZ/A1L
lacks, as listed by the RZ/A1L manual (R01UH0437EJ0700, Table 46.1, "List of Registers").

A peripheral goes when none of its registers' addresses is an RZ/A1L register. A register goes
when, in every peripheral instance the RZ/A1L keeps that shares its block, its address is not an
RZ/A1L register. Registers the manual marks "RZ/A1LU only" (note *2) count as absent.

Usage: pac-a1l-absent.py <rza1.svd> <ch46-list-of-registers.md> <absent.yaml>
"""
import collections
import re
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, __import__('os').path.dirname(__file__))
from pac_manual import a1l_registers  # noqa: E402

svd_path, md_path, out_path = sys.argv[1:4]
a1l = a1l_registers(md_path)


def num(s):
    s = s.strip()
    return int(s, 16) if s.lower().startswith('0x') else int(s)


root = ET.parse(svd_path).getroot()
periphs = collections.OrderedDict((p.findtext('name'), p) for p in root.iter('peripheral'))
block_of = {n: (p.get('derivedFrom') or n) for n, p in periphs.items()}
regs_of = {}
for n, p in periphs.items():
    src = periphs[block_of[n]]
    regs = src.find('registers')
    regs_of[n] = [] if regs is None else [
        (r.findtext('name'), num(r.findtext('addressOffset'))) for r in regs.iter('register')]

kept, gone = [], []
for n, p in periphs.items():
    base = num(p.findtext('baseAddress'))
    (kept if any(base + off in a1l for _, off in regs_of[n]) else gone).append(n)

absent_regs = collections.defaultdict(list)
for blk in sorted({block_of[n] for n in kept}):
    insts = [n for n in kept if block_of[n] == blk]
    for name, off in regs_of[blk]:
        if all(num(periphs[i].findtext('baseAddress')) + off not in a1l for i in insts):
            absent_regs[blk].append(name)

with open(out_path, 'w') as f:
    f.write('# Generated by tools/pac-a1l-absent.py from vendor/rza1.svd (RZ/A1H) and the RZ/A1L manual\'s\n')
    f.write('# Table 46.1; do not edit. What the RZ/A1L lacks: whole peripherals, then registers.\n')
    f.write('transforms:\n')
    f.write('  - !DeletePeripherals\n    devices: .*\n    from: \'^(%s)$\'\n' % '|'.join(gone))
    for blk, names in absent_regs.items():
        f.write('  - !DeleteRegisters\n    block: \'^%s::%s$\'\n    from: \'^(%s)$\'\n' % (blk, blk, '|'.join(names)))
print('peripherals deleted:', len(gone), gone)
print('registers deleted:', {b: len(v) for b, v in absent_regs.items()})
```

`tools/pac-check-a1l.py`:

```python
#!/usr/bin/env python3
"""Check the RZ/A1L PAC against the RZ/A1L manual's register list (Table 46.1), both ways:

- every register the PAC defines is an RZ/A1L register, at a width the manual allows;
- every RZ/A1L register is in the PAC, except the modules in NOT_YET (with the reason).

Input is chiptool's IR after all transforms (`chiptool generate --debug-ir-output`).

Usage: pac-check-a1l.py <ir.yaml> <ch46-list-of-registers.md>
"""
import os
import sys

import yaml

sys.path.insert(0, os.path.dirname(__file__))
from pac_manual import a1l_registers  # noqa: E402

# Manual modules the PAC doesn't describe yet, and why.
NOT_YET = {
    'Debugger interface': 'CoreSight debug components; no driver uses them',
    'SD host interface': 'absent from the vendor SVD; Phase 5b adds a hand-written block',
}

# Port-indexed GPIO arrays start at port 0, which has none of these registers (port 0 is
# input-only): their element 0 is a documented hole, not a register.
GPIO_PORT0_HOLES = {'GPIO.%s[0]' % r for r in ('p', 'psr', 'pm', 'pnot', 'pmsr', 'pbdc', 'pipc')}

ir_path, md_path = sys.argv[1:3]
ir = yaml.safe_load(open(ir_path))
manual = a1l_registers(md_path)


def offsets(item):
    arr = item.get('array')
    if not arr:
        return [0]
    if 'offsets' in arr:
        return arr['offsets']
    return [i * arr['stride'] for i in range(arr['len'])]


def walk(block, base, out, path):
    for item in ir['block/' + block]['items']:
        for k, o in enumerate(offsets(item)):
            addr = base + item['byte_offset'] + o
            name = '%s.%s%s' % (path, item['name'], '[%d]' % k if item.get('array') else '')
            if 'block' in item:
                walk(item['block'], addr, out, name)
            else:
                out.append((addr, item.get('bit_size', 32), name))


pac = []
for key, dev in ir.items():
    if key.startswith('device/'):
        for p in dev['peripherals']:
            walk(p['block'], p['base_address'], pac, p['name'])

errors = []
for addr, bits, name in pac:
    if name in GPIO_PORT0_HOLES:
        continue
    m = manual.get(addr)
    if m is None:
        errors.append('PAC register %s at 0x%08X is not an RZ/A1L register' % (name, addr))
    elif bits not in m[2]:
        errors.append('PAC register %s at 0x%08X is %d bits; the manual allows %s (%s)'
                      % (name, addr, bits, m[2], m[1]))
in_pac = {a for a, _, _ in pac}
for addr, (module, abbr, _) in sorted(manual.items()):
    if addr not in in_pac and module not in NOT_YET:
        errors.append('RZ/A1L register %s (%s) at 0x%08X is missing from the PAC' % (abbr, module, addr))

for e in errors:
    print(e)
skipped = sum(1 for m in manual.values() if m[0] in NOT_YET)
print('==> %d PAC registers, %d RZ/A1L registers (%d in modules not yet covered), %d problems'
      % (len(pac), len(manual), skipped, len(errors)))
sys.exit(1 if errors else 0)
```

`tools/pac-postprocess.py`:

```python

```python
#!/usr/bin/env python3
"""Strip chiptool's Cortex-M device glue from a rustfmt'd lib.rs (tools/gen-pac.sh).

chiptool always emits an `Interrupt` enum implementing `cortex_m::interrupt::InterruptNumber`, an
`rt` vector table and a `cortex_m_rt` re-export. The RZ/A1L is a Cortex-A9 whose interrupts go
through the GIC (`rza1l_hal::interrupt`), so they go, as do the crate-level attributes, which the
PAC's own lib.rs sets around `include!`. Each removal asserts its target exists, so a chiptool
change that moves them fails here instead of shipping half-stripped.

Usage: pac-postprocess.py <chiptool lib.rs> <output chip .rs>
"""
import re
import sys

src, dst = sys.argv[1], sys.argv[2]
s = open(src).read()
s, n = re.subn(r'^#!\[[^\n]*\]\n', '', s, flags=re.M)
assert n >= 1, 'no crate attributes to strip'


def cut_item(s, start):
    i = s.index(start)
    depth = 0
    for k in range(s.index('{', i), len(s)):
        if s[k] == '{':
            depth += 1
        elif s[k] == '}':
            depth -= 1
            if depth == 0:
                return s[:i] + s[k + 2:]
    raise SystemExit('unbalanced braces after: ' + start)


s = cut_item(s, '#[derive(Copy, Clone, Debug, PartialEq, Eq)]\npub enum Interrupt {')
s = cut_item(s, 'unsafe impl cortex_m::interrupt::InterruptNumber for Interrupt {')
s = cut_item(s, '#[cfg(feature = "rt")]\nmod _vectors {')
for line in ('#[cfg(feature = "rt")]\npub use cortex_m_rt::interrupt;\n',
             '#[cfg(feature = "rt")]\npub use Interrupt as interrupt;\n'):
    assert line in s, 'missing: ' + line
    s = s.replace(line, '')
assert 'cortex_m' not in s, 'cortex_m glue left behind'
open(dst, 'w').write(s)
```

`tools/gen-pac.sh`:

```bash
#!/usr/bin/env bash
#
# Regenerate rza1-pac: transforms/rza1l/absent.yaml from the RZ/A1L manual, then
# src/chips/{rza1l,rza1h}.rs from vendor/rza1.svd and each chip's transforms, then check the
# RZ/A1L PAC against the manual's register list. chiptool is pinned and installed under target/ on
# first use.
#
# Usage: tools/gen-pac.sh          regenerate
#        tools/gen-pac.sh --check  fail if a checked-in file differs from a fresh generation
set -euo pipefail

cd "$(dirname "$0")/.."

CHIPTOOL_REV=bcf538a2e7b8584ae874ee9ab72efb1576fc6152
ROOT="target/chiptool-$CHIPTOOL_REV"
BIN="$ROOT/bin/chiptool"
if [ ! -x "$BIN" ]; then
  cargo install --git https://github.com/embassy-rs/chiptool --rev "$CHIPTOOL_REV" --locked --root "$ROOT" \
    || cargo install --git https://github.com/embassy-rs/chiptool --rev "$CHIPTOOL_REV" --locked --root "$ROOT" --offline
fi

SVD=vendor/rza1.svd
MANUAL=vendor/docs/rza1/rza1l-hw-manual/ch46-list-of-registers.md
PAC=crates/rza1-pac
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

stale=0
emit() { # emit <generated file> <checked-in path>
  if [ "${MODE:-}" = check ]; then
    if ! cmp -s "$1" "$2"; then
      echo "error: $2 is stale; run tools/gen-pac.sh" >&2
      stale=1
    fi
  else
    cp "$1" "$2"
    echo "==> wrote $2"
  fi
}
[ "${1:-}" = --check ] && MODE=check

python3 tools/pac-a1l-absent.py "$SVD" "$MANUAL" "$OUT/absent.yaml" > /dev/null
emit "$OUT/absent.yaml" "$PAC/transforms/rza1l/absent.yaml"
# Generate from the transforms as they are on disk; in --check mode that's the checked-in
# absent.yaml, which was just compared.
for chip in rza1l rza1h; do
  mkdir -p "$OUT/$chip"
  "$BIN" generate --svd "$SVD" --transform "$PAC/transforms/$chip/chip.yaml" \
    --output "$OUT/$chip" --no-defmt --common-module 'crate::common' \
    --debug-ir-output "$OUT/$chip/ir.yaml"
  rustfmt --edition 2024 "$OUT/$chip/lib.rs"
  python3 tools/pac-postprocess.py "$OUT/$chip/lib.rs" "$OUT/$chip/chip.rs"
  emit "$OUT/$chip/chip.rs" "$PAC/src/chips/$chip.rs"
done
python3 tools/pac-check-a1l.py "$OUT/rza1l/ir.yaml" "$MANUAL"
[ "$stale" -eq 0 ] && [ "${MODE:-}" = check ] && echo "==> rza1-pac is up to date."
exit "$stale"
```

`chmod +x tools/gen-pac.sh tools/pac-postprocess.py tools/pac-a1l-absent.py tools/pac-check-a1l.py`, then run `tools/gen-pac.sh`.

Expected:
- `==> wrote` for `absent.yaml`, `chips/rza1l.rs` and `chips/rza1h.rs`;
- then `==> 2586 PAC registers, 2968 RZ/A1L registers (395 in modules not yet covered), 0 problems`.

The planning prototype produced these numbers. The deletion generator prints "peripherals deleted: 24" and registers deleted from CEU, CPG, GPIO, INB, INTC, RSCAN0, SPIBSC0 and VDC50 when run by hand.

`pac-check-a1l.py` and `pac-a1l-absent.py` need PyYAML (`python3 -c 'import yaml'`); install it (Arch: `python-yaml`; Debian: `python3-yaml`) if missing.

If neither `cargo install` resolves (no network and no cached checkout under that URL), install from the cached checkout instead and ledger that as a ruling:

```bash
cargo install --path ~/.cargo/git/checkouts/chiptool-2d3d730b3d942671/bcf538a --locked --offline --root target/chiptool-bcf538a2e7b8584ae874ee9ab72efb1576fc6152
```

- [ ] **Step 6: The PAC's tests pass, for each chip**

Run:
- `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1l --lib 2>&1 | grep -E "test result|FAILED|panicked"`
- `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1h --lib 2>&1 | grep -E "test result|FAILED|panicked"`
- `cargo check -p rza1-pac --features rza1l,rza1h 2>&1 | grep -c "enable exactly one chip feature"`
- `cargo check -p rza1-pac 2>&1 | grep -c "enable a chip feature"`

Expected: `ok` twice. Each run includes the three new tests and the seam's own tests that moved with `mmio.rs`. The last two print `1`.

- [ ] **Step 7: The HAL uses the PAC's seam**

- **Root `Cargo.toml`:** add `rza1-pac = { path = "crates/rza1-pac", version = "0.1.0" }` to `[workspace.dependencies]`, next to the other `rza1-*` entries. Match their form.
- **`crates/rza1l-hal/Cargo.toml`:** add `rza1-pac = { workspace = true, features = ["rza1l"] }` to `[dependencies]`.
- **`crates/rza1l-hal/src/lib.rs`:** delete `pub mod mmio;` (or `mod mmio;`), and add after the other top-level `pub use`:

```rust
/// The RZ/A1L's registers, generated from the vendor SVD and checked against the RZ/A1L manual
/// ([`rza1_pac`], built with its `rza1l` feature).
pub use rza1_pac as pac;
/// The MMIO seam every register access goes through (real volatile on the device, a shadow with
/// an access log under host tests).
pub use rza1_pac::mmio;
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|^error"`
Expected: `ok`, with the same count as before (190). The HAL's `crate::mmio::…` paths resolve through the re-export.

- [ ] **Step 8: The tools**

`tools/check-register-tables.sh`:

```bash

```bash
#!/usr/bin/env bash
#
# Phase 5 exit check (spec: "no const OFF_*/*_BASE register tables left in rza1l-hal"), per module
# as each one moves onto rza1-pac. A migrated module may not declare an OFF_*/*_BASE constant or
# name a raw peripheral address (0xE8……, 0xFC……, 0x3F……) outside comments and its test module.
#
# Usage: tools/check-register-tables.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# Modules under crates/rza1l-hal/src that take every register from rza1_pac.
MIGRATED=()

status=0
for m in "${MIGRATED[@]}"; do
  f="crates/rza1l-hal/src/$m"
  # Everything before the first host-test module; comment lines skipped.
  hits="$(awk '/^#\[cfg\(all\(test/{exit} {print FILENAME":"NR": "$0}' "$f" \
    | grep -vE '^[^:]+:[0-9]+: *(//|\*)' \
    | grep -E 'const +(OFF_[A-Z0-9_]+|[A-Z0-9_]*_BASE) *:|0x(E8|FC|3F)[0-9A-Fa-f]{2}_?[0-9A-Fa-f]{4}' || true)"
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    status=1
  fi
done
if [ "$status" -ne 0 ]; then
  echo "error: a migrated rza1l-hal module still hand-writes register addresses; take them from rza1_pac" >&2
fi
exit "$status"
```

`chmod +x tools/check-register-tables.sh`.

In **`tools/test.sh`**:
- After `tools/check-layering.sh`, add:

```bash
tools/check-register-tables.sh
# The checked-in PAC must match its SVD, transforms and the RZ/A1L manual. Needs the pinned
# chiptool, which tools/gen-pac.sh installs under target/ on first use.
if [ -x target/chiptool-bcf538a2e7b8584ae874ee9ab72efb1576fc6152/bin/chiptool ]; then
  tools/gen-pac.sh --check
else
  echo "  (skipped rza1-pac drift and manual check: run tools/gen-pac.sh once to install chiptool)"
fi
```

- In the QEMU bucket, before the HAL line, add:

```bash
cargo test --target "$QEMU" -p rza1-pac --features rza1l --lib
cargo test --target "$QEMU" -p rza1-pac --features rza1h --lib
```

In **`tools/check-layering.sh`**:
- Add `crates/rza1-pac` to `CHIP_DIRS`, and name it in that error message ("a chip crate (rza1-pac, rza1l-hal, rza1-rt, rza1-alloc) names the Deluge").
- In the Cargo-graph rule table (the Python list with `"crates/rza1-rt/"`), add `("crates/rza1-pac/", lambda n: n.startswith("deluge-"), "rza1-pac depends on a deluge-* crate; board facts belong in deluge-bsp"),`.

`crates/rza1-pac/README.md`: four short paragraphs.
1. What the crate is, and the two chip features.
2. How the RZ/A1L PAC is derived (the A1H SVD minus `absent.yaml`, which is generated from Table 46.1) and checked (`tools/pac-check-a1l.py`).
3. "Do not edit `src/chips/*.rs` or `transforms/rza1l/absent.yaml`: change the other transforms and run `tools/gen-pac.sh`."
4. Why `common.rs` routes through `mmio`.

- [ ] **Step 9: The checks catch drift and an RZ/A1H register**

Drift, by hand-editing the generated chip file:

```bash
cp crates/rza1-pac/src/chips/rza1l.rs /tmp/claude-1000/rza1l.keep
sed -i '0,/0xfcfe_3000usize/s//0xfcfe_3004usize/' crates/rza1-pac/src/chips/rza1l.rs
tools/gen-pac.sh --check; echo "exit $?"
cp /tmp/claude-1000/rza1l.keep crates/rza1-pac/src/chips/rza1l.rs
```

Expected: `error: crates/rza1-pac/src/chips/rza1l.rs is stale; run tools/gen-pac.sh`, `exit 1`.

An RZ/A1H-only peripheral, by planting RSPI3/4 back into the RZ/A1L's transforms:

```bash
cp crates/rza1-pac/transforms/rza1l/absent.yaml /tmp/claude-1000/absent.keep
sed -i 's/RSPI3|RSPI4|//' crates/rza1-pac/transforms/rza1l/absent.yaml
tools/gen-pac.sh --check > /tmp/claude-1000/p5a-plant.log 2>&1; echo "exit $?"; tail -3 /tmp/claude-1000/p5a-plant.log
cp /tmp/claude-1000/absent.keep crates/rza1-pac/transforms/rza1l/absent.yaml
tools/gen-pac.sh --check
```

Expected:
- `absent.yaml is stale`, plus `PAC register RSPI4.… at 0xE800E8… is not an RZ/A1L register` lines, ending `40 problems`, and a non-zero exit;
- then, after the restore, `0 problems` and `==> rza1-pac is up to date.`

- [ ] **Step 10: Suite, build check, commit**

Run `tools/test.sh` (exit 0; it now runs the PAC tests for both chips, the drift and manual check, and the empty register-table check), the **Firmware build check**, and `tools/link-layout.sh check`.

```bash
git add Cargo.toml Cargo.lock crates/rza1-pac crates/rza1l-hal tools/gen-pac.sh tools/pac-postprocess.py tools/pac_manual.py tools/pac-a1l-absent.py tools/pac-check-a1l.py tools/check-register-tables.sh tools/test.sh tools/check-layering.sh
git commit -m "feat(rza1-pac): chiptool PACs for the RZ/A1L and RZ/A1H, the RZ/A1L's checked against its manual; the MMIO seam moves into the crate"
```

---

### Task 2: CPG, STB and BSC take their registers from the PAC

**Files:** `crates/rza1l-hal/src/{cpg.rs,stb.rs,bsc.rs}`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{CPG, BSC, L2C}`, `cpg::regs::{Frqcr, Syscr3, Stbcr2..Stbcr12}`, `bsc::regs::{Cs0bcr, Cs1bcr, Cs0wcr, Cs1wcr}`.
- Produces: no API change. `cpg::init`, `stb::init(&StbConfig)` and `bsc::init_cs0`/`init_cs1` (or whatever names `bsc.rs` uses) issue the same bus sequence.

- [ ] **Step 1: Pin the PAC to the old addresses and widths**

Append to each module's `mod tests`. In `stb.rs`:

```rust
    /// The PAC's STBCRn are the registers (and widths) this module wrote by address.
    #[test]
    fn the_pac_names_the_same_stbcr_registers() {
        use crate::pac::CPG;
        let regs = [
            (CPG.stbcr2().addr(), CPG.stbcr2().width()),
            (CPG.stbcr3().addr(), CPG.stbcr3().width()),
            (CPG.stbcr4().addr(), CPG.stbcr4().width()),
            (CPG.stbcr5().addr(), CPG.stbcr5().width()),
            (CPG.stbcr6().addr(), CPG.stbcr6().width()),
            (CPG.stbcr7().addr(), CPG.stbcr7().width()),
            (CPG.stbcr8().addr(), CPG.stbcr8().width()),
            (CPG.stbcr9().addr(), CPG.stbcr9().width()),
            (CPG.stbcr10().addr(), CPG.stbcr10().width()),
            (CPG.stbcr11().addr(), CPG.stbcr11().width()),
            (CPG.stbcr12().addr(), CPG.stbcr12().width()),
        ];
        assert_eq!(
            regs,
            [
                (0xFCFE_0024, 1),
                (0xFCFE_0420, 1),
                (0xFCFE_0424, 1),
                (0xFCFE_0428, 1),
                (0xFCFE_042C, 1),
                (0xFCFE_0430, 1),
                (0xFCFE_0434, 1),
                (0xFCFE_0438, 1),
                (0xFCFE_043C, 1),
                (0xFCFE_0440, 1),
                (0xFCFE_0444, 1),
            ]
        );
    }
```

In `cpg.rs` (add a `#[cfg(all(test, not(target_os = "none")))] mod tests` if it has none):

```rust
    #[test]
    fn the_pac_names_the_same_clock_registers() {
        use crate::pac::{CPG, L2C};
        assert_eq!((CPG.frqcr().addr(), CPG.frqcr().width()), (0xFCFE_0010, 2));
        assert_eq!((CPG.syscr3().addr(), CPG.syscr3().width()), (0xFCFE_0408, 1));
        assert_eq!(
            (L2C.reg15_power_ctrl().addr(), L2C.reg15_power_ctrl().width()),
            (0x3FFF_FF80, 4)
        );
    }
```

In `bsc.rs`:

```rust
    #[test]
    fn the_pac_names_the_same_bus_registers() {
        use crate::pac::BSC;
        assert_eq!((BSC.cs0bcr().addr(), BSC.cs0bcr().width()), (0x3FFF_C004, 4));
        assert_eq!((BSC.cs1bcr().addr(), BSC.cs1bcr().width()), (0x3FFF_C008, 4));
        assert_eq!((BSC.cs0wcr().addr(), BSC.cs0wcr().width()), (0x3FFF_C028, 4));
        assert_eq!((BSC.cs1wcr().addr(), BSC.cs1wcr().width()), (0x3FFF_C02C, 4));
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib the_pac_names 2>&1 | grep -E "^test |test result"`
Expected: `ok` for all three. These are characterization tests: they pin the PAC to the table before the table goes. A failure means a transform or the SVD is wrong; fix the transform and regenerate, never the expected value.

- [ ] **Step 3: Migrate**

- **`stb.rs`:** delete the `STBCR2`..`STBCR12` constants. `init` keeps its loop over `(address, value)` pairs and its `mmio::write8(addr, val); let _ = mmio::read8(addr);` body. Only the addresses change, to `CPG.stbcr2().addr()` … `CPG.stbcr12().addr()`. Each STBCRn has its own fieldset type, so the handles don't fit in one array; taking addresses from them keeps the dummy-read sequence byte-identical.
- **The existing tests** `init_writes_every_stbcr_in_order` and `each_write_is_followed_by_a_dummy_read_of_same_reg` name the deleted constants. Replace each `STBCRn` there with its literal address (from Step 1's table), so the tests now pin addresses independently of the code.
- **The old address test** (`assert_eq!(STBCR2, 0xFCFE_0024)` …) is superseded by Step 1's test: delete it.
- **`cpg.rs`:** delete `L2_POWER_CTRL`, `FRQCR` and `SYSCR3`, and replace the six accesses:

```rust
        let l2_power = L2C.reg15_power_ctrl();
        l2_power.write_value(1);
        let _ = l2_power.read();
        CPG.frqcr().write_value(cpg::regs::Frqcr(frqcr));
        let _ = CPG.frqcr().read();
        CPG.syscr3().write_value(cpg::regs::Syscr3(0x0F));
        let _ = CPG.syscr3().read();
```

  with `use crate::pac::{cpg, CPG, L2C};`. If `Syscr3`'s newtype field isn't `pub`, use `mmio::write8(CPG.syscr3().addr(), 0x0F)` instead and note it in the ledger.
- **`bsc.rs`:** delete the four constants. Each `crate::mmio::write32(CS0BCR, BCR_16BIT_1IDLE)` becomes `BSC.cs0bcr().write_value(bsc::regs::Cs0bcr(BCR_16BIT_1IDLE))`, and the same for the others. In its tests, replace the constants in expected `writes()` with literals and delete the old address test.
- **`tools/check-register-tables.sh`:** `MIGRATED=(cpg.rs stb.rs bsc.rs)`.

- [ ] **Step 4: Verify**

Run:
- `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED"`: `ok`. STB's sequence tests and BSC's write tests pass unchanged apart from the literal addresses.
- `tools/check-register-tables.sh`: exit 0, no output.
- `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

- [ ] **Step 5: Commit**

```bash
git add crates/rza1l-hal/src/cpg.rs crates/rza1l-hal/src/stb.rs crates/rza1l-hal/src/bsc.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): CPG, STB and BSC take their registers from rza1-pac"
```

---

### Task 3: OSTM and ADC take their registers from the PAC

**Files:** `crates/rza1l-hal/src/{ostm.rs,adc.rs}`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{OSTM0, OSTM1, ostm, ADC, CPG}`.
- Produces: no API change.

- [ ] **Step 1: Pin**

In `ostm.rs`'s tests:

```rust
    #[test]
    fn the_pac_names_the_same_timer_registers() {
        use crate::pac::{CPG, OSTM0, OSTM1};
        for (t, base) in [(OSTM0, 0xFCFE_C000), (OSTM1, 0xFCFE_C400)] {
            assert_eq!((t.cmp().addr(), t.cmp().width()), (base, 4));
            assert_eq!((t.cnt().addr(), t.cnt().width()), (base + 0x04, 4));
            assert_eq!((t.te().addr(), t.te().width()), (base + 0x10, 1));
            assert_eq!((t.ts().addr(), t.ts().width()), (base + 0x14, 1));
            assert_eq!((t.tt().addr(), t.tt().width()), (base + 0x18, 1));
            assert_eq!((t.ctl().addr(), t.ctl().width()), (base + 0x20, 1));
        }
        assert_eq!((CPG.stbcr5().addr(), CPG.stbcr5().width()), (0xFCFE_0428, 1));
    }
```

In `adc.rs` (add a test module if missing):

```rust
    #[test]
    fn the_pac_names_the_same_converter_registers() {
        use crate::pac::ADC;
        for ch in 0..8 {
            assert_eq!((ADC.dr(ch).addr(), ADC.dr(ch).width()), (0xE800_5800 + ch * 2, 2));
        }
        assert_eq!((ADC.csr().addr(), ADC.csr().width()), (0xE800_5860, 2));
    }
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib the_pac_names 2>&1 | grep -E "^test .*(ostm|adc)|test result"`
Expected: `ok` (characterization).

- [ ] **Step 2: Migrate**

- **`ostm.rs`:**
  - Delete `OSTM0_BASE`, `OSTM1_BASE`, `CMP`, `CNT`, `TE`, `TS`, `TT`, `CTL` and `STBCR5`. Keep the bit constants.
  - Replace `base(n)` with `fn timer(n: u8) -> pac::ostm::Ostm { [pac::OSTM0, pac::OSTM1][n as usize] }`.
  - Each access maps one to one:
    - `crate::mmio::write8(b + TT, 1)` → `timer(n).tt().write_value(ostm::regs::Tt(1))`
    - `read8(b + TE)` → `timer(n).te().read().0`
    - `read32(b + CNT)` → `timer(n).cnt().read().0`
    - `write32(b + CMP, v)` → `.cmp().write_value(ostm::regs::Cmp(v))`
    - `write8(b + CTL, v)` → `.ctl().write_value(ostm::regs::Ctl(v))`
    - `write8(b + TS, 1)` → `.ts().write_value(ostm::regs::Ts(1))`
  - If a fieldset's inner field isn't `pub`, use `mmio::writeN(reg.addr(), v)` for that register and ledger it.
  - The STBCR5 read-modify-write becomes `let r = CPG.stbcr5(); let v = r.read().0; r.write_value(cpg::regs::Stbcr5(v & !(STBCR5_MSTP50 | STBCR5_MSTP51))); let _ = r.read();`.
- **`ostm.rs`'s tests:**
  - `the_time_drivers_ostm0_setup_is_free_running_with_a_wrap_interrupt` and `the_alarm_counts_down_once_from_its_delta` name `OSTM0_BASE + TT` etc.; replace them with literals (`0xFCFE_C018`, `0xFCFE_C000`, `0xFCFE_C020`, `0xFCFE_C014`; OSTM1 = +0x400).
  - Delete the base-address test.
- **`adc.rs`:**
  - Delete `ADC_BASE`, `ADDR0` and `ADCSR`.
  - `mmio::write16(ADCSR, v)` → `ADC.csr().write_value(v)`; `mmio::read16(ADCSR)` → `ADC.csr().read()`; `mmio::read16(ADDR0 + ch * 2)` → `ADC.dr(channel as usize).read()`. `csr` and `dr` are plain `u16` (no fieldset).
- **`tools/check-register-tables.sh`:** add `ostm.rs adc.rs`.

- [ ] **Step 3: Verify and commit**

Run the HAL tests (`ok`; the two OSTM sequence tests pass with literal addresses), `tools/check-register-tables.sh` (exit 0), `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/ostm.rs crates/rza1l-hal/src/adc.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): OSTM and ADC take their registers from rza1-pac"
```

---

### Task 4: The L2 cache controller takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/cache.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::L2C`.
- Produces: no API change. `l2_init`, `l2_disable` and `l2_invalidate_all` now go through the seam, so they become host-testable.

- [ ] **Step 1: Write the failing tests**

Append to `cache.rs` (add `#[cfg(all(test, not(target_os = "none")))] mod tests { use super::*; use crate::mmio::test; … }` if none exists):

```rust
    #[test]
    fn the_pac_names_the_same_pl310_registers() {
        use crate::pac::L2C;
        assert_eq!((L2C.reg1_control().addr(), L2C.reg1_control().width()), (0x3FFF_F100, 4));
        assert_eq!((L2C.reg2_int_clear().addr(), L2C.reg2_int_clear().width()), (0x3FFF_F220, 4));
        assert_eq!((L2C.reg7_inv_way().addr(), L2C.reg7_inv_way().width()), (0x3FFF_F77C, 4));
        assert_eq!((L2C.reg7_cache_sync().addr(), L2C.reg7_cache_sync().width()), (0x3FFF_F730, 4));
        assert_eq!((L2C.reg9_d_lockdown0().addr(), L2C.reg9_d_lockdown0().width()), (0x3FFF_F900, 4));
        assert_eq!((L2C.reg9_i_lockdown0().addr(), L2C.reg9_i_lockdown0().width()), (0x3FFF_F904, 4));
    }

    #[test]
    fn disabling_the_l2_clears_its_control_register() {
        test::reset();
        test::poke32(0x3FFF_F100, 1);
        unsafe { l2_disable() };
        assert_eq!(test::writes(), [(0x3FFF_F100, 0)]);
    }
```

`l2_init` and `l2_invalidate_all` poll `REG7_INV_WAY` until the hardware clears it, and the shadow never clears it, so they stay untested on the host; `l2_disable` is the one that can be driven.

- [ ] **Step 2: Run them to see the second fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib cache:: 2>&1 | tail -15`
Expected: the pinning test passes. `disabling_the_l2_clears_its_control_register` brings the test process down with a segmentation fault: `l2_disable` dereferences `0x3FFF_F100` directly, outside the seam, which is the reason for this task. If the run aborts before reporting the pinning test, run the pinning test alone by name to see it pass.

- [ ] **Step 3: Migrate**

- Delete `L2C_BASE`, `L2C_REG1_CONTROL`, `L2C_REG2_INT_CLR`, `L2C_REG7_INV_WAY`, `L2C_REG9_D_LOCK0`, `L2C_REG9_I_LOCK0`, `L2C_REG7_CACHE_SYNC`, and the private `wr32`/`rd32`. Keep the value constants (`L2C_8WAY`, `L2C_INT_CLR_ALL`, `L2C_ENABLE`).
- Each `wr32(L2C_REG1_CONTROL, v)` becomes `L2C.reg1_control().write_value(v)`, and `rd32(L2C_REG7_INV_WAY)` becomes `L2C.reg7_inv_way().read()`. The same applies to the other four registers. All are plain `u32`.
- `use crate::pac::L2C;` at the top.
- **`tools/check-register-tables.sh`:** add `cache.rs`.

Run the cache tests again: `ok` for both.

- [ ] **Step 4: Verify and commit**

Run the HAL tests, `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/cache.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): the PL310 takes its registers from rza1-pac, and its accesses go through the seam"
```

---

### Task 5: MTU2 takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/mtu2.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{MTU2, common::{Reg, RW}}`, with the flattened per-channel accessors `tcr_N`, `tier_N`, `tsr_N`, `tcnt_N`, `tgra_N` (N = 0–4), plus `tstr` and `trwer`.
- Produces: no API change. `OneShot` and the channel functions issue the same sequence.

- [ ] **Step 1: Pin**

```rust
    #[test]
    fn the_pac_names_the_same_channel_registers() {
        use crate::pac::MTU2;
        let tcr = [MTU2.tcr_0().addr(), MTU2.tcr_1().addr(), MTU2.tcr_2().addr(), MTU2.tcr_3().addr(), MTU2.tcr_4().addr()];
        assert_eq!(tcr, [0xFCFF_0300, 0xFCFF_0380, 0xFCFF_0000, 0xFCFF_0200, 0xFCFF_0201]);
        let tier = [MTU2.tier_0().addr(), MTU2.tier_1().addr(), MTU2.tier_2().addr(), MTU2.tier_3().addr(), MTU2.tier_4().addr()];
        assert_eq!(tier, [0xFCFF_0304, 0xFCFF_0384, 0xFCFF_0004, 0xFCFF_0208, 0xFCFF_0209]);
        let tsr = [MTU2.tsr_0().addr(), MTU2.tsr_1().addr(), MTU2.tsr_2().addr(), MTU2.tsr_3().addr(), MTU2.tsr_4().addr()];
        assert_eq!(tsr, [0xFCFF_0305, 0xFCFF_0385, 0xFCFF_0005, 0xFCFF_022C, 0xFCFF_022D]);
        let tcnt = [MTU2.tcnt_0().addr(), MTU2.tcnt_1().addr(), MTU2.tcnt_2().addr(), MTU2.tcnt_3().addr(), MTU2.tcnt_4().addr()];
        assert_eq!(tcnt, [0xFCFF_0306, 0xFCFF_0386, 0xFCFF_0006, 0xFCFF_0210, 0xFCFF_0212]);
        let tgra = [MTU2.tgra_0().addr(), MTU2.tgra_1().addr(), MTU2.tgra_2().addr(), MTU2.tgra_3().addr(), MTU2.tgra_4().addr()];
        assert_eq!(tgra, [0xFCFF_0308, 0xFCFF_0388, 0xFCFF_0008, 0xFCFF_0218, 0xFCFF_021C]);
        assert_eq!((MTU2.tstr().addr(), MTU2.tstr().width()), (0xFCFF_0280, 1));
        assert_eq!((MTU2.trwer().addr(), MTU2.trwer().width()), (0xFCFF_0284, 1));
        // Widths: TCR/TIER/TSR are bytes, TCNT/TGRA halfwords.
        assert_eq!((MTU2.tcr_3().width(), MTU2.tier_3().width(), MTU2.tsr_3().width()), (1, 1, 1));
        assert_eq!((MTU2.tcnt_3().width(), MTU2.tgra_3().width()), (2, 2));
    }
```

Run it by name. Expected: `ok` (characterization).

- [ ] **Step 2: Migrate**

- Replace the five `[usize; 5]` tables with tables of register handles. Every `MTU2.x_N()` is a `const fn`, so these are `const`:

```rust
use crate::pac::common::{Reg, RW};
use crate::pac::MTU2;

const TCR: [Reg<u8, RW>; 5] = [MTU2.tcr_0(), MTU2.tcr_1(), MTU2.tcr_2(), MTU2.tcr_3(), MTU2.tcr_4()];
const TIER: [Reg<u8, RW>; 5] = [MTU2.tier_0(), MTU2.tier_1(), MTU2.tier_2(), MTU2.tier_3(), MTU2.tier_4()];
const TSR: [Reg<u8, RW>; 5] = [MTU2.tsr_0(), MTU2.tsr_1(), MTU2.tsr_2(), MTU2.tsr_3(), MTU2.tsr_4()];
const TCNT: [Reg<u16, RW>; 5] = [MTU2.tcnt_0(), MTU2.tcnt_1(), MTU2.tcnt_2(), MTU2.tcnt_3(), MTU2.tcnt_4()];
const TGRA: [Reg<u16, RW>; 5] = [MTU2.tgra_0(), MTU2.tgra_1(), MTU2.tgra_2(), MTU2.tgra_3(), MTU2.tgra_4()];
```

  If a generated register's type is a fieldset rather than `u8`/`u16`, the array's element type is that fieldset's register type. The compiler names it; follow it.
- Delete `TSTR` and `TRWER`, and use `MTU2.tstr()` / `MTU2.trwer()`. Their fieldsets (`mtu2::regs::Tstr`, `Trwer`) wrap a `u8`, so writes take `mtu2::regs::Tstr(v)` and reads `.read().0`.
- Delete `rd8`/`wr8`/`rd16`/`wr16`. Each `rd8(TCR[ch])` becomes `TCR[ch as usize].read()`, and each `wr8(TSR[ch], v)` becomes `TSR[ch as usize].write_value(v)`.
- In the existing tests, replace any table-indexed expected address with its literal.
- **`tools/check-register-tables.sh`:** add `mtu2.rs`.

- [ ] **Step 3: Verify and commit**

Run the HAL tests (`ok`; MTU2's existing OneShot sequence tests pass), `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/mtu2.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): MTU2 takes its registers from rza1-pac"
```

---

### Task 6: The SCIF UART takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/uart.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{SCIF0..SCIF4, scif}`, with `scif::Scif` methods `smr, brr, scr, ftdr, fsr, frdr, fcr, fdr, sptr, lsr, emr` and their fieldsets `scif::regs::{Smr, Brr, Scr, Ftdr, Fsr, Frdr, Fcr, Fdr, Sptr, Lsr, Emr}`.
- Produces: no API change. `UartRx`/`UartTx`, `try_write_pic` and the RX ring descriptor issue the same sequences, and the descriptor's source address is unchanged.

- [ ] **Step 1: Pin**

```rust
    #[test]
    fn the_pac_names_the_same_scif_registers() {
        for ch in 0..NUM_CHANNELS {
            let s = scif(ch);
            let base = 0xE800_7000 + ch * 0x800;
            let got = [
                (s.smr().addr(), s.smr().width()),
                (s.brr().addr(), s.brr().width()),
                (s.scr().addr(), s.scr().width()),
                (s.ftdr().addr(), s.ftdr().width()),
                (s.fsr().addr(), s.fsr().width()),
                (s.frdr().addr(), s.frdr().width()),
                (s.fcr().addr(), s.fcr().width()),
                (s.fdr().addr(), s.fdr().width()),
                (s.sptr().addr(), s.sptr().width()),
                (s.lsr().addr(), s.lsr().width()),
                (s.emr().addr(), s.emr().width()),
            ];
            assert_eq!(
                got,
                [
                    (base, 2),
                    (base + 0x04, 1),
                    (base + 0x08, 2),
                    (base + 0x0C, 1),
                    (base + 0x10, 2),
                    (base + 0x14, 1),
                    (base + 0x18, 2),
                    (base + 0x1C, 2),
                    (base + 0x20, 2),
                    (base + 0x24, 2),
                    (base + 0x28, 2),
                ],
                "SCIF{ch}"
            );
        }
    }
```

Add the helper it calls next to the old `base`, ahead of the migration:

```rust
/// SCIF channel `ch`'s registers.
fn scif(ch: usize) -> crate::pac::scif::Scif {
    [crate::pac::SCIF0, crate::pac::SCIF1, crate::pac::SCIF2, crate::pac::SCIF3, crate::pac::SCIF4][ch]
}
```

Run it by name. Expected: `ok` (characterization).

- [ ] **Step 2: Migrate**

- Delete `SCSMR`..`SCEMR`, `base` and the four `rd*`/`wr*` helpers. Keep the bit constants.
- Each `wr16(b + SCSCR, v)` becomes `scif(ch).scr().write_value(Scr(v))`, and each `rd16(b + SCFSR)` becomes `scif(ch).fsr().read().0`. The rest map the same way:

  | Old offset | Accessor | Fieldset |
  |---|---|---|
  | SCSMR | `smr` | `Smr` |
  | SCBRR | `brr` | `Brr` |
  | SCSCR | `scr` | `Scr` |
  | FTDR | `ftdr` | `Ftdr` |
  | SCFSR | `fsr` | `Fsr` |
  | SCFRDR | `frdr` | `Frdr` |
  | SCFCR | `fcr` | `Fcr` |
  | SCFDR | `fdr` | `Fdr` |
  | SCSPTR | `sptr` | `Sptr` |
  | SCLSR | `lsr` | `Lsr` |
  | SCEMR | `emr` | `Emr` |

  Import them with `use crate::pac::scif::regs::{…}`.
- Where a function held `let b = base(ch);`, hold `let s = scif(ch);`.
- The RX ring descriptor's source, `(b + SCFRDR) as u32`, becomes `scif(ch).frdr().addr() as u32`.
- `UartTx`'s byte write at `base(self.ch) + FTDR` becomes `scif(self.ch).ftdr().write_value(Ftdr(byte))`.
- In the tests, replace `base(n)` and offset constants in expected values with literals, and delete `base`'s address test (Step 1's test supersedes it).
- **`tools/check-register-tables.sh`:** add `uart.rs`. The test module's literal DMAC addresses (`0xE820_0000 + …`, `0xFCFE_1014`) sit after `#[cfg(all(test`, which the check skips.

- [ ] **Step 3: Verify and commit**

Run the HAL tests (`ok`; the existing UART tests pass, including the RX-ring tests that read the descriptor's source address), `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/uart.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): the SCIF UART takes its registers from rza1-pac"
```

---

### Task 7: RSPI takes its registers from the PAC, and its channels are the RZ/A1L's three

**Files:** `crates/rza1l-hal/src/{rspi.rs,lib.rs}`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{RSPI0, RSPI1, RSPI2, rspi::Rspi}` (the RZ/A1L has RSPI channels 0–2; the `rza1l` PAC has no `RSPI3`/`RSPI4`), with methods `spcr, sslp, sppcr, spsr, spdr, spdr_byte, spdr_half, spscr, spssr, spbr, spdcr, spckd, sslnd, spnd, spcmd0, spbfcr` (all plain `u8`/`u16`/`u32`; the SVD gives RSPI no fieldsets).
- Produces: `peripherals::RSPI3` and `RSPI4` are gone, along with their `Instance` impls. They named channels the RZ/A1L doesn't have, and nothing used them. Otherwise no API change.

- [ ] **Step 1: Pin**

```rust
    #[test]
    fn the_pac_names_the_same_rspi_registers() {
        for ch in 0..3u8 {
            let r = rspi(ch);
            let base = 0xE800_C800 + ch as usize * 0x800;
            let got = [
                (r.spcr().addr(), r.spcr().width()),
                (r.sslp().addr(), r.sslp().width()),
                (r.sppcr().addr(), r.sppcr().width()),
                (r.spsr().addr(), r.spsr().width()),
                (r.spdr().addr(), r.spdr().width()),
                (r.spdr_byte().addr(), r.spdr_byte().width()),
                (r.spscr().addr(), r.spscr().width()),
                (r.spssr().addr(), r.spssr().width()),
                (r.spbr().addr(), r.spbr().width()),
                (r.spdcr().addr(), r.spdcr().width()),
                (r.spckd().addr(), r.spckd().width()),
                (r.sslnd().addr(), r.sslnd().width()),
                (r.spnd().addr(), r.spnd().width()),
                (r.spcmd0().addr(), r.spcmd0().width()),
                (r.spbfcr().addr(), r.spbfcr().width()),
            ];
            assert_eq!(
                got,
                [
                    (base, 1),
                    (base + 0x01, 1),
                    (base + 0x02, 1),
                    (base + 0x03, 1),
                    (base + 0x04, 4),
                    (base + 0x04, 1),
                    (base + 0x08, 1),
                    (base + 0x09, 1),
                    (base + 0x0A, 1),
                    (base + 0x0B, 1),
                    (base + 0x0C, 1),
                    (base + 0x0D, 1),
                    (base + 0x0E, 1),
                    (base + 0x10, 2),
                    (base + 0x20, 1),
                ],
                "RSPI{ch}"
            );
        }
    }
```

Add the helper first:

```rust
/// RSPI channel `ch`'s registers (the RZ/A1L has channels 0–2).
fn rspi(ch: u8) -> crate::pac::rspi::Rspi {
    [crate::pac::RSPI0, crate::pac::RSPI1, crate::pac::RSPI2][ch as usize]
}
```

Run it by name. Expected: `ok` (characterization).

- [ ] **Step 2: Migrate**

- Delete `RSPI0_BASE`, `RSPI_STRIDE`, every `OFF_*`, `base`, and the `reg8`/`reg16`/`reg32` helpers.
- `reg8(ch, OFF_SPCR).write(v)` becomes `rspi(ch).spcr().write_value(v)`, and so on for each register.
- `reg32(ch, OFF_SPDR).write(data)` becomes `rspi(ch).spdr().write_value(data)`.
- The byte write at line ~313 (`crate::mmio::Reg8(reg32(ch, OFF_SPDR).0).write(data)`) becomes `rspi(ch).spdr_byte().write_value(data)`. Same address, same width: the reason Task 1 added `SPDR_BYTE`.
- The RSPI register handles are not `unsafe` to use; drop `unsafe` blocks that only wrapped `Reg8::write`/`read`, keeping each block whose body still has another unsafe operation.
- In the tests, replace offset constants and `base` in expected values with literals, and delete the base-address test (it asserted `base(3)`/`base(4)`, channels the RZ/A1L lacks).
- **Drop RSPI3 and RSPI4:**
  - Remove them from `crates/rza1l-hal/src/lib.rs`'s `peripherals!` list.
  - In `rspi.rs`, remove them from the macro or list that implements `Instance` (`const INDEX`) for each channel.
  - `rg -n "RSPI[34]" -g '*.rs' crates firmwares hw-tests examples` must print nothing afterwards; comments don't count.
- **`tools/check-register-tables.sh`:** add `rspi.rs`.

- [ ] **Step 3: Verify and commit**

Run the HAL tests (`ok`; RSPI's sequence tests and `a_stolen_rspi_touches_no_register` pass), `tools/check-register-tables.sh`, `tools/test.sh`, the BSP tests (`cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`; `bus.rs` drives RSPI0 through the seam), the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/rspi.rs tools/check-register-tables.sh
git add crates/rza1l-hal/src/lib.rs
git commit -m "refactor(rza1l-hal)!: RSPI takes its registers from rza1-pac, and RSPI3/RSPI4, which the RZ/A1L lacks, are gone"
```

---

### Task 8: GPIO takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/gpio.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::GPIO`, with port-indexed `p, ppr, pm, pmc, pfc, pfce, pfcae, pibc, pipc` (`n` = port number, 0–9 on the RZ/A1L; all `u16`).
- Produces:
  - `set_as_output`, `set_as_input`, `enable_input_buffer`, `read_pin`, `read_port`, `write`, `set_pin_mux` and `Pin` are unchanged.
  - `p`, `pm`, `pmc` and `pipc` stop being `pub` (nothing outside the HAL calls them) and return `Reg<u16, RW>`. `ppr` and `pibc` likewise.

- [ ] **Step 1: Pin**

Replace the four existing address tests (lines ~296–331, `p(1) as usize == 0xFCFE_3004` …) with one test that covers every register array this module uses, by port:

```rust
    #[test]
    fn the_pac_indexes_gpio_registers_by_port() {
        use crate::pac::GPIO;
        for port in 1..=9usize {
            let n = port * 4;
            assert_eq!((GPIO.p(port).addr(), GPIO.p(port).width()), (0xFCFE_3000 + n, 2));
            assert_eq!(GPIO.ppr(port).addr(), 0xFCFE_3200 + n);
            assert_eq!(GPIO.pm(port).addr(), 0xFCFE_3300 + n);
            assert_eq!(GPIO.pfc(port).addr(), 0xFCFE_3500 + n);
            assert_eq!(GPIO.pfce(port).addr(), 0xFCFE_3600 + n);
            assert_eq!(GPIO.pfcae(port).addr(), 0xFCFE_3A00 + n);
            assert_eq!(GPIO.pipc(port).addr(), 0xFCFE_7200 + n);
        }
        for port in 0..=9usize {
            assert_eq!((GPIO.pmc(port).addr(), GPIO.pmc(port).width()), (0xFCFE_3400 + port * 4, 2));
            assert_eq!((GPIO.pibc(port).addr(), GPIO.pibc(port).width()), (0xFCFE_7000 + port * 4, 2));
        }
    }
```

These are the addresses the old tests pinned, for the ports the RZ/A1L has: `p(1)` = `0xFCFE_3004`, `pm(6)` = `0xFCFE_3318`, `pmc(0)` = `0xFCFE_3400`. The old tests' port-11 cases (`p(11)`, `pm(11)`, `pmc(11)`, `pipc(11)`) named registers the RZ/A1L doesn't have.

Run it by name. Expected: `ok` (characterization).

- [ ] **Step 2: Migrate**

- Delete `GPIO_BASE`, and `set_pin_mux`'s local `PFC_BASE`, `PFCE_BASE`, `PFCAE_BASE`, `PMC_BASE` and `PIPC_BASE`.
- `p(port)`, `pm`, `pmc`, `pipc`, `ppr` and `pibc` become private one-liners returning `GPIO.p(port as usize)` (etc.). Delete them if each has only a couple of callers, and call `GPIO.x(port as usize)` inline.
- `set_bits16`/`clear_bits16(addr, bits)` take a `Reg<u16, RW>` and become `r.modify(|v| *v |= bits)` / `r.modify(|v| *v &= !bits)`. That is one read and one write, the same sequence as today's read-then-write; the existing `set_pin_mux` sequence test (lines ~415–) pins it.
- `set_pin_mux`'s `modify(base, port, pin, set)` takes the register: `modify(GPIO.pfcae(port as usize), pin, mux >= 5)`.
- In the existing `set_pin_mux` test, keep its literal expected addresses (`PFC = 0xFCFE_3504` …); they're test-local constants, which the check ignores.
- The module docs and the functions' `port` docs say "1-based (1..=11)"; make them "1–9 (the RZ/A1L's ports; port 0 is input-only)", and say 0–9 where port 0 is valid (`pmc`, `pibc`).
- **`tools/check-register-tables.sh`:** add `gpio.rs`. Doc comments citing addresses (`//! - Pn … 0xFCFE_3004`) are comments and pass.

- [ ] **Step 3: Verify and commit**

Run the HAL tests (`ok`; `set_pin_mux`'s sequence test passes unchanged), `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/gpio.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): GPIO takes its port-indexed registers from rza1-pac"
```

---

### Task 9: The GIC takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/gic.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::INTC`, with:
  - arrays `icdisr, icdiser, icdicer, icdicpr, icdabr` (17 words on the RZ/A1L), `icdipr, icdiptr` (135) and `icdicfr` (34), `n` = word;
  - `icddcr, iccicr, iccpmr, iccbpr, icciar, icceoir, icchpir`;
  - `icr1, irqrr` (the RZ/A1L external-interrupt registers at `0xFCFE_F802`/`0xFCFE_F804`, which the SVD puts in the INTC block).
- Produces:
  - `gic::INT_ID_TOTAL` is 538, the RZ/A1L's interrupt count (Table 7.3's highest ID is 537). It was the RZ/A1H's 587, so `init` wrote distributor words the RZ/A1L doesn't have.
  - `gic::init`, `enable`, `disable`, `set_priority`, `shutdown`, `set_irq_both_edges`, `set_irq_falling_edge`, `clear_irq_pending` and the dispatcher now all go through the seam, so `init` and the IRQ-edge helpers become host-testable.

- [ ] **Step 1: Write the failing tests**

Append to `gic.rs`'s `mod tests`:

```rust
    #[test]
    fn the_pac_names_the_same_gic_registers() {
        use crate::pac::INTC;
        assert_eq!(INTC.icddcr().addr(), 0xE820_1000);
        assert_eq!(INTC.icdisr(0).addr(), 0xE820_1080);
        assert_eq!(INTC.icdiser(0).addr(), 0xE820_1100);
        assert_eq!(INTC.icdicer(0).addr(), 0xE820_1180);
        assert_eq!(INTC.icdicpr(0).addr(), 0xE820_1280);
        assert_eq!(INTC.icdabr(0).addr(), 0xE820_1300);
        assert_eq!((INTC.icdipr(0).addr(), INTC.icdipr(0).width()), (0xE820_1400, 4));
        assert_eq!(INTC.icdipr(134).addr(), 0xE820_1618);
        assert_eq!(INTC.icdiptr(0).addr(), 0xE820_1800);
        assert_eq!(INTC.icdicfr(0).addr(), 0xE820_1C00);
        assert_eq!(INTC.iccicr().addr(), 0xE820_2000);
        assert_eq!(INTC.iccpmr().addr(), 0xE820_2004);
        assert_eq!(INTC.iccbpr().addr(), 0xE820_2008);
        assert_eq!((INTC.icr1().addr(), INTC.icr1().width()), (0xFCFE_F802, 2));
        assert_eq!((INTC.irqrr().addr(), INTC.irqrr().width()), (0xFCFE_F804, 2));
    }

    /// rza1-rt's IRQ shim (`src/startup.rs`, `.equ GICC_*_ADDR`) acknowledges and ends every
    /// interrupt at these addresses; `global_asm!` can't take them from the PAC, so pin them here.
    #[test]
    fn the_irq_shims_gicc_addresses_are_the_pacs() {
        use crate::pac::INTC;
        assert_eq!(INTC.icciar().addr(), 0xE820_200C);
        assert_eq!(INTC.icceoir().addr(), 0xE820_2010);
        assert_eq!(INTC.icchpir().addr(), 0xE820_2018);
    }

    /// The RZ/A1L's distributor: interrupt IDs 0–537 (manual Table 7.3), so 17 words of enable
    /// bits, 135 of priorities and 34 of configuration (Table 46.1).
    #[test]
    fn the_gic_is_sized_for_the_rza1l() {
        assert_eq!(INT_ID_TOTAL, 538);
        assert_eq!(ICDICFR_INIT.len(), 34);
        assert!(std::panic::catch_unwind(|| crate::pac::INTC.icdiser(17)).is_err());
        assert!(std::panic::catch_unwind(|| crate::pac::INTC.icdipr(135)).is_err());
    }

    #[test]
    fn init_masks_everything_then_enables_the_interfaces_last() {
        crate::mmio::test::reset();
        unsafe { init() };
        let w = crate::mmio::test::writes();
        // Every ICDICER word written all-ones (interrupts off), and none past the RZ/A1L's 17.
        for i in 0..17 {
            assert!(w.contains(&(0xE820_1180 + 4 * i, 0xFFFF_FFFF)), "ICDICER{i}");
        }
        assert!(!w.iter().any(|&(a, _)| a == 0xE820_1180 + 4 * 17), "no ICDICER17 on the RZ/A1L");
        // SPIs (words 8..) routed to CPU 0; the CPU-local words untouched.
        assert!(w.contains(&(0xE820_1800 + 4 * 8, 0x0101_0101)));
        assert!(!w.iter().any(|&(a, _)| (0xE820_1800..0xE820_1820).contains(&a)));
        // Priority mask 31, then the CPU interface, then the distributor, last.
        let n = w.len();
        assert_eq!(&w[n - 4..], [(0xE820_2004, 31 << 3), (0xE820_2008, 2), (0xE820_2000, 3), (0xE820_1000, 1)]);
    }

    #[test]
    fn an_irq_pin_is_set_to_both_edges_through_level_low() {
        crate::mmio::test::reset();
        crate::mmio::test::poke16(0xFCFE_F802, 0b01 << 6);
        unsafe { set_irq_both_edges(3) };
        assert_eq!(crate::mmio::test::writes(), [(0xFCFE_F802, 0), (0xFCFE_F802, 0b11 << 6)]);
    }
```

- [ ] **Step 2: Run them to see the last two fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib gic:: 2>&1 | tail -15`
Expected:
- The two pinning tests pass when run alone by name.
- `the_gic_is_sized_for_the_rza1l` fails: `INT_ID_TOTAL` is 587 and `ICDICFR_INIT` has 37 words.
- `init_masks_everything_then_enables_the_interfaces_last` and `an_irq_pin_is_set_to_both_edges_through_level_low` crash the test process with a segmentation fault: `init` and `set_irq_both_edges` dereference GIC addresses directly, outside the seam.

- [ ] **Step 3: Migrate**

- Delete:
  - `GICD_BASE`, `GICD_CTLR`, and every `GICD_*0` table constant;
  - `GICC_BASE` and the six `pub(crate) GICC_*_ADDR`;
  - `GICD_IPRIORITYR0_ADDR` and the local `ICR1`/`IRQRR` constants.

  Keep the value constants (`GIC_PRIORITY_SHIFT`, `GIC_PRIORITY_LOWEST_WORD`, `ICCICR_*`, `ICCIAR_*`).
- **Size the GIC for the RZ/A1L:**
  - `INT_ID_TOTAL` becomes `538`, with its doc citing manual Table 7.3 (highest ID 537). `n_isr`, `n_ipr` and `n_iptr` follow from it: 17, 135, 135.
  - `ICDICFR_INIT` becomes `[u32; 34]`: delete its last three entries (`ICDICFR34`–`36`, IDs 544–586).
  - The typed interrupts top out at 307, so nothing else moves.
- **`init`:** each raw-pointer loop becomes a loop over the PAC array, writing the same values. `icpendr.add(i).write_volatile(0xFFFF_FFFF)` → `INTC.icdicpr(i).write_value(0xFFFF_FFFF)`; `icdisr(i)` writes 0; `icdicfr(i)` takes `ICDICFR_INIT[i]` from `i = 1`; `icdipr(i)` takes `GIC_PRIORITY_LOWEST_WORD`; `icdiptr(i)` takes `0x0101_0101` from `i = 8`; `icdicer(i)` takes `0xFFFF_FFFF`. Then:

```rust
        INTC.iccpmr().write_value(intc::regs::Iccpmr(31u32 << GIC_PRIORITY_SHIFT));
        INTC.iccbpr().write_value(intc::regs::Iccbpr(2));
        INTC.iccicr().write_value(intc::regs::Iccicr(ICCICR_ENABLES | ICCICR_ENABLENS));
        INTC.icddcr().write_value(intc::regs::Icddcr(1));
```

  The order of steps 0–9 is unchanged.
- **`end_stale_active`** (device only):
  - It reads `INTC.icdabr(i).read()` per word. The priority closure reads the word and takes the ID's byte, `(INTC.icdipr(id / 4).read() >> ((id % 4) * 8)) as u8`: the RZ/A1L manual gives the priority registers 32-bit access only (Table 46.1), so the old byte read goes.
  - It writes `INTC.icceoir().write_value(intc::regs::Icceoir(id as u32))`.
- **`enable`/`disable`:** `INTC.icdiser((id >> 5) as usize).write_value(1u32 << (id & 31))`, and `icdicer` likewise.
- **`set_priority`:** read-modify-write on `INTC.icdipr((id / 4) as usize)`, as now: the existing `set_priority` sequence tests still hold.
- **`shutdown`:** `INTC.iccicr().write_value(Iccicr(0)); INTC.icddcr().write_value(Icddcr(0));`.
- **`set_irq_both_edges` / `set_irq_falling_edge`:** `let r = INTC.icr1(); let cleared = r.read().0 & !mask; r.write_value(intc::regs::Icr1(cleared)); r.write_value(intc::regs::Icr1(cleared | (pattern << shift)));`.
- **`clear_irq_pending`:** `let r = INTC.irqrr(); r.write_value(Irqrr(r.read().0 & !(1u16 << irq_num)));`.
- **The dispatcher** (`gic_dispatch`, if it reads IAR/EOIR in Rust) takes `INTC.icciar()` / `INTC.icceoir()`.
- Every fieldset newtype named above lives in `crate::pac::intc::regs`. If one's field isn't `pub`, write through `mmio::write32(reg.addr(), v)` for that register and ledger it.
- **The existing tests** that assert `GICD_ICPENDR0 == 0xE820_1280` etc. are superseded by Step 1's test; delete them. Replace any `GICD_*` constant in an expected `writes()` with its literal.
- **`tools/check-register-tables.sh`:** add `gic.rs`.

Run the gic tests again: all `ok`.

- [ ] **Step 4: The dispatcher stays a load and a store**

```bash
cargo build-fw -p deluge-controller --release 2>&1 | tail -1
llvm-objdump -d --no-show-raw-insn target/armv7a-none-eabihf/release/deluge-controller | awk '/<gic_dispatch>:/,/^$/' > /tmp/claude-1000/p5a-dispatch.txt
grep -cE "bl.*(common|mmio|transmute)" /tmp/claude-1000/p5a-dispatch.txt
```

Expected: `0`. No call into the PAC's handle or the seam survives inlining. If `gic_dispatch` is mangled, find its symbol with `llvm-nm -C … | grep gic_dispatch`. If `llvm-objdump` isn't on PATH, `rust-objdump` (from `cargo-binutils`) takes the same arguments.

- [ ] **Step 5: Verify and commit**

Run the HAL tests, the BSP tests (`encoder.rs`/`trigger_clock.rs` call the IRQ-edge helpers on the device only, and their host tests must still pass), `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/gic.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal)!: the GIC takes its registers from rza1-pac and is sized for the RZ/A1L's 538 interrupts; init and the IRQ-edge helpers go through the seam"
```

---

### Task 10: Docs and final verification

**Files:** `CHANGELOG.md`, `README.md`, `crates/rza1l-hal/{README.md,Cargo.toml}`, `crates/rza1-rt/README.md`, `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md`

- [ ] **Step 1: Docs**

- **`CHANGELOG.md`, top of `### Changed`:**

```markdown
- `rza1-pac`: the RZ/A1L's and RZ/A1H's registers (features `rza1l`, `rza1h`), generated by
  chiptool from the vendor SVD (an RZ/A1H SVD) plus YAML transforms (`tools/gen-pac.sh`). The
  RZ/A1L PAC drops what the RZ/A1L lacks, generated from its manual's register list, and is checked
  against that list both ways; `tools/test.sh` fails on drift or a mismatch. Every access goes
  through the MMIO test seam, which moves from `rza1l-hal` into the PAC (`rza1l_hal::mmio`
  re-exports it). `rza1l_hal::pac` re-exports the crate, built for the RZ/A1L. CPG/STB, BSC, OSTM,
  ADC, the L2 cache, MTU2, SCIF, RSPI, GPIO and the GIC take their registers from it; their
  hand-written address tables are gone, and the L2 cache, the GIC's `init` and the IRQ-edge helpers
  are now host-testable. **Breaking:** `gpio::{p, pm, pmc, pipc}` are no longer public;
  `peripherals::{RSPI3, RSPI4}` are gone (the RZ/A1L has RSPI0–2); `gic::INT_ID_TOTAL` is the
  RZ/A1L's 538, not the RZ/A1H's 587.
```

- **`crates/rza1l-hal/README.md`:** add a short "Registers" paragraph after the module table. Name `rza1_pac` (its `rza1l` feature), `tools/gen-pac.sh`, the manual check, and the seam's new home. Note which modules still hand-write addresses until Phase 5b: DMAC, SSI, SCUX, SDHI, SPIBSC, USB.
- **The part number.** `README.md` (twice), `crates/rza1l-hal/README.md` (twice), `crates/rza1l-hal/Cargo.toml`'s `description` and `crates/rza1-rt/README.md` give the chip as "R7S721001", which is an RZ/A1H part number. The Deluge's chip is the RZ/A1L R7S721020, as `docs/advanced-guide.md` already says. Replace each, then run `rg -n R7S721001 -g '!docs/superpowers/**' .` and expect no output.
- **Spec, after the "4c done" paragraph:**

```markdown
**5a done (2026-10-15 plan):** `rza1-pac` is generated by chiptool (pinned revision, checked in,
drift-checked) for two chips. The vendor SVD is the RZ/A1H's; the `rza1l` PAC deletes what the
RZ/A1L lacks (24 peripherals and parts of eight blocks, generated from the RZ/A1L manual's Table
46.1) and is checked against that table both ways. GPIO, INTC, RSPI and ADC are reshaped by
transforms; the register handle routes through `mmio`, which moved into the PAC. CPG/STB, BSC,
OSTM, ADC, L2C, MTU2, SCIF, RSPI, GPIO and the GIC use it, `tools/check-register-tables.sh` holds
them to the exit criterion, and the HAL lost its RZ/A1H-isms (RSPI3/4, a 587-ID GIC). 5b moves
DMAC, SSI, SCUX, SDHI (a hand-written YAML block: the SVD lacks it, and the RZ/A1L check then
covers it), SPIBSC and USB (a full migration of its register struct).
```

- [ ] **Step 2: Final verification**

- `tools/test.sh > /tmp/claude-1000/p5a-final.log 2>&1; echo "exit $?"`: `exit 0`, with `failed 0` across the summed results.
- `tools/gen-pac.sh --check`: up to date, and the RZ/A1L manual check reports `0 problems`.
- `cargo check --target armv7a-none-eabihf -Zbuild-std=core -p rza1-pac --features rza1h`: the RZ/A1H PAC still builds for the device.
- `tools/check-register-tables.sh`: exit 0, with `MIGRATED` naming the eleven modules: `cpg.rs stb.rs bsc.rs ostm.rs adc.rs cache.rs mtu2.rs uart.rs rspi.rs gpio.rs gic.rs`.
- `tools/link-layout.sh check && tools/check-manifests.sh`: pass.
- The **Firmware build check**.
- The simulator check: `oled_hello` headless gives `cv 0 0 0 0` / `gate 0 0 0 0`, and the frame shows "hello deluge".

```bash
git add CHANGELOG.md crates/rza1l-hal/README.md docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md
git commit -m "docs: Phase 5a — rza1-pac, and the small blocks take their registers from it"
```

---

## Hardware smoke tests (the human partner's merge gate)

Every register access in the migrated modules now compiles through a new path, so the gate exercises each one once:

1. **Boot and clocks (CPG, STB, BSC, L2C):** app-loader boots to its menu; an app chain-loads from it.
2. **Timers (OSTM, MTU2):** `blinky` blinks at its rate; `midi_cv` gate lengths are right (MTU2 ch2 one-shot); encoders turn smoothly (MTU2 ch3 poll).
3. **UART (SCIF):** pads, buttons and LEDs work through the PIC link, including the 31 250 → 200 000 bps switch; DIN MIDI in and out.
4. **RSPI:** the OLED draws and CV out tracks (`cv_gate`).
5. **GPIO:** the sync LED, jack detection and the codec-power GPIO (audio starts).
6. **GIC:** every interrupt-driven path above, plus the trigger-clock input (IRQ6, both edges) and `usb_log` enumerating.
7. **ADC:** whatever reads it on this board, if anything does. Otherwise skip, with a note in the merge report.
