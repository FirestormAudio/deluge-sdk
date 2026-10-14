# RZ/A1 Phase 1: Board Facts Out of the HAL — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `rza1l-hal` board-agnostic. Every Deluge-specific fact (crystal frequency, SDRAM size, fault-hook name) moves to `deluge-bsp`. `deluge-sdk` stops re-exporting the HAL. `tools/check-layering.sh` enforces both. No behaviour change on the device.

**Architecture:** A new `rza1l_hal::clocks` module holds a `ClockConfig { extal_hz }` with `const fn` derivations (P1φ, P0φ, an exact P0 ticks-per-µs rational) and a runtime store set once at boot. `deluge_bsp::system::CLOCKS` names the Deluge's 13,225,625 Hz crystal, and `init_clocks` stores it before anything else. OSTM delays, the RSPI baud divider and the time driver read the store. The BSP's MTU2 tick constants use the `const fn`s directly. Every derived number is pinned in tests to the value the old hardcoded constant had.

**Tech Stack:** Rust nightly, `armv7a-none-eabihf` firmware, Embassy (`embassy-time-driver`), host and QEMU (`armv7-unknown-linux-gnueabihf`) unit tests, bash.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 1 of 6).

## Global Constraints

- **No behaviour change on the device.** These values must come out the same as today: OSTM delay 33,064 ticks/ms; RSPI SPBR 3 at 10 MHz and 8 at 4 MHz; MTU2 `TICK_HZ` = 516,625 and `POLL_TICKS` = 516; time driver ratio 21161/640 ticks per µs; `MAX_ARM` = 3,300,000 ticks.
- **The Deluge crystal is 13,225,625 Hz.** It is written exactly once, in `deluge_bsp::system::CLOCKS`. HAL code may only use it in `#[cfg(test)]` (`clocks::TEST_CLOCKS`).
- **Keep the MMIO seam.** Register tests keep using `crate::mmio::test`.
- **Worktree:** work on branch `refactor/rza1-board-facts` in `~/GitHub/deluge-sdk`, or in a *sibling* worktree (e.g. `~/GitHub/deluge-sdk-rza1`). **Never** a nested `.worktrees/` dir: the workspace's `../flare` path deps would not resolve there.
- **Commit style:** conventional commits scoped like the log, e.g. `feat(rza1l-hal): …`, `refactor(bsp): …`, `chore(layering): …`.
- **Don't edit history docs:** leave the rest of `docs/superpowers/**` alone. Add to `CHANGELOG.md` under `## [Unreleased]` only.
- **Test commands:**
  - HAL: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib`
  - BSP: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`
  - Device build check: `cargo build-fw -p deluge-controller`

## Review Focus

1. **A clock-dependent driver used before `clocks::init`** (e.g. `ostm::delay_ms` in early boot or in a panic path) must panic with a message naming `rza1l_hal::clocks::init`. It must not divide by zero or spin forever. Pinned by `store_is_unset_until_set` (Task 1) and the boot-order audit in Task 2, Step 6.
2. **`embassy_time::Instant::now()` before `time_driver::init`** must return 0, not divide by zero. An alarm armed before init must panic with a clear message, not loop forever in `schedule_wake`. Pinned by `unset_ratio_reads_zero_time` and `unset_ratio_is_not_set` (Task 4).
3. **Long uptime:** `ticks_to_us` must not overflow at any 64-bit tick count, for any crystal. Pinned by `ticks_to_us_matches_u128_reference_across_crystals` and `ticks_to_us_survives_u64_max` (Task 4).
4. **A non-Deluge crystal** (the RZ/A1L's nominal 13.33 MHz) must give a correctly reduced ratio and correct dividers. Pinned by `nominal_crystal_ratio_is_reduced` (Task 1) and the u128 reference sweeps (Task 4).
5. **Parallel host tests share one global clock store.** Every test that sets it must set the same `TEST_CLOCKS`, so test threads can't race to different values. Enforced by convention: tests call `clocks::init(clocks::TEST_CLOCKS)` and nothing else.

---

## Before you start: baseline

- [ ] **B1: Record the baseline** so pre-existing failures aren't blamed on this work.

```bash
cd ~/GitHub/deluge-sdk
git switch -c refactor/rza1-board-facts
tools/test.sh 2>&1 | tail -20
tools/build-examples.sh 2>&1 | tail -5
cargo build-fw -p deluge-controller 2>&1 | tail -3
cargo build-app-loader 2>&1 | tail -3
cargo build-wren 2>&1 | tail -3
cargo build-msc 2>&1 | tail -3
```

Write down anything that already fails. From here on, every "Expected: PASS" means "no new failures compared with this baseline".

---

### Task 1: `rza1l_hal::clocks`

**Files:**
- Create: `crates/rza1l-hal/src/clocks.rs`
- Modify: `crates/rza1l-hal/src/lib.rs` (add `pub mod clocks;` above `pub mod dmac;`)

**Interfaces:**
- Produces:
  - `pub struct ClockConfig { pub extal_hz: u32 }` (`Clone, Copy, Debug, PartialEq, Eq`)
  - `pub const fn ClockConfig::new(extal_hz: u32) -> ClockConfig`
  - `pub const fn ClockConfig::p1_hz(&self) -> u32`
  - `pub const fn ClockConfig::p0_hz(&self) -> u32`
  - `pub const fn ClockConfig::p0_ticks_per_us(&self) -> (u32, u32)`, a reduced `(num, den)`
  - `pub fn init(config: ClockConfig)`
  - `pub fn get() -> ClockConfig` (panics if `init` hasn't run)
  - `pub fn try_get() -> Option<ClockConfig>`
  - `#[cfg(test)] pub(crate) const TEST_CLOCKS: ClockConfig` (13,225,625 Hz)

- [ ] **Step 1: Write the module with its tests and stubbed bodies.** Create `crates/rza1l-hal/src/clocks.rs`:

```rust
//! The RZ/A1L clock tree, derived from the board's EXTAL crystal.
//!
//! The CPG runs the PLL at ×30, and the peripheral clocks are fixed fractions of it:
//! P1φ = EXTAL × 30 / 6 and P0φ = EXTAL × 30 / 12. The crystal is a board fact, so the HAL
//! takes it as a [`ClockConfig`] from the BSP and derives everything else.
//!
//! P0φ is a half-integer for an odd crystal (13,225,625 Hz gives 33,064,062.5 Hz), so code that
//! needs it exactly (the Embassy time driver) uses [`ClockConfig::p0_ticks_per_us`], not the
//! truncated [`ClockConfig::p0_hz`].
//!
//! The BSP calls [`init`] once, first thing at boot. Drivers then read the clocks with [`get`].

use core::sync::atomic::{AtomicU32, Ordering};

/// The board's clock source. Everything else in the clock tree is derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockConfig {
    /// EXTAL crystal frequency in Hz.
    pub extal_hz: u32,
}

impl ClockConfig {
    /// A clock tree driven by an `extal_hz` crystal.
    pub const fn new(extal_hz: u32) -> Self {
        todo!()
    }

    /// P1φ in Hz (EXTAL × 30 / 6). Exact.
    pub const fn p1_hz(&self) -> u32 {
        todo!()
    }

    /// P0φ in Hz (EXTAL × 30 / 12), truncated to a whole Hz.
    pub const fn p0_hz(&self) -> u32 {
        todo!()
    }

    /// P0φ ticks per microsecond as an exact, fully reduced rational `(num, den)`.
    ///
    /// For the Deluge's 13,225,625 Hz crystal this is `(21161, 640)`. `num` < 2^27 and
    /// `den` ≤ 2,000,000 < 2^21 for any crystal the part accepts.
    pub const fn p0_ticks_per_us(&self) -> (u32, u32) {
        todo!()
    }
}

/// The clock tree the BSP registered, as its crystal frequency (0 = not registered yet).
struct Store(AtomicU32);

impl Store {
    const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    fn set(&self, config: ClockConfig) {
        todo!()
    }

    fn get(&self) -> Option<ClockConfig> {
        todo!()
    }
}

static STORE: Store = Store::new();

/// Register the board's clock tree. Call once at boot, before any clock-dependent driver.
pub fn init(config: ClockConfig) {
    STORE.set(config);
}

/// The registered clock tree.
///
/// # Panics
/// If [`init`] has not been called.
pub fn get() -> ClockConfig {
    STORE
        .get()
        .expect("rza1l_hal::clocks::init was not called before a clock-dependent driver")
}

/// The registered clock tree, or `None` before [`init`].
pub fn try_get() -> Option<ClockConfig> {
    STORE.get()
}

/// The Deluge's crystal, for HAL unit tests only. Every test that registers clocks uses this one
/// value, because the store is global and test threads run in parallel.
#[cfg(test)]
pub(crate) const TEST_CLOCKS: ClockConfig = ClockConfig::new(13_225_625);

const fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn deluge_crystal_derives_the_values_the_old_constants_had() {
        // The removed `rspi::P1_HZ`, `ostm::OSTM_HZ`/`mtu2::MTU2_P0_HZ`, and time_math ratio.
        assert_eq!(TEST_CLOCKS.p1_hz(), 66_128_125);
        assert_eq!(TEST_CLOCKS.p0_hz(), 33_064_062);
        assert_eq!(TEST_CLOCKS.p0_ticks_per_us(), (21_161, 640));
    }

    #[test]
    fn nominal_crystal_ratio_is_reduced() {
        // The RZ/A1L's nominal 13.33 MHz crystal: 66,666,665 / 2,000,000 reduces by 5.
        let c = ClockConfig::new(13_333_333);
        assert_eq!(c.p0_ticks_per_us(), (13_333_333, 400_000));
        assert_eq!(c.p1_hz(), 66_666_665);
        assert_eq!(c.p0_hz(), 33_333_332);
    }

    #[test]
    fn ratio_is_always_fully_reduced_and_matches_p0() {
        for extal in [10_000_000, 12_000_000, 13_000_000, 13_225_625, 13_333_333] {
            let c = ClockConfig::new(extal);
            let (num, den) = c.p0_ticks_per_us();
            assert_eq!(gcd(num as u64, den as u64), 1, "extal {extal}: not reduced");
            // num/den ticks per µs × 1e6 µs = P0φ, truncated to whole Hz.
            assert_eq!(num as u64 * 1_000_000 / den as u64, c.p0_hz() as u64, "extal {extal}");
        }
    }

    #[test]
    fn store_is_unset_until_set() {
        // A local store: the global one is shared with other tests.
        let store = Store::new();
        assert_eq!(store.get(), None);
        store.set(TEST_CLOCKS);
        assert_eq!(store.get(), Some(TEST_CLOCKS));
    }

    #[test]
    fn init_then_get_round_trips() {
        init(TEST_CLOCKS);
        assert_eq!(get(), TEST_CLOCKS);
        assert_eq!(try_get(), Some(TEST_CLOCKS));
    }
}
```

Add to `crates/rza1l-hal/src/lib.rs`, directly above `pub mod dmac;`:

```rust
/// The clock tree, derived from the board's crystal (set once at boot by the BSP).
pub mod clocks;
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib clocks`
Expected: compiles, and all 5 `clocks::tests` FAIL, panicking with `not yet implemented`. (`TEST_CLOCKS` calls a `todo!()` const fn, so if the compiler rejects it at compile time instead, that also counts as the expected failure.)

- [ ] **Step 3: Implement the bodies**

```rust
    pub const fn new(extal_hz: u32) -> Self {
        assert!(extal_hz != 0, "ClockConfig: extal_hz must be non-zero");
        Self { extal_hz }
    }

    pub const fn p1_hz(&self) -> u32 {
        self.extal_hz * 5
    }

    pub const fn p0_hz(&self) -> u32 {
        (self.extal_hz as u64 * 5 / 2) as u32
    }

    pub const fn p0_ticks_per_us(&self) -> (u32, u32) {
        let num = self.extal_hz as u64 * 5;
        let den = 2_000_000u64;
        let g = gcd(num, den);
        ((num / g) as u32, (den / g) as u32)
    }
```

```rust
    fn set(&self, config: ClockConfig) {
        self.0.store(config.extal_hz, Ordering::Relaxed);
    }

    fn get(&self) -> Option<ClockConfig> {
        match self.0.load(Ordering::Relaxed) {
            0 => None,
            hz => Some(ClockConfig { extal_hz: hz }),
        }
    }
```

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib clocks`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/rza1l-hal/src/clocks.rs crates/rza1l-hal/src/lib.rs
git commit -m "feat(rza1l-hal): derive the clock tree from a board-supplied crystal"
```

---

### Task 2: The BSP owns the crystal; MTU2 constants derive from it

**Files:**
- Modify: `crates/deluge-bsp/src/system.rs` (add `CLOCKS` near `STB_CONFIG`; first line of `init_clocks`; its doc list)
- Modify: `crates/deluge-bsp/src/midi_gate.rs:157-161` (`TICK_HZ`) and the stale doc at line 20
- Modify: `crates/deluge-bsp/src/encoder.rs:159` (`POLL_TICKS`)
- Modify: `crates/rza1l-hal/src/mtu2.rs` (remove `MTU2_P0_HZ` at 113-117; module doc line 4; test at 433-438)

**Interfaces:**
- Consumes: `rza1l_hal::clocks::{ClockConfig, init}` (Task 1)
- Produces: `pub const deluge_bsp::system::CLOCKS: rza1l_hal::clocks::ClockConfig`. `init_clocks()` now calls `rza1l_hal::clocks::init(CLOCKS)` first.

- [ ] **Step 1: Write the failing BSP test.** Add to the end of `crates/deluge-bsp/src/system.rs`:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::CLOCKS;

    #[test]
    fn deluge_crystal_is_13_2256_mhz() {
        assert_eq!(CLOCKS.extal_hz, 13_225_625);
        assert_eq!(CLOCKS.p0_hz(), 33_064_062);
        assert_eq!(CLOCKS.p1_hz(), 66_128_125);
    }
}
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib system`
Expected: FAIL to compile, `cannot find value CLOCKS`.

- [ ] **Step 3: Add `CLOCKS` and register it at boot.** In `crates/deluge-bsp/src/system.rs`, add next to the other `use rza1l_hal::…` lines:

```rust
use rza1l_hal::clocks::ClockConfig;
```

Add directly above `pub const STB_CONFIG`:

```rust
/// The Deluge's EXTAL crystal: 13.2256 MHz (13,225,625 Hz), not the RZ/A1L's 13.33 MHz maximum.
/// Every peripheral clock derives from it (P0φ = 33,064,062.5 Hz, P1φ = 66,128,125 Hz).
pub const CLOCKS: ClockConfig = ClockConfig::new(13_225_625);
```

Make the first statement inside `init_clocks`'s `unsafe { … }` block:

```rust
        rza1l_hal::clocks::init(CLOCKS);
```

In `init_clocks`'s doc comment, insert above the `1.` step of the numbered list:

```rust
/// 0. Clock-tree registration ([`rza1l_hal::clocks::init`] with [`CLOCKS`]), so every
///    clock-dependent driver below sees the Deluge's crystal.
```

- [ ] **Step 4: Move the MTU2 tick constants onto `CLOCKS`, with compile-time pins.** In `crates/deluge-bsp/src/midi_gate.rs`, replace the `TICK_HZ` doc comment and definition (lines 157-161) with:

```rust
/// Tick frequency of the one-shot timer (Hz): P0φ / 64 = 516,625 Hz.
///
/// Use this to convert a duration to ticks:
/// `ticks = duration_us * TICK_HZ / 1_000_000`
pub const TICK_HZ: u32 = crate::system::CLOCKS.p0_hz() / PRESCALER as u32;
const _: () = assert!(TICK_HZ == 516_625, "MIDI gate tick rate changed");
```

In the same file, line 20, replace the stale doc line
`` //! `MTU2_P0_HZ` (33.333 MHz) / 64 = **520 833 Hz** → 1 tick ≈ **1.92 µs**. `` with:

```rust
//! P0φ (33.064 MHz) / 64 = **516 625 Hz** → 1 tick ≈ **1.94 µs**.
```

In `crates/deluge-bsp/src/encoder.rs`, replace line 159 with:

```rust
const POLL_TICKS: u16 = (crate::system::CLOCKS.p0_hz() / POLL_PRESCALER as u32 / 1000) as u16;
#[cfg(target_os = "none")]
const _: () = assert!(POLL_TICKS == 516, "encoder poll period changed");
```

(Line 158 already has `#[cfg(target_os = "none")]` for `POLL_TICKS`. Keep it.)

- [ ] **Step 5: Remove `MTU2_P0_HZ` from the HAL.** In `crates/rza1l-hal/src/mtu2.rs`:
  - Delete lines 113-117 (the `/// P0 clock frequency driving MTU2 (Hz).` doc block and `pub const MTU2_P0_HZ: u32 = 33_064_062;`).
  - Line 4: replace `(33.064 MHz on the Deluge; see [`MTU2_P0_HZ`]).` with `(see [`crate::clocks::ClockConfig::p0_hz`]).`
  - Replace the test `p0_clock_matches_deluge_extal` (lines 433-438) with:

```rust
    #[test]
    fn tgia_irq_table_covers_all_five_channels() {
        assert_eq!(TGIA_IRQ.len(), 5);
    }
```

- [ ] **Step 6: Audit boot order.** Every binary must call `init_clocks` before any OSTM delay or RSPI init. Run:

```bash
cd ~/GitHub/deluge-sdk
for f in firmwares/*/src/main.rs hw-tests/*/src/main.rs crates/deluge-sdk/src/lib.rs; do
  awk -v f="$f" '/init_clocks\(\)/{seen=1} /ostm::delay|rspi::init|cv_gate::init|audio::init|oled::init/ && !seen {print f": "FNR": "$0}' "$f"
done
```

Expected: no output. Any line printed is a driver used before the clock tree exists: move the call below `init_clocks()` and note it in the commit message.

- [ ] **Step 7: Run the tests and the device build**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib && cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib && cargo build-fw -p deluge-controller`
Expected: PASS. The device build also checks both `const _: () = assert!` pins.

- [ ] **Step 8: Commit**

```bash
git add crates/deluge-bsp/src/system.rs crates/deluge-bsp/src/midi_gate.rs crates/deluge-bsp/src/encoder.rs crates/rza1l-hal/src/mtu2.rs
git commit -m "refactor(bsp): own the Deluge crystal; derive MTU2 tick rates from it"
```

---

### Task 3: OSTM delays and RSPI baud read the clock tree

**Files:**
- Modify: `crates/rza1l-hal/src/ostm.rs` (module doc lines 1-21; remove `OSTM_HZ` at 23-27; `delay_ms` 158-166; `Delay::delay_ns` 190-197; tests 334-362)
- Modify: `crates/rza1l-hal/src/rspi.rs` (remove `P1_HZ` at 115-119; `init` line 181; tests `rspi_spbr_10mhz` and `init_programs_master_mode_config`)

**Interfaces:**
- Consumes: `crate::clocks::{get, TEST_CLOCKS}` (Task 1)
- Produces: `fn ostm::ms_to_ticks(p0_hz: u32, ms: u32) -> u32`, `fn ostm::ns_to_ticks(p0_hz: u32, ns: u32) -> u32`, `fn rspi::spbr_for(p1_hz: u32, bit_rate: u32) -> u8` (all private, pure)

- [ ] **Step 1: Write the failing tests.** In `crates/rza1l-hal/src/ostm.rs`, replace the whole `mod tests` (lines 334-362) with:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::{OSTM0_BASE, OSTM1_BASE, base, ms_to_ticks, ns_to_ticks};
    use crate::clocks::TEST_CLOCKS;

    #[test]
    fn ostm_base_addresses() {
        // RZ/A1L HW Manual: OSTM0 @ 0xFCFEC000, OSTM1 @ 0xFCFEC400
        assert_eq!(OSTM0_BASE, 0xFCFE_C000);
        assert_eq!(OSTM1_BASE, 0xFCFE_C400);
    }

    #[test]
    fn base_selects_correct_channel() {
        assert_eq!(base(0), OSTM0_BASE);
        assert_eq!(base(1), OSTM1_BASE);
    }

    /// 1 ms at the Deluge's P0φ = 33_064 ticks, as with the old `OSTM_HZ / 1000`.
    #[test]
    fn one_ms_ticks() {
        assert_eq!(ms_to_ticks(TEST_CLOCKS.p0_hz(), 1), 33_064);
        assert_eq!(ms_to_ticks(TEST_CLOCKS.p0_hz(), 100), 3_306_400);
    }

    /// `delay_ns` rounds up, so a short delay is never shorter than asked.
    #[test]
    fn ns_ticks_round_up() {
        let p0 = TEST_CLOCKS.p0_hz();
        assert_eq!(ns_to_ticks(p0, 0), 0);
        assert_eq!(ns_to_ticks(p0, 1), 1);
        // 1 µs = 33.064 ticks → 34.
        assert_eq!(ns_to_ticks(p0, 1_000), 34);
        // u32::MAX ns ≈ 4.29 s still fits u32.
        assert_eq!(ns_to_ticks(p0, u32::MAX), 142_009_065);
    }
}
```

In `crates/rza1l-hal/src/rspi.rs`, replace the test `rspi_spbr_10mhz` with:

```rust
    #[test]
    fn spbr_matches_the_old_hardcoded_p1() {
        let p1 = crate::clocks::TEST_CLOCKS.p1_hz();
        // ceil(66_128_125 / 20_000_000) - 1 = 3 → 8.27 MHz actual.
        assert_eq!(spbr_for(p1, 10_000_000), 3);
        // The 4 MHz OLED rate: ceil(8.27) - 1 = 8 → 3.67 MHz actual.
        assert_eq!(spbr_for(p1, 4_000_000), 8);
    }
```

and make the first line of `init_programs_master_mode_config`'s body:

```rust
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
```

- [ ] **Step 2: Run them and confirm they fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib ostm rspi`
Expected: FAIL to compile, `cannot find function ms_to_ticks` / `ns_to_ticks` / `spbr_for`.

- [ ] **Step 3: Implement OSTM.** In `crates/rza1l-hal/src/ostm.rs`:

Delete lines 23-27 (the `/// OSTM P0 clock frequency (Hz).` doc and `pub const OSTM_HZ: u32 = 33_064_062;`).

Replace module-doc lines 3-4
```
//! The RZ/A1L has two OSTM channels, each a 32-bit timer clocked by P0
//! (33.33 MHz = CPU / 12).
```
with
```
//! The RZ/A1L has two OSTM channels, each a 32-bit timer clocked by P0φ
//! (see [`crate::clocks`]).
```

In the module-doc usage example (lines 11-21), replace the line `//! # use rza1l_hal::ostm::OSTM_HZ;` with `//! # use rza1l_hal::clocks;` and replace `OSTM_HZ` in the `while` line with `clocks::get().p0_hz()`.

Add above `pub unsafe fn delay_ticks`:

```rust
/// OSTM ticks in `ms` milliseconds at `p0_hz`.
fn ms_to_ticks(p0_hz: u32, ms: u32) -> u32 {
    ms * (p0_hz / 1000)
}

/// OSTM ticks in `ns` nanoseconds at `p0_hz`, rounded up so a delay is never short.
/// Fits u32 for any `ns`: u32::MAX ns ≈ 4.3 s ≈ 143 M ticks.
fn ns_to_ticks(p0_hz: u32, ns: u32) -> u32 {
    (ns as u64 * p0_hz as u64).div_ceil(1_000_000_000) as u32
}
```

Replace the body of `delay_ms` with:

```rust
    unsafe { delay_ticks(ms_to_ticks(crate::clocks::get().p0_hz(), ms)) }
```

and add `/// # Panics` / `/// If [`crate::clocks::init`] has not been called.` to its doc comment. Replace the body of `DelayNs::delay_ns` with:

```rust
        let ticks = ns_to_ticks(crate::clocks::get().p0_hz(), ns);
        unsafe { delay_ticks(ticks) };
```

- [ ] **Step 4: Implement RSPI.** In `crates/rza1l-hal/src/rspi.rs`, replace the `// ── P1 clock frequency ──` block (lines 115-119: the comment banner, its doc comment, and `const P1_HZ`) with:

```rust
/// SPBR for `bit_rate` at `p1_hz`: `ceil(P1 / (2 × bit_rate)) - 1`, so the actual rate
/// `P1 / (2 × (SPBR + 1))` never exceeds the request.
fn spbr_for(p1_hz: u32, bit_rate: u32) -> u8 {
    p1_hz.div_ceil(bit_rate * 2).saturating_sub(1) as u8
}
```

Replace line 181 (`let spbr = P1_HZ.div_ceil(bit_rate * 2).saturating_sub(1) as u8;`) with:

```rust
        let spbr = spbr_for(crate::clocks::get().p1_hz(), bit_rate);
```

and add `/// # Panics` / `/// If [`crate::clocks::init`] has not been called.` to `init`'s doc comment.

- [ ] **Step 5: Run the HAL and BSP tests and the device build**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib && cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib && cargo build-fw -p deluge-controller`
Expected: PASS. If a BSP host test panics with `rza1l_hal::clocks::init was not called`, add `rza1l_hal::clocks::init(crate::system::CLOCKS);` as the first line of that test and re-run.

- [ ] **Step 6: Confirm no hardcoded clock is left in the HAL**

Run: `grep -rnE '33_064_062|66_128_125|13_225_625|21_161' crates/rza1l-hal/src | grep -v 'clocks.rs'`
Expected: only lines inside `#[cfg(test)]` modules, plus doc comments that cite a value as an example. No non-test code.

- [ ] **Step 7: Commit**

```bash
git add crates/rza1l-hal/src/ostm.rs crates/rza1l-hal/src/rspi.rs crates/deluge-bsp/src
git commit -m "refactor(rza1l-hal): OSTM delays and RSPI baud read the registered clock tree"
```

---

### Task 4: The time driver's tick ratio comes from the clock tree

**Files:**
- Modify: `crates/rza1l-hal/src/time_math.rs` (replace lines 8-46: the constants and the two conversion fns; replace the tests from `max_arm_bounds_a_mis_armed_alarm_to_about_100ms` to the end of the module)
- Modify: `crates/rza1l-hal/src/time_driver.rs` (import line 39; `try_set_alarm`; `now`; `init`; module doc lines 12-14)

**Interfaces:**
- Consumes: `crate::clocks::{ClockConfig, get, TEST_CLOCKS}` (Task 1)
- Produces (all `pub(crate)` in `time_math`):
  - `struct TickRatio`
  - `const fn TickRatio::from_clocks(c: &ClockConfig) -> TickRatio`
  - `const fn TickRatio::is_set(&self) -> bool`
  - `const fn TickRatio::ticks_to_us(&self, ticks: u64) -> u64`
  - `const fn TickRatio::us_to_ticks(&self, us: u64) -> u64`
  - `const fn TickRatio::max_arm_ticks(&self) -> u32`
  - `struct AtomicTickRatio` with `const fn unset()`, `fn store(&self, r: TickRatio)` and `fn load(&self) -> TickRatio`

- [ ] **Step 1: Write the failing tests.** In `crates/rza1l-hal/src/time_math.rs`, keep the two `extend_*` tests. Replace everything from `#[test] fn max_arm_bounds_a_mis_armed_alarm_to_about_100ms` to the end of `mod tests` with:

```rust
    use crate::clocks::{ClockConfig, TEST_CLOCKS};

    const DELUGE: TickRatio = TickRatio::from_clocks(&TEST_CLOCKS);
    const CRYSTALS: [u32; 4] = [10_000_000, 12_000_000, 13_225_625, 13_333_333];

    /// Exact `floor(ticks × den / num)` and saturating `ceil(us × num / den)`, in u128.
    fn ref_ticks_to_us(r: TickRatio, ticks: u64) -> u64 {
        (ticks as u128 * r.den as u128 / r.num as u128) as u64
    }
    fn ref_us_to_ticks(r: TickRatio, us: u64) -> u64 {
        (us as u128 * r.num as u128)
            .div_ceil(r.den as u128)
            .min(u64::MAX as u128) as u64
    }

    #[test]
    fn deluge_ratio_is_21161_over_640() {
        assert_eq!((DELUGE.num, DELUGE.den), (21_161, 640));
        assert!(DELUGE.is_set());
    }

    #[test]
    fn max_arm_bounds_a_mis_armed_alarm_to_about_100ms() {
        assert_eq!(DELUGE.max_arm_ticks(), 3_300_000);
        // A full-range arm is u32::MAX ticks = 129.898353 s; the cap must be far below that.
        let full_range_us = DELUGE.ticks_to_us(u32::MAX as u64);
        assert!((129_898_000..=129_899_000).contains(&full_range_us), "got {full_range_us}");
        let capped_us = DELUGE.ticks_to_us(DELUGE.max_arm_ticks() as u64);
        assert!((99_000..=101_000).contains(&capped_us), "got {capped_us}");
        assert!(DELUGE.max_arm_ticks() < u32::MAX / 1000);
    }

    #[test]
    fn ticks_to_us_uses_the_rational_not_33() {
        let us = DELUGE.ticks_to_us(33_064_062);
        assert!((999_999..=1_000_000).contains(&us), "got {us}");
        // A naive /33 would give 1_001_941 µs, a 0.19% error.
        assert!(us < 1_001_000);
    }

    #[test]
    fn us_to_ticks_rounds_up_so_alarms_never_fire_early() {
        assert!(DELUGE.us_to_ticks(1) >= 1);
        assert_eq!(DELUGE.us_to_ticks(640), 21_161);
        assert!(DELUGE.us_to_ticks(641) > 21_161);
        assert_eq!(DELUGE.us_to_ticks(0), 0);
    }

    #[test]
    fn conversions_round_trip_within_one_us() {
        for &us in &[1u64, 1000, 1_000_000, 5_000_000] {
            let back = DELUGE.ticks_to_us(DELUGE.us_to_ticks(us));
            assert!(back >= us && back <= us + 1, "us={us} back={back}");
        }
    }

    #[test]
    fn ticks_to_us_matches_u128_reference_across_crystals() {
        for extal in CRYSTALS {
            let r = TickRatio::from_clocks(&ClockConfig::new(extal));
            for ticks in [0, 1, r.num - 1, r.num, r.num + 1, 1 << 32, 1 << 50, 1 << 62, u64::MAX] {
                assert_eq!(r.ticks_to_us(ticks), ref_ticks_to_us(r, ticks), "extal {extal} ticks {ticks}");
            }
        }
    }

    #[test]
    fn us_to_ticks_matches_u128_reference_across_crystals() {
        for extal in CRYSTALS {
            let r = TickRatio::from_clocks(&ClockConfig::new(extal));
            for us in [0, 1, r.den - 1, r.den, r.den + 1, 1 << 32, 1 << 50, u64::MAX] {
                assert_eq!(r.us_to_ticks(us), ref_us_to_ticks(r, us), "extal {extal} us {us}");
            }
        }
    }

    #[test]
    fn ticks_to_us_survives_u64_max() {
        // The old single-multiply formula overflowed here; the split one must not panic.
        let _ = TickRatio::from_clocks(&ClockConfig::new(13_333_333)).ticks_to_us(u64::MAX);
        let _ = DELUGE.ticks_to_us(u64::MAX);
    }

    #[test]
    fn unset_ratio_is_not_set() {
        assert!(!AtomicTickRatio::unset().load().is_set());
    }

    #[test]
    fn unset_ratio_reads_zero_time() {
        // `Instant::now()` before `time_driver::init` reads 0 instead of dividing by zero.
        assert_eq!(AtomicTickRatio::unset().load().ticks_to_us(123_456), 0);
    }

    #[test]
    fn atomic_ratio_round_trips() {
        let a = AtomicTickRatio::unset();
        a.store(DELUGE);
        assert_eq!(a.load(), DELUGE);
    }
```

- [ ] **Step 2: Run them and confirm they fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib time_math`
Expected: FAIL to compile, `cannot find type TickRatio`.

- [ ] **Step 3: Implement `TickRatio`.** In `crates/rza1l-hal/src/time_math.rs`, replace lines 8-46 (from `/// OSTM0 ticks per Embassy tick (1 µs), as an exact rational.` through the `MAX_ARM_OSTM_TICKS` definition) with:

```rust
use core::sync::atomic::{AtomicU32, Ordering};

use crate::clocks::ClockConfig;

/// OSTM0 ticks per Embassy tick (1 µs), as an exact reduced rational `num / den`.
///
/// Taken from [`ClockConfig::p0_ticks_per_us`]: 21161/640 on the Deluge (P0φ = 33,064,062.5 Hz),
/// which avoids the ~0.2% drift of truncating to 33 ticks/µs. `num` < 2^27 and `den` < 2^21.
/// The all-zero value means "not set yet" (before `time_driver::init`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TickRatio {
    num: u64,
    den: u64,
}

impl TickRatio {
    pub(crate) const fn from_clocks(c: &ClockConfig) -> Self {
        let (num, den) = c.p0_ticks_per_us();
        Self { num: num as u64, den: den as u64 }
    }

    pub(crate) const fn is_set(&self) -> bool {
        self.num != 0
    }

    /// Raw OSTM0 ticks to Embassy µs, `floor(ticks × den / num)`, without overflow for any
    /// `ticks`: with `ticks = q·num + r`, `q·den` never exceeds the result, and
    /// `r·den < num·den < 2^48`. Reads 0 while unset.
    #[inline]
    pub(crate) const fn ticks_to_us(&self, ticks: u64) -> u64 {
        if self.num == 0 {
            return 0;
        }
        let q = ticks / self.num;
        let r = ticks % self.num;
        q * self.den + r * self.den / self.num
    }

    /// Embassy µs to OSTM0 ticks, rounded **up** so a short alarm never fires early:
    /// `ceil(us × num / den)`, saturating at `u64::MAX`. Callers clamp it to the 32-bit counter.
    #[inline]
    pub(crate) const fn us_to_ticks(&self, us: u64) -> u64 {
        if self.num == 0 {
            return 0;
        }
        let q = us / self.den;
        let r = us % self.den;
        q.saturating_mul(self.num)
            .saturating_add((r * self.num).div_ceil(self.den))
    }

    /// Longest OSTM1 alarm this driver will program at once (~100 ms), rather than the counter's
    /// full u32 range (~129.9 s).
    ///
    /// Capping costs no accuracy: a farther deadline is re-armed when the intermediate alarm fires
    /// (`ostm1_alarm_isr` re-reads the queue every time), at the cost of at most ~10 timer
    /// interrupts per second on an idle system. It bounds the damage of any error in the
    /// `at - now` computation: a miscalculated alarm self-heals within 100 ms.
    pub(crate) const fn max_arm_ticks(&self) -> u32 {
        (self.num / self.den) as u32 * 100_000
    }
}

/// A [`TickRatio`] the time driver sets once at init and reads on every `now()`.
pub(crate) struct AtomicTickRatio {
    num: AtomicU32,
    den: AtomicU32,
}

impl AtomicTickRatio {
    pub(crate) const fn unset() -> Self {
        Self { num: AtomicU32::new(0), den: AtomicU32::new(0) }
    }

    pub(crate) fn store(&self, r: TickRatio) {
        // `den` first: a reader that sees a non-zero `num` then sees its `den` (single core;
        // set once, before interrupts are enabled).
        self.den.store(r.den as u32, Ordering::Relaxed);
        self.num.store(r.num as u32, Ordering::Release);
    }

    #[inline]
    pub(crate) fn load(&self) -> TickRatio {
        let num = self.num.load(Ordering::Acquire) as u64;
        let den = self.den.load(Ordering::Relaxed) as u64;
        TickRatio { num, den }
    }
}
```


- [ ] **Step 4: Wire it into the time driver.** In `crates/rza1l-hal/src/time_driver.rs`:

Replace line 39 with:

```rust
use crate::time_math::{AtomicTickRatio, TickRatio, extend};
```

Add after the `LAST_RAW` static:

```rust
/// OSTM0 ticks per µs, from the registered clock tree. Set by [`init`].
static RATIO: AtomicTickRatio = AtomicTickRatio::unset();
```

In `try_set_alarm`, replace the clamp lines (`// Clamp to MAX_ARM_OSTM_TICKS…` through `let delta_ostm = …;`) with:

```rust
        let ratio = RATIO.load();
        assert!(
            ratio.is_set(),
            "embassy-time alarm armed before rza1l_hal::time_driver::init"
        );
        // Clamp to max_arm_ticks; a farther alarm is re-armed when that intermediate ISR
        // fires, so capping only costs wakeups, never accuracy.
        let delta_ostm = ratio
            .us_to_ticks(delta_us)
            .min(ratio.max_arm_ticks() as u64) as u32;
```

Replace `now`'s body (the comment and `ostm_ticks_to_us(...)`) with:

```rust
        // Overflow-free for any uptime; reads 0 before `init` (see `TickRatio::ticks_to_us`).
        RATIO.load().ticks_to_us(Self::raw_ostm_ticks())
```

Make the first statement in `init`'s `unsafe { … }` block:

```rust
        RATIO.store(TickRatio::from_clocks(&crate::clocks::get()));
```

and add to `init`'s `# Safety` paragraph: `` [`crate::clocks::init`] must have been called. ``

In the module doc, replace lines 12-14 (`OSTM0 runs at P0φ = 33,064,062.5 Hz on the Deluge. … from [`crate::time_math`].`) with:

```rust
//! OSTM0 runs at P0φ (see [`crate::clocks`]). The Embassy tick rate is 1 MHz (1 µs per tick);
//! `now()` converts OSTM0 ticks with the exact ticks-per-µs rational from
//! [`crate::time_math::TickRatio`] (21161/640 on the Deluge).
```

- [ ] **Step 5: Run the tests and the device build**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib && cargo build-fw -p deluge-controller`
Expected: PASS (`time_math` tests: 2 `extend_*` + 11 new).

- [ ] **Step 6: Commit**

```bash
git add crates/rza1l-hal/src/time_math.rs crates/rza1l-hal/src/time_driver.rs
git commit -m "refactor(rza1l-hal): compute the time driver's tick ratio from the clock tree"
```

---

### Task 5: GIC code-address check uses the chip's SDRAM window

**Files:**
- Modify: `crates/rza1l-hal/src/gic.rs:106-120` (`is_plausible_code_address` and its doc)
- Test: `crates/rza1l-hal/src/gic.rs` (`mod tests`, line ~533)

**Interfaces:**
- Consumes: nothing new. Produces: nothing new (private fn behaviour only).

- [ ] **Step 1: Write the failing test.** Append inside `gic.rs`'s `mod tests`:

```rust
    #[test]
    fn plausible_code_addresses_cover_sram_and_the_cs2_cs3_sdram_window() {
        // On-chip SRAM, ARM and Thumb entry points.
        assert!(is_plausible_code_address(0x2002_0000));
        assert!(is_plausible_code_address(0x2002_0001));
        assert!(!is_plausible_code_address(0x2030_0000));
        // CS2 and CS3 (SDRAM-capable areas), whatever size the board fits.
        assert!(is_plausible_code_address(0x0800_0000));
        assert!(is_plausible_code_address(0x0C00_0000));
        assert!(is_plausible_code_address(0x0FFF_FFFE));
        assert!(!is_plausible_code_address(0x1000_0000));
        // Null and CS0/CS1.
        assert!(!is_plausible_code_address(0x0000_0000));
        assert!(!is_plausible_code_address(0x07FF_FFFC));
    }
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib plausible_code`
Expected: FAIL at `is_plausible_code_address(0x0800_0000)` (today's range starts at the Deluge's 0x0C00_0000).

- [ ] **Step 3: Implement.** In `crates/rza1l-hal/src/gic.rs`, replace the doc sentence
`Code on this part lives in on-chip SRAM (`0x2000_0000`, 3 MB) or SDRAM (`0x0C00_0000`, 64 MB), and the range check is what actually catches corruption.`
with
`Code on this part lives in on-chip SRAM (`0x2000_0000`, 3 MB) or in SDRAM on CS2/CS3 (`0x0800_0000`–`0x0FFF_FFFF`, however much of it the board fits), and the range check is what actually catches corruption.`

and replace the `SDRAM` constant with:

```rust
    // CS2 + CS3, the SDRAM-capable external areas (64 MB each).
    const SDRAM: core::ops::Range<usize> = 0x0800_0000..0x1000_0000;
```

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rza1l-hal/src/gic.rs
git commit -m "fix(rza1l-hal): accept handlers anywhere in the CS2/CS3 SDRAM window, not just the Deluge's 64 MB"
```

---

### Task 6: Chip-named fault hook; enforce "no Deluge in the HAL"

**Files:**
- Modify: `crates/rza1l-hal/src/startup.rs` (line 658 symbol; doc mentions at lines 11, 367, 637-640)
- Modify: `crates/rza1l-hal/Cargo.toml:42` (feature comment)
- Modify: `tools/check-layering.sh` (new HAL rule)

**Interfaces:**
- Produces: the external symbol `rza1_cpu_fault_report(fault_pc: u32, fault_type: u32, sys_lr: u32, sys_sp: u32) -> !`, which replaces `deluge_cpu_fault_report`. This repo has no definers (checked across `~/GitHub` on 2026-10-14); the CHANGELOG entry in Task 8 tells external users.

- [ ] **Step 1: Add the failing layering rule.** In `tools/check-layering.sh`, insert before the final `echo "==> Layering check passed."`:

```bash
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
```

- [ ] **Step 2: Run it and confirm it fails on the fault hook only**

Run: `tools/check-layering.sh`
Expected: exit 1, listing exactly one hit: `crates/rza1l-hal/src/startup.rs:658:    ldr  r12, =deluge_cpu_fault_report`. (Tasks 2 and 4 already removed the two test names that also matched.) Any other hit is a Deluge fact Tasks 1-5 missed: fix it the same way before continuing.

- [ ] **Step 3: Rename the symbol.** In `crates/rza1l-hal/src/startup.rs`:
  - Line 658: `ldr  r12, =deluge_cpu_fault_report` → `ldr  r12, =rza1_cpu_fault_report`
  - Lines 11 and 637-640 (doc comments): replace `deluge_cpu_fault_report` with `rza1_cpu_fault_report`. Leave the sentence citing the Deluge C firmware's `fault_pattern.c` as prior art.
  - Line 367: `DelugeCpuFault:` → `fault type:`

In `crates/rza1l-hal/Cargo.toml` line 42, replace `` `deluge_cpu_fault_report` `` with `` `rza1_cpu_fault_report` ``.

- [ ] **Step 4: Prove the rule catches a regression, then revert**

```bash
echo 'const _DELUGE_PROBE: u8 = 0;' >> crates/rza1l-hal/src/lib.rs
tools/check-layering.sh; echo "exit=$?"
git checkout crates/rza1l-hal/src/lib.rs
tools/check-layering.sh
```

Expected: the first run prints the probe line and `exit=1`; the second prints `==> Layering check passed.`

- [ ] **Step 5: Check the device build with the feature on.** The asm references the symbol only under `app-fault-reporter`:

Run: `cargo build-fw -p deluge-controller --features rza1l-hal/app-fault-reporter 2>&1 | grep -E 'rza1_cpu_fault_report|error' | head -3`
Expected: a link error `undefined symbol: rza1_cpu_fault_report`. That is the documented "fails to link rather than faulting silently" behaviour, and it proves the asm uses the new name. Then run `cargo build-fw -p deluge-controller` (feature off). Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/check-layering.sh crates/rza1l-hal/src/startup.rs crates/rza1l-hal/Cargo.toml
git commit -m "refactor(rza1l-hal): name the fault hook rza1_cpu_fault_report; forbid Deluge names in the HAL"
```

---

### Task 7: The facade stops re-exporting the HAL; examples are facade-only

**Files:**
- Modify: `crates/deluge-sdk/src/lib.rs:188-196` (re-export block)
- Modify: `tools/check-layering.sh` (`WREN_DIRS` → facade-only dir list)

**Interfaces:**
- Produces: `deluge::rza1l_hal` no longer exists. `deluge::deluge_bsp` and `deluge::deluge_alloc` stay; whether they stay for good is a Phase 4 decision.

- [ ] **Step 1: Extend the layering rule (failing first).** In `tools/check-layering.sh`, replace `WREN_DIRS=(wren)` with:

```bash
# Consumers that must reach the platform only through the `deluge` facade.
FACADE_ONLY_DIRS=(wren examples/baremetal)
```

replace both `"${WREN_DIRS[@]}"` with `"${FACADE_ONLY_DIRS[@]}"`, and change the error line to:

```bash
  echo "error: facade-only code (wren/, examples/baremetal/) reaches past \`deluge\` into deluge-bsp/rza1l-hal" >&2
```

Add a probe, run, and confirm it fails:

```bash
echo 'fn _probe() { let _ = deluge::rza1l_hal::UNCACHED_MIRROR_OFFSET; }' >> examples/baremetal/blinky/src/main.rs
tools/check-layering.sh; echo "exit=$?"
```

Expected: the probe line printed and `exit=1`. Keep the probe in for Step 3.

- [ ] **Step 2: Remove the re-export.** In `crates/deluge-sdk/src/lib.rs`, replace lines 188-196 with:

```rust
// Re-export the board layers so apps can reach functionality the capability API does not cover
// through the single `deluge` dependency. The allocator is a device-only escape hatch (no host
// equivalent); an app reaching for it won't run in the desktop simulator. The chip HAL is
// deliberately not re-exported: the board crate is the lowest layer an app should see.
#[cfg(target_os = "none")]
pub use deluge_alloc;
pub use deluge_bsp;
```

- [ ] **Step 3: Confirm the probe no longer compiles, then remove it**

Run: `cargo build-fw -p blinky 2>&1 | grep -m1 'rza1l_hal'`
Expected: an error like `could not find rza1l_hal in deluge`. (If the package isn't named `blinky`, use the `name` from `examples/baremetal/blinky/Cargo.toml`.)

```bash
git checkout examples/baremetal/blinky/src/main.rs
tools/check-layering.sh
```

Expected: `==> Layering check passed.`

- [ ] **Step 4: Build everything that uses the facade**

Run: `tools/build-examples.sh && cargo build-wren && cargo test --target x86_64-unknown-linux-gnu -p deluge-sdk --features sim`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/deluge-sdk/src/lib.rs tools/check-layering.sh
git commit -m "refactor(deluge-sdk): stop re-exporting rza1l_hal; examples must use the facade only"
```

---

### Task 8: Full verification, CHANGELOG, hardware smoke test

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

- [ ] **Step 1: Add the CHANGELOG entries.** Under `## [Unreleased]`, add a `### Changed` section (or append to it if one exists):

```markdown
### Changed

- `rza1l-hal` is now board-agnostic. The board registers its crystal with
  `rza1l_hal::clocks::init(ClockConfig::new(extal_hz))` (the Deluge's is
  `deluge_bsp::system::CLOCKS`, registered by `init_clocks`). OSTM delays, the RSPI baud divider
  and the Embassy time driver derive their rates from it. **Breaking:** `ostm::OSTM_HZ` and
  `mtu2::MTU2_P0_HZ` are removed; use `rza1l_hal::clocks::get().p0_hz()` or
  `deluge_bsp::system::CLOCKS.p0_hz()`.
- **Breaking:** the `app-fault-reporter` hook is now `rza1_cpu_fault_report` (was
  `deluge_cpu_fault_report`).
- **Breaking:** `deluge` no longer re-exports `rza1l_hal`. Use `deluge::deluge_bsp`, or depend on
  `rza1l-hal` directly.
- The Embassy time driver's µs conversion no longer overflows on long uptimes for crystals other
  than the Deluge's.
```

- [ ] **Step 2: Run the full suite against the baseline**

```bash
tools/test.sh 2>&1 | tail -20
tools/build-examples.sh 2>&1 | tail -5
cargo build-fw -p deluge-controller 2>&1 | tail -3
cargo build-app-loader 2>&1 | tail -3
cargo build-wren 2>&1 | tail -3
cargo build-msc 2>&1 | tail -3
cargo build-uac 2>&1 | tail -3
```

Expected: no failures beyond the B1 baseline.

- [ ] **Step 3: Confirm the release image hasn't grown meaningfully.** Run this on the baseline commit and on this branch, then compare:

```bash
cargo build-fw-rel -p deluge-controller && arm-none-eabi-size target/armv7a-none-eabihf/release/deluge-controller
```

Expected: `text` within ~1 KB of the baseline (the ratio is now loaded from atomics instead of folded into constants). Note both numbers in the PR description.

- [ ] **Step 4: Hardware smoke test on a real Deluge** (flash the controller firmware the usual way, e.g. `cargo deluge run` or the J-Link flow). Check:
  - The OLED draws normally (RSPI baud at 4 MHz).
  - CV out tracks a MIDI note (RSPI at 10 MHz, MAX5136).
  - Encoders respond with no missed detents (MTU2 1 ms poll).
  - MIDI gate lengths sound unchanged (MTU2 `TICK_HZ`).
  - A 60 s `embassy_time::Timer` (or the sync LED's tempo) matches a stopwatch within ±0.1 s (time driver ratio).

Write down any deviation. Any failure blocks the merge.

- [ ] **Step 5: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): rza1l-hal clock tree, fault hook rename, facade re-export removal"
```

- [ ] **Step 6: Hand off.** Use superpowers:finishing-a-development-branch. The next step is writing the Phase 2 plan (split `deluge-bsp`) from the spec.
