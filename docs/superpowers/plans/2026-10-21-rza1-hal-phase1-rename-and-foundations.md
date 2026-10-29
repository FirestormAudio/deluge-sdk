# rza1-hal Phase 1: Rename and Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- Rename `rza1l-hal` to `rza1-hal` with `rza1l`/`rza1h` chip features.
- Give it embassy's crate-level foundations:
  - optional `log`/`defmt`;
  - `unstable-pac`, `rt` and `critical-section-impl` features;
  - a typed `Config`;
  - `InterruptExt`-style interrupt control;
  - no driver-hard-coded priorities;
  - the time driver started inside `init()`;
  - no public driver `steal()`.
- Migrate every downstream crate in the same phase.

**Architecture:**
- The crate directory moves (`git mv`, history kept) and the crate is renamed; `rza1-pac`'s chip
  is picked by the HAL's chip feature.
- Logging goes through an embassy-style `fmt.rs` shim.
- The `Interrupt` trait gains provided methods over the GIC registers.
- Drivers stop calling `gic::set_priority`. The HAL `Config` carries the two priorities embassy
  configs carry (DMA and the time driver), and `deluge-bsp` sets the rest explicitly to today's
  values.
- `init()` starts the OSTM time driver itself and keeps OSTM0/OSTM1 out of `Peripherals`.

**Tech Stack:** Rust nightly, embassy-hal-internal 0.5, embassy-time-driver 0.2, `log` 0.4, `defmt` 1, chiptool PAC (`rza1-pac`), QEMU ARM host tests.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md` (§4 decisions, §5 phase 1).

## Global Constraints

- Nothing under `rza1/` names `deluge` in code or depends on a `deluge-*` crate. `tools/check-layering.sh` must keep firing; update it for the new crate name.
- **Lockstep:** every task updates `deluge-bsp`, `deluge-sdk`, `usb-classes`, firmwares, hw-tests and examples so the root workspace builds at every commit. DelugeFirmware is **not** migrated in this phase (spec decision 6: one bump at the end of the pass).
- No behaviour change on the Deluge. `tools/link-layout.sh check` must match after every task, apart from the symbol renames this phase makes.
- Every task ends with all of these green:
  - `tools/test.sh` (it runs `rza1/tools/test.sh`);
  - `tools/check-layering.sh`;
  - every firmware, hw-test and example built with
    `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p <pkg>`.
- The deploy target is 32-bit: review any `usize` arithmetic by hand.
- Keep the `mmio.rs` test seam. Register-sequence tests use `rza1_pac::mmio::test` (`reset`, `writes`).
- Never push, never publish.

## Review Focus

1. **An interrupt's priority silently changes.** Drivers used to set USB, SCIF, SDHI, DMAC and OSTM to 10/10/10/13/14. If `deluge-bsp` misses one, it falls to the new GIC default (P16) and the Deluge's nesting order changes, e.g. USB no longer preempting DMA completions. Pinned by Task 5's `board_irq_priorities_match_the_pre_phase1_values` test.
2. **`pend()` on an SGI vs an SPI.** SGIs (IDs 0–15) can only be raised through ICDSGIR, and writing ICDISPR for them does nothing. A wrong path means an `InterruptExecutor` on an SGI never wakes. Pinned by Task 4's two `pend` tests.
3. **The time driver now starts before `Board::new`'s MMU, cache and SDRAM bring-up.** Anything that read `Instant::now()` or armed a timer in between used to see a stopped clock and now sees a running one. The app-loader's hand-off (`time_driver::stop()` then `gic::shutdown()`) must still quiesce it. Pinned by Task 6's host test of `init`'s call order and the Task 8 bench check of the app-loader hand-off.
4. **`defmt` builds and no-logging builds.** A format string that `log` accepts but `defmt` doesn't (`{:?}` on a non-`Format` type, `{:p}`) breaks only the `defmt` build. With neither feature on, the macros must still consume their arguments, so there are no unused-variable warnings. Pinned by Task 2's two extra builds in `rza1/tools/test.sh`.
5. **Chip-feature misuse.** With no chip feature or with both, the build must fail with a clear `compile_error!`, not a page of missing PAC items. `rza1h` must build. Pinned by Task 1's checks in `rza1/tools/test.sh`.

---

## File Structure

| Path | Change |
|---|---|
| `rza1/rza1l-hal/` → `rza1/rza1-hal/` | `git mv`; crate `rza1-hal`, lib `rza1_hal` |
| `rza1/rza1-hal/Cargo.toml` | chip, `log`, `defmt`, `unstable-pac`, `rt`, `critical-section-impl`, `unstable-driver-steal` features |
| `rza1/rza1-hal/src/fmt.rs` (new) | `trace!`…`error!`, `unwrap!`, `panic!` shims over `log`/`defmt`/nothing |
| `rza1/rza1-hal/src/lib.rs` | chip `compile_error!`, gated re-exports, `Config`, `init` |
| `rza1/rza1-hal/src/config.rs` (new) | `Config`, `CpgConfig`, `ModuleClocks`, `ModuleClock` |
| `rza1/rza1-hal/src/interrupt.rs` | `Priority`, provided `Interrupt` methods |
| `rza1/rza1-hal/src/gic.rs` | default priority P16; `set_priority`/`enable`/`disable` become `pub(crate)` |
| `rza1/rza1-hal/src/{uart,usb/mod,sdhi,dmac,mtu2,time_driver}.rs` | no hard-coded priorities |
| `rza1/rza1-hal/src/time_driver.rs` | `pub(crate) fn start`, own handler binding |
| `rza1/rza1-hal/src/{dmac,rspi,uart}.rs` | `steal` behind `unstable-driver-steal` |
| `rza1/Cargo.toml`, `rza1/tools/*.sh`, `rza1/tools/pac-postprocess.py`, `rza1/rza1-rt/link*.x`, `rza1/rza1-rt/src/lib.rs` | new name |
| Root `Cargo.toml`, `.cargo/config.toml`, `tools/check-layering.sh`, 76 `.rs` files, every downstream `Cargo.toml`, README/CONTRIBUTING/docs | `rza1l_hal` → `rza1_hal`, `features = ["rza1l"]` |
| `crates/deluge-bsp/src/{system,board,encoder,trigger_clock,bus,uart}.rs` | typed `BOARD_CONFIG`, explicit priorities, no time-driver init, steal feature |

---

### Task 1: Rename to `rza1-hal` with chip features

**Files:**
- Move: `rza1/rza1l-hal` → `rza1/rza1-hal`
- Modify: `rza1/rza1-hal/Cargo.toml`, `rza1/rza1-hal/src/lib.rs`, `rza1/rza1-hal/src/gic.rs:7,55`, `rza1/Cargo.toml`, `rza1/tools/{test.sh,check-register-tables.sh,pac-postprocess.py}`, `rza1/rza1-rt/{link.x,link_rtt.x,src/lib.rs}`, root `Cargo.toml`, `.cargo/config.toml`, `tools/check-layering.sh`, all downstream manifests and `.rs` files, README.md, CONTRIBUTING.md, docs/getting-started.md, docs/advanced-guide.md.
- Leave alone: `docs/superpowers/**` (history), `CHANGELOG.md` past entries.

**Interfaces:**
- Produces:
  - crate `rza1-hal`, lib `rza1_hal`;
  - features `rza1l`, `rza1h` (exactly one);
  - `gic::INT_ID_TOTAL` per chip (A1L 538, A1H 587);
  - module `sdhi` and peripherals `SDHI0`/`SDHI1` exist only with `rza1l`;
  - peripherals `RSPI3`/`RSPI4` and their `rspi::Instance` impls exist only with `rza1h`.

- [ ] **Step 1: Add the chip checks to `rza1/tools/test.sh`** (they fail now: the crate doesn't exist)

After the dependency-closure check, add a section that requires all four outcomes:
- `cargo check -p rza1-hal --no-default-features` fails and its stderr contains `exactly one chip feature`;
- `--features rza1l,rza1h` fails the same way;
- `-Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem --features rza1h` builds for `armv7a-none-eabihf`;
- `--features rza1l` builds.

Switch the existing QEMU test and device build lines to `-p rza1-hal --features rza1l`.

- [ ] **Step 2: Run it to see it fail**

Run: `rza1/tools/test.sh`
Expected: FAIL, `package ID specification 'rza1-hal' did not match any packages`.

- [ ] **Step 3: Rename and gate**

1. `git mv rza1/rza1l-hal rza1/rza1-hal`.
2. Set `name = "rza1-hal"` and description "Embassy HAL for the Renesas RZ/A1 (RZ/A1L, RZ/A1H; ARM Cortex-A9)."
3. Depend on `rza1-pac` with no chip feature. Add the features
   `rza1l = ["rza1-pac/rza1l"]` and `rza1h = ["rza1-pac/rza1h"]`; neither is a default.
4. In `lib.rs`, add `#[cfg(not(any(feature = "rza1l", feature = "rza1h")))] compile_error!("rza1-hal: enable exactly one chip feature: rza1l or rza1h");` and the same check for both enabled.
5. Make `INT_ID_TOTAL` a `cfg`'d constant: 538 for `rza1l`, 587 for `rza1h`. The `gic.rs:7` doc says 587 today; fix it to say which chip.
6. Gate on `rza1l`: the `sdhi` module, `SDHI0`, `SDHI1`, `SDHI*` typelevel interrupts. The A1H PAC has no SDHI block.
7. Add `#[cfg(feature = "rza1h")] RSPI3, RSPI4` to `peripherals!` (`peripherals!` accepts `#[cfg]` per entry) with their `rspi::Instance` impls. Take base addresses and interrupt IDs from `rza1_pac::RSPI3`/`RSPI4` and the A1H interrupt table.
8. Update `docs.rs` metadata: `features = ["rza1l"]`.

- [ ] **Step 4: Rename every use**

- Replace `rza1l_hal` → `rza1_hal` and `rza1l-hal` → `rza1-hal` across the files listed above. Downstream manifests depend on `rza1-hal = { path = …, features = ["rza1l"] }`.
- In `tools/check-layering.sh`, rule 1 must still fire on a `deluge` name planted under `rza1/rza1-hal/src`. Plant one, run the script, see it fail, then remove it.

- [ ] **Step 5: Verify**

Run:
- `rza1/tools/test.sh`
- `tools/test.sh`
- `tools/check-layering.sh`
- `tools/link-layout.sh check`
- the firmware/hw-test/example builds.

Expected:
- All pass.
- `link-layout` differs only in mangled `rza1l_hal` → `rza1_hal` symbol names. If the script compares names, regenerate its baseline in this commit and say so in the message.
- `grep -rn "rza1l_hal\|rza1l-hal" --exclude-dir=target --exclude-dir=superpowers .` prints only CHANGELOG history.

- [ ] **Step 6: Commit**

```bash
git add -A rza1 crates firmwares hw-tests examples usb drivers linux tools .cargo Cargo.toml Cargo.lock README.md CONTRIBUTING.md docs/getting-started.md docs/advanced-guide.md
git commit -m "rza1-hal: rename from rza1l-hal, with rza1l/rza1h chip features"
```

---

### Task 2: Optional logging and the crate feature gates

**Files:**
- Create: `rza1/rza1-hal/src/fmt.rs`
- Modify: `rza1/rza1-hal/Cargo.toml`, `rza1/rza1-hal/src/lib.rs`, every `log::…!` call site in `rza1/rza1-hal/src` (87 calls: 48 `debug`, 24 `trace`, 10 `warn`, 3 `info`, 2 `error`), `rza1/Cargo.toml`, `rza1/tools/test.sh`, downstream manifests that need `log`, `unstable-pac` or `rt`.

**Interfaces:**
- Produces the following features. Default: `["rt", "critical-section-impl", "time-driver-ostm"]`; chip and logging features are never default.

  | Feature | What it does |
  |---|---|
  | `log` | `dep:log` (`log = { workspace = true, optional = true }`) |
  | `defmt` | `dep:defmt` (`defmt = { version = "1", optional = true }`); `defmt::Format` derives on public error and config types |
  | `unstable-pac` | gates `pub use rza1_pac as pac` |
  | `rt` | gates the `#[unsafe(no_mangle)] gic_dispatch` export that `rza1-rt`'s IRQ stub calls |
  | `critical-section-impl` | turns on cortex-ar's `critical-section-single-core` |

- `pub use rza1_pac::mmio` stays ungated: it is the documented test seam.
- Crate-internal macros `trace!`, `debug!`, `info!`, `warn!`, `error!`, `unwrap!`.

- [ ] **Step 1: Add the build matrix to `rza1/tools/test.sh`**

Add a device build of `rza1-hal --features rza1l` once with `log`, once with `defmt`, and once with neither. Pass `RUSTFLAGS=-Dwarnings` for the neither-feature build only, so unused-argument warnings fail it.

- [ ] **Step 2: Run to see the `defmt` build fail**

Run: `rza1/tools/test.sh`
Expected: FAIL, `the package 'rza1-hal' does not contain this feature: defmt`.

- [ ] **Step 3: Implement**

1. Copy embassy-hal-internal 0.5's `src/fmt.rs` pattern into `src/fmt.rs` (in `~/.cargo/registry/src/*/embassy-hal-internal-0.5.0/src/fmt.rs`). Keep only the macros the crate uses, and declare `#[macro_use] mod fmt;` first in `lib.rs`. With neither feature on, each macro expands to `let _ = ($( & $x ),*);`.
2. Replace every `log::x!(…)` with `x!(…)`.
3. Rewrite format arguments `defmt` can't take:
   - `{:p}` → `{:#x}` on `ptr as usize`;
   - `{:?}` on types without `Format` → derive `Format` under `cfg_attr(feature = "defmt", derive(defmt::Format))`, or log a field instead.
4. In `rza1/Cargo.toml`, drop `features = ["release_max_level_info"]` from `log`: the HAL must not set a downstream crate's level. The root workspace keeps its own `log` features.
5. Downstream crates that read HAL logs (`deluge-bsp`) enable `rza1-hal/log`. `hw-tests/vbus-probe` enables `unstable-pac`.

- [ ] **Step 4: Verify**

Run: `rza1/tools/test.sh && tools/test.sh`, then the firmware builds.
Expected: all pass, and the three logging builds are warning-free.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: optional log/defmt via fmt.rs; unstable-pac, rt, critical-section-impl features"
```

---

### Task 3: Typed `Config`

**Files:**
- Create: `rza1/rza1-hal/src/config.rs`
- Modify: `rza1/rza1-hal/src/lib.rs:49-86`, `rza1/rza1-hal/src/stb.rs`, `rza1/rza1-hal/src/cpg.rs`, `crates/deluge-bsp/src/system.rs:27-88`
- Test: `rza1/rza1-hal/src/config.rs` (`#[cfg(all(test, not(target_os = "none")))]`), `crates/deluge-bsp/src/system.rs` tests

**Interfaces:**
- Produces:

```rust
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub clocks: clocks::ClockConfig,
    pub cpg: CpgConfig,
    pub module_clocks: ModuleClocks,
    pub dma_interrupt_priority: interrupt::Priority,  // used from Task 5
    pub time_interrupt_priority: interrupt::Priority, // used from Task 6
}
impl Config {
    pub const fn new(clocks: clocks::ClockConfig) -> Self; // cpg/module_clocks/priorities = their defaults
    pub const fn with_cpg(self, cpg: CpgConfig) -> Self;
    pub const fn with_module_clocks(self, m: ModuleClocks) -> Self;
    pub const fn with_dma_interrupt_priority(self, p: interrupt::Priority) -> Self;
    pub const fn with_time_interrupt_priority(self, p: interrupt::Priority) -> Self;
}

#[non_exhaustive] #[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpgConfig { pub ifc: IClockDivider, pub clock_output: ClockOutput, pub clock_output2: ClockOutput2 }
impl Default for CpgConfig // the FRQCR reset value's settings
impl CpgConfig { pub(crate) const fn frqcr(&self) -> u16; pub const fn new() -> Self /* = default */; /* const with_* per field */ }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ModuleClocks(/* private */);           // Default: every module clock stopped
#[non_exhaustive] #[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleClock { /* one variant per STBCR2–12 bit the manual names: Ostm0, Ostm1, Mtu2, Adc, Scif0…Scif4, Usb0, Usb1, Scux, Spibsc0, Rspi0…Rspi2 (+Rspi3/4 on rza1h), Ssif0…Ssif3, Sdhi0, Sdhi1, CoreSight, Dvdec0, Dvdec1, … */ }
impl ModuleClocks {
    pub const fn new() -> Self;
    pub const fn with(self, m: ModuleClock) -> Self;
    pub(crate) const fn stbcr(&self, n: u8) -> u8; // n in 2..=12; a 0 bit runs the clock
}
```

- Mirror the field and enum names of `IClockDivider`, `ClockOutput` and `ClockOutput2` on the PAC's FRQCR fields (`vals::Ifc`, `vals::Ckoen`, `vals::Ckoen2`). Keep them as HAL types, because the PAC is behind `unstable-pac`.
- **No `Default` for `Config`.** The RZ/A1 has no internal oscillator, so there is no crystal frequency that is safe to assume, and a wrong default would silently mis-time every peripheral. This deviates from spec §4 ("Config with Default"). Say so in the commit message, and report it to the user at the end of the phase.

- [ ] **Step 1: Write the failing tests**

```rust
// rza1-hal config.rs
#[test] fn module_clocks_default_stops_every_module()   // stbcr(n) == 0xFF for n in 2..=12, except reserved bits per the manual
#[test] fn with_a_module_clears_its_stbcr_bit()        // ModuleClocks::new().with(ModuleClock::Ostm0).stbcr(5) has bit for OSTM0 clear
#[test] fn cpg_default_encodes_the_frqcr_reset_value()  // CpgConfig::default().frqcr() == the manual's FRQCR reset value
#[test] fn init_writes_frqcr_then_stbcr2_to_12()       // keep the existing init_tests order assertion, now from a typed Config

// deluge-bsp system.rs
#[test] fn board_config_encodes_to_the_pre_phase1_registers() {
    assert_eq!(BOARD_CONFIG.cpg.frqcr(), 0x1035);   // via a pub(crate)-for-test accessor or the mmio write log
    let want = [0x6A, 0x80, 0x00, 0x00, /* … the 11 old STBCR bytes from system.rs today, in order 2..=12 */];
    // assert each stbcr(n) == want[n-2]
}
```

Before rewriting `system.rs`, copy the 11 old STB bytes from it into this test.

- [ ] **Step 2: Run to see them fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib config` (inside `rza1/`)
Expected: FAIL to compile, because `ModuleClocks` is not defined.

- [ ] **Step 3: Implement**

1. Add the types.
2. Make `init` take the typed `Config`; `cpg::init` and `stb::init` take the encoded values.
3. Make `StbConfig` `pub(crate)`, or delete it in favour of `ModuleClocks`.
4. Rewrite `BOARD_CONFIG` as a `const` built with `Config::new(CLOCKS).with_cpg(…).with_module_clocks(…)`. Name each module, and move the doc table in `system.rs:8-25` onto the named modules.

- [ ] **Step 4: Verify**

Run: both test commands, `tools/test.sh`, the firmware builds, and `tools/link-layout.sh check`.
Expected: all pass, and the Deluge's FRQCR/STBCR writes are unchanged.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: typed Config (CpgConfig, ModuleClocks) with const builders; no Default (no safe crystal default)"
```

---

### Task 4: Interrupt control on the interrupt types

**Files:**
- Modify: `rza1/rza1-hal/src/interrupt.rs`, `rza1/rza1-hal/src/gic.rs`
- Test: `rza1/rza1-hal/src/interrupt.rs` tests

**Interfaces:**
- Produces:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Priority { P0 = 0, P1, …, P30 }   // 0 = highest; 31 is the PMR mask level, so not offered
impl Default for Priority { fn default() -> Self { Priority::P16 } }

pub trait Interrupt: 'static {
    const ID: u16;
    unsafe fn enable() { … }          // ICDISER
    fn disable() { … }                // ICDICER
    fn is_enabled() -> bool { … }     // ICDISER read
    fn is_pending() -> bool { … }     // ICDISPR read
    fn pend() { … }                   // SGI (ID < 16): ICDSGIR = (0b10 << 24) | ID (target: this CPU); else ICDISPR bit
    fn unpend() { … }                 // ICDICPR bit (SGIs: no-op on A9 UP; document)
    fn get_priority() -> Priority { … }  // ICDIPR byte >> 3
    fn set_priority(p: Priority) { … }   // existing byte-lane write, inside a critical section
}
```

- `gic::init` writes `Priority::default()` (P16 → `0x8080_8080`) into every ICDIPR word instead of P31. PMR stays at 31.
- `gic::set_priority`, `gic::enable` and `gic::disable` become `pub(crate)`. The IRQ0–7 edge helpers and `set_sdram_code_window`/`shutdown` stay public. GPIO pin interrupts are Phase 2.

- [ ] **Step 1: Write the failing tests** (mmio write-log, addresses from the GIC map at `0xE820_1000`)

```rust
#[test] fn pend_on_an_spi_sets_its_ispr_bit()      { USB0::pend(); assert_eq!(writes(), [(0xE820_1208, 1 << 9)]); }   // ID 73
#[test] fn pend_on_an_sgi_writes_sgir_to_self()    { SGI3::pend(); assert_eq!(writes(), [(0xE820_1F00, (0b10 << 24) | 3)]); }
#[test] fn unpend_clears_its_icpr_bit()            { USB0::unpend(); assert_eq!(writes(), [(0xE820_1288, 1 << 9)]); }
#[test] fn set_priority_writes_its_byte_lane()     { DMAINT4::set_priority(Priority::P13); assert_eq!(writes(), [(0xE820_142C, (13 << 3) << 8)]); }
#[test] fn get_priority_reads_back_what_was_set()  // set P13 then get == P13 on the shadow
#[test] fn gic_init_defaults_every_priority_to_p16() // every ICDIPR write in init is 0x8080_8080
```

Move the existing `setting_a_priority_writes_its_byte_lane` and `enabling…`/`disabling…` tests onto the trait methods.

- [ ] **Step 2: Run to see them fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib interrupt` (in `rza1/`)
Expected: FAIL to compile, because `Priority` is not defined.

- [ ] **Step 3: Implement**

Add the provided methods over the PAC's `INTC` registers (`icdispr`, `icdicpr`, `icdsgir`, `icdiser`, `icdicer`, `icdipr`). Migrate the HAL's internal `gic::enable`/`set_priority` calls on typelevel interrupts to the trait methods.

- [ ] **Step 4: Verify**

Run: the test command, then `rza1/tools/test.sh`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: Priority and enable/disable/pend/unpend/set_priority on interrupt types; GIC default P16"
```

---

### Task 5: Drivers stop hard-coding priorities

**Files:**
- Modify:
  - `rza1/rza1-hal/src/uart.rs:83,496-497`, `usb/mod.rs:142,153`, `sdhi.rs:243,1522-1523`, `dmac.rs:66,157,345`, `mtu2.rs:164` (`OneShot::new` loses its `priority` parameter);
  - `crates/deluge-bsp/src/{board.rs,encoder.rs:276,trigger_clock.rs:95}` and every `mtu2::OneShot::new` caller.
- Test: `crates/deluge-bsp/src/board.rs` (host test).

**Interfaces:**
- Consumes `Priority` and the `Interrupt` methods from Task 4, and `Config::dma_interrupt_priority` from Task 3.
- Produces:
  - DMAC channels get `Config::dma_interrupt_priority` (stored at `init`, read by `Dma::new`), which is embassy-stm32's `dma_interrupt_priority`.
  - Every other driver leaves its interrupt priority alone.
  - In `deluge-bsp`, `pub(crate) fn apply_irq_priorities()` is called from `Board::new`, and `BOARD_CONFIG` uses `.with_dma_interrupt_priority(Priority::P13)`.

- [ ] **Step 1: Write the failing test**

```rust
// deluge-bsp board.rs
#[test]
fn board_irq_priorities_match_the_pre_phase1_values() {
    test::reset();
    apply_irq_priorities();
    // (GIC ID, priority) the drivers used to set: USB0/USB1 73/74 → 10; SCIF0/SCIF1 ERI 222/226, TXI 224/228 → 10;
    // SDHI1 status/op 305/306 → 10; the encoders' IRQ0–4,7 and MTU2_3 TGI3A → ENCODER_PRIORITY;
    // IRQ6 (trigger) → TRIG_PRIORITY; MTU2_2 TGI2A → the gate timer's current priority.
    for (id, prio) in EXPECTED { assert_eq!(gic_priority_from_writes(id), prio, "GIC {id}"); }
}
```

`gic_priority_from_writes` decodes the byte lane from `test::writes()`. Read `EXPECTED` from the driver constants and call sites listed above before deleting them.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host-uac,usb-host-overbridge --lib board_irq`
Expected: FAIL to compile, because `apply_irq_priorities` is not defined.

- [ ] **Step 3: Implement**

1. Delete `UART_IRQ_PRIORITY`, `USB_IRQ_PRIORITY`, `SDHI_IRQ_PRIORITY` and the `set_priority` calls in those drivers.
2. Read DMAC's priority from `Config`.
3. Remove `OneShot::new`'s `priority` parameter.
4. In `deluge-bsp`, set the board's priorities on the typed interrupts: `typelevel::USB0::set_priority(Priority::P10)`, etc.
   - `encoder.rs` stops looping over raw `gic_id`s: it sets and enables IRQ0–4 and IRQ7 through their types.
   - `trigger_clock.rs` uses `IRQ6`.

- [ ] **Step 4: Verify**

Run: the test, `tools/test.sh`, the firmware builds, and `tools/link-layout.sh check`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: drivers leave interrupt priorities to the application; deluge-bsp sets the Deluge's explicitly"
```

---

### Task 6: The time driver starts inside `init()`

**Files:**
- Modify: `rza1/rza1-hal/src/time_driver.rs:215-245`, `rza1/rza1-hal/src/lib.rs` (`peripherals!`, `init`), `crates/deluge-bsp/src/board.rs:104-126`, `crates/deluge-bsp/src/{audio.rs:53,66,119,cv_gate.rs:72}` (docs).
- Test: `rza1/rza1-hal/src/lib.rs` `init_tests`.

**Interfaces:**
- Consumes `Config::time_interrupt_priority` (Task 3) and `Interrupt::set_priority` (Task 4).
- Produces:
  - `pub(crate) fn time_driver::start(priority: Priority)`. It binds `OverflowHandler`/`AlarmHandler` itself through `gic::set_handler`, so no user `bind_interrupts!` is needed. Both handlers become private.
  - `pub unsafe fn time_driver::stop()` is unchanged; the app-loader uses it.
  - With `time-driver-ostm` on, `OSTM0`/`OSTM1` are absent from `Peripherals` and `typelevel::OSTMI0/1` are `pub(crate)`.
  - `BOARD_CONFIG` uses `.with_time_interrupt_priority(Priority::P14)`.
  - `Board::new` no longer starts the time driver, and `TimeIrqs` is deleted.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn init_starts_the_time_driver_after_the_gic() // host-testable split: factor init's body into
                                               // `unsafe fn bring_up(&Config)` and assert, on the mmio log, that the OSTM0
                                               // start (OSTM0 base 0xFCFE_C000 writes) and the OSTMI0/OSTMI1 priority bytes (= P14)
                                               // come after the last GIC ICDIPR write
```

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-hal --features rza1l --lib init_starts`
Expected: FAIL to compile, because `bring_up` is not defined.

- [ ] **Step 3: Implement**

1. `init` order: clocks → CPG → module clocks → warm-start quiesce → GIC init → `time_driver::start(config.time_interrupt_priority)`, behind `#[cfg(feature = "time-driver-ostm")]`.
2. Put `#[cfg(not(feature = "time-driver-ostm"))]` on the `OSTM0, OSTM1` entries in `peripherals!`.
3. Delete `TimeIrqs` and the `time_driver::init` call in `Board::new`.

- [ ] **Step 4: Verify**

Run: the test, `tools/test.sh`, and the firmware builds.
Expected: PASS. `grep -rn "time_driver::init" --exclude-dir=target --exclude-dir=superpowers .` finds nothing.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: init() starts the OSTM time driver; OSTM0/1 leave Peripherals under time-driver-ostm"
```

---

### Task 7: No public driver `steal()`

**Files:**
- Modify: `rza1/rza1-hal/src/dmac.rs:185`, `rspi.rs:336`, `uart.rs:598`, `rza1/rza1-hal/Cargo.toml`, `crates/deluge-bsp/Cargo.toml`, `crates/deluge-bsp/src/{bus.rs:222-232,uart.rs:188-191}`.

**Interfaces:**
- Produces:
  - `Dma::steal`, `Rspi::steal` and `UartRx::steal` become `#[doc(hidden)]` and exist only with the feature `unstable-driver-steal`. Its doc comment reads: "for panic and fault paths that must reach an initialised driver without its owner; not covered by semver".
  - `deluge-bsp` enables the feature for `steal_rspi0` and `steal_pic_rx`.
  - Singleton `steal` (`peripherals::X::steal()`, from `peripherals!`) is unchanged.

- [ ] **Step 1: Add the build check**

In `rza1/tools/test.sh`, add a step that fails if `cargo doc -p rza1-hal --features rza1l` output (`target/armv7a-none-eabihf/doc/rza1_hal/**/*.html`) mentions `fn.steal` or `method.steal` on `Dma`, `Rspi` or `UartRx`.

- [ ] **Step 2: Run to see it fail**

Run: `rza1/tools/test.sh`
Expected: FAIL, because `Dma::steal` is documented.

- [ ] **Step 3: Implement**

Gate the three functions behind the feature and mark them `#[doc(hidden)]`, then enable the feature in `deluge-bsp`.

- [ ] **Step 4: Verify**

Run: `rza1/tools/test.sh && tools/test.sh`, then the firmware builds.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git commit -am "rza1-hal: driver steal() behind the hidden unstable-driver-steal feature"
```

---

### Task 8: Phase close and bench smoke check

Phase 1 is not on the spec's bench-check list, but it moves every interrupt priority and the time driver's start. Do one bench run before merging.

- [ ] **Step 1: Full checks**

Run:
- `tools/test.sh`
- `tools/check-layering.sh`
- `tools/link-layout.sh check`
- every firmware, hw-test and example build
- `cd rza1 && cargo doc --no-deps --features rza1l`

Expected: all pass. `cargo doc` warnings are no more than before this phase; they are cleaned up in Phase 8.

- [ ] **Step 2: Bench**

On the pi-top bench (`deluge_jlink.py --select IP=pi-top`), confirm each of these:
- `demo-firmware` boots via the app-loader hand-off;
- the OLED draws;
- the encoders and pads respond;
- MIDI in reaches the log;
- USB audio plays to the speaker;
- `Timer::after_millis` blinks keep time against the RTT timestamps.

- [ ] **Step 3: CHANGELOG**

Add an `[Unreleased]` entry covering:
- the rename;
- the new features;
- the typed `Config` and that it has no `Default`;
- the `Priority` API and P16 GIC default;
- drivers no longer setting priorities;
- the time driver started in `init`;
- `steal` behind `unstable-driver-steal`.

Note that DelugeFirmware#next is expected to break until Phase 9.

- [ ] **Step 4: Commit**

```bash
git commit -am "CHANGELOG: rza1-hal phase 1"
```
