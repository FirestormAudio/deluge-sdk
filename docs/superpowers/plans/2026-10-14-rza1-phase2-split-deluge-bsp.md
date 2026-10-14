# RZ/A1 Phase 2: Split `deluge-bsp` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Leave `deluge-bsp` holding only Deluge wiring and the drivers tied to it. Board-agnostic USB classes move to a new `usb-classes` crate, the MAX5136 DAC protocol to a `max5136` driver crate, the SCUX path recipes into `rza1l_hal::scux`, and the desktop-simulator backend into `deluge-sdk`, behind a hook the BSP exposes.

**Architecture:** Each move keeps public behaviour identical and keeps the code's existing tests next to it. Board facts (DMA channels, SSI config, the engine sample rate, the SD card) stay in the BSP and are passed into the moved code as arguments. Where firmware imports a module path that stays meaningful (`deluge_bsp::scux_dvu_path`, `deluge_bsp::usb::host::usb_host_supervisor`), the BSP keeps a thin wrapper so those imports don't change. Elsewhere, consumers switch to the new crate. `tools/check-layering.sh` gains spec rules 3 and 4 so nothing slides back.

**Tech Stack:** Rust nightly, `armv7a-none-eabihf` firmware, Embassy (`embassy-usb` 0.6, `embassy-usb-host` 0.1, `embassy-usb-driver` 0.2), `embedded-hal` 1.0, host + QEMU (`armv7-unknown-linux-gnueabihf`) unit tests, bash.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 2).

**Decisions made with the user on 2026-10-14:**
- USB classes go in **one** crate, `usb-classes`, with one feature per class, `publish = false` until the API settles.
- The simulator backend moves out of the BSP **now**, via a hook.
- **Extract the off-chip drivers.** Investigation showed SPI-NOR is already split correctly: `rza1l_hal::spibsc` holds the controller and the generic `FlashMap` type, and `deluge_bsp::flash` is only Deluge data (part size, board flash layout). So only the MAX5136 is extracted. Task 8 records this in the spec.

## Global Constraints

- **No behaviour change on the device.** These must stay the same:
  - CV DAC words (`0x31AB_CD00` for channel 0, `0xABCD`).
  - LIN init words `0x0542_0000` / `0x0540_0000`, with a 10 ms gap between them.
  - USB descriptors and VID/PIDs.
  - BOT/SCSI behaviour.
  - UAC host negotiating 44,100 Hz on the Deluge.
  - SCUX DMA channel assignments (`deluge_bsp::system::SCUX_*_DMA_CH`).
  - Simulator output.
- **Layering (spec §2):**
  - Rule 3: `usb/` crates never name `rza1l_hal`, `deluge_bsp` or any `deluge_*` crate, in code or manifests.
  - The same applies to `drivers/`.
  - Rule 4: `deluge-bsp` never depends on or names `deluge_sim_link` / `deluge-simulator`.
  - Rules 1–2 from Phase 1 keep passing.
- **New crates:**
  - `usb/usb-classes` (package `usb-classes`) and `drivers/max5136` (package `max5136`).
  - Both `publish = false`, `version.workspace`, `edition.workspace`, `license = "MIT OR Apache-2.0"`, and both workspace members.
- **Moves use `git mv`** for tracked source files, so history follows them.
- **Device-only gating stays as it is.** Any module that is `#[cfg(target_os = "none")]` in the BSP today keeps that gate in its new home (`embassy-usb` is a device-only dependency).
- **Worktree:** branch `refactor/rza1-split-bsp` in `~/GitHub/deluge-sdk`, or a *sibling* worktree; never a nested `.worktrees/` dir (`../flare` must resolve).
- **Commit style:** conventional commits scoped like the log, e.g. `refactor(bsp): …`, `feat(usb-classes): …`, `feat(max5136): …`, `chore(layering): …`.
- **Don't edit history docs:** leave the rest of `docs/superpowers/**` alone; the spec gets one Phase 2 note in Task 8. CHANGELOG goes under `## [Unreleased]`.
- **Test commands:**
  - HAL: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib`
  - BSP: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`
  - usb-classes: `cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes --features device-midi,host-midi,host-uac --lib`
  - max5136: `cargo test --target x86_64-unknown-linux-gnu -p max5136`
  - SDK sim: `cargo test --target x86_64-unknown-linux-gnu -p deluge-sdk --features sim`
  - Device builds: `tools/build-examples.sh`, `cargo build-fw -p deluge-controller`, `cargo build-fw -p demo-firmware`, `cargo build-app-loader`, `cargo build-msc`, `cargo build-uac`, `cargo build-wren`

## Review Focus

1. **The simulator panel isn't installed** (any plain host build or test). PIC commands and OLED frames must still drop silently, and `pic::read_byte` must still park forever, exactly as today. Pinned by `tx_without_a_panel_is_a_no_op`, the existing `host_read_byte_parks_and_does_not_resolve` and capture tests, and `capture_without_a_panel_needs_none` (Task 6).
2. **The hook is installed twice** (e.g. a second simulator brain). This must panic with a message naming `host_panel::install`, not silently keep the first. Pinned by `installing_twice_panics` (Task 6).
3. **A UAC device that only does 48 kHz, plugged into a host asking for 44.1 kHz.** It must still fail with `UnsupportedRate`. A host asking for 48 kHz must accept it without issuing a SET. Pinned by `register_at_the_requested_rate_skips_set_cur` and `register_rejects_a_device_that_will_not_switch` (Task 3).
4. **MAX5136 channel out of range** (`ch >= 4`). The old code silently built a wrong mask; the driver must panic. Pinned by `channel_four_is_rejected` (Task 1).
5. **A firmware that enables only some USB class features.** It must build without pulling in the others (e.g. msc-firmware without `device-audio`). Pinned by Task 4 Step 7, which builds each firmware with exactly the features it declares.

---

## Before you start: baseline

- [ ] **B1: Record the baseline** so pre-existing failures aren't blamed on this work.

```bash
cd ~/GitHub/deluge-sdk
git switch -c refactor/rza1-split-bsp
tools/test.sh > /tmp/p2-baseline-test.log 2>&1; echo "test.sh exit=$?"
tools/build-examples.sh > /tmp/p2-baseline-ex.log 2>&1; echo "examples exit=$?"
cargo build-fw -p deluge-controller > /dev/null 2>&1; echo "controller exit=$?"
cargo build-fw -p demo-firmware > /dev/null 2>&1; echo "demo exit=$?"
cargo build-app-loader > /dev/null 2>&1; echo "app-loader exit=$?"
cargo build-msc > /dev/null 2>&1; echo "msc exit=$?"
cargo build-uac > /dev/null 2>&1; echo "uac exit=$?"
cargo build-wren > /dev/null 2>&1; echo "wren exit=$?"
```

(Write each `cargo` alias out in full, as above. In zsh, `cargo $var` does not word-split a multi-word alias.) Write down anything that already fails. From here on, "Expected: PASS" means "no new failures compared with this baseline".

---

### Task 1: `max5136` driver crate; `cv_gate` uses it

**Files:**
- Create: `drivers/max5136/Cargo.toml`, `drivers/max5136/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`: add `"drivers/max5136"`; `[workspace.dependencies]`: add `max5136 = { path = "drivers/max5136" }`)
- Modify: `crates/deluge-bsp/Cargo.toml` (add `max5136 = { workspace = true }`)
- Modify: `crates/deluge-bsp/src/cv_gate.rs:75-85` (`dac_word`), `:128` and `:139` (the LIN words), `:47` (`LIN_DELAY_MS`)

**Interfaces:**
- Produces (crate `max5136`, `#![no_std]`):
  - `pub const NUM_CHANNELS: u8 = 4`
  - `pub const fn write_through(ch: u8, value: u16) -> [u8; 3]`
  - `pub const fn write_through_u32(ch: u8, value: u16) -> u32`
  - `pub const LINEARITY_ON: [u8; 3]`, `pub const LINEARITY_OFF: [u8; 3]`
  - `pub const LINEARITY_ON_U32: u32`, `pub const LINEARITY_OFF_U32: u32`
  - `pub const LINEARITY_SETTLE_MS: u32 = 10`
  - `pub struct Max5136<SPI>` with `new(spi)`, `init(&mut self, delay: &mut impl DelayNs) -> Result<(), SPI::Error>`, `set(&mut self, ch: u8, value: u16) -> Result<(), SPI::Error>`, and `release(self) -> SPI`

- [ ] **Step 1: Check crates.io for an existing driver**

Run: `cargo search max5136; cargo search max513`
Expected: no `embedded-hal` 1.0 driver for this part. If one exists with a compatible license and 1.0 support, stop and note it in the ledger as a ruling: depend on it instead and skip Steps 2–5's driver struct. Keep only the `*_u32` frame helpers in `cv_gate`.

- [ ] **Step 2: Write the crate with its tests and stubbed bodies.**

`drivers/max5136/Cargo.toml`:

```toml
[package]
name = "max5136"
version.workspace = true
edition.workspace = true
authors.workspace = true
license = "MIT OR Apache-2.0"
repository.workspace = true
homepage.workspace = true
publish = false
description = "Platform-agnostic embedded-hal driver for the Maxim MAX5136 quad 16-bit SPI DAC."
categories = ["embedded", "no-std", "hardware-support"]
keywords = ["max5136", "dac", "spi", "embedded-hal", "no-std"]

[dependencies]
embedded-hal = "1"
```

`drivers/max5136/src/lib.rs`:

```rust
//! Platform-agnostic driver for the Maxim **MAX5136** quad 16-bit SPI DAC.
//!
//! Every command is a 24-bit frame, sent MSB first: a command byte, then 16 data bits. The
//! frame helpers ([`write_through`], [`LINEARITY_ON`], …) are usable without the driver, for
//! boards that move frames over their own transport. The `*_u32` forms pad the frame into a
//! 32-bit word (`frame << 8`) for SPI controllers run in 32-bit mode; the part ignores the
//! trailing byte.
//!
//! Power-up needs a linearity calibration: send [`LINEARITY_ON`], wait at least
//! [`LINEARITY_SETTLE_MS`], then send [`LINEARITY_OFF`]. [`Max5136::init`] does exactly that.

#![no_std]

use embedded_hal::delay::DelayNs;
use embedded_hal::spi::SpiDevice;

/// DAC channels on the part.
pub const NUM_CHANNELS: u8 = 4;

/// Linearity calibration on: `0x05` command, data `0x4200`.
pub const LINEARITY_ON: [u8; 3] = [0x05, 0x42, 0x00];
/// Linearity calibration off: `0x05` command, data `0x4000`.
pub const LINEARITY_OFF: [u8; 3] = [0x05, 0x40, 0x00];
/// [`LINEARITY_ON`] as a padded 32-bit word.
pub const LINEARITY_ON_U32: u32 = 0x0542_0000;
/// [`LINEARITY_OFF`] as a padded 32-bit word.
pub const LINEARITY_OFF_U32: u32 = 0x0540_0000;
/// Minimum wait between [`LINEARITY_ON`] and [`LINEARITY_OFF`].
pub const LINEARITY_SETTLE_MS: u32 = 10;

/// Write-through frame: load `value` into channel `ch`'s input and DAC registers at once
/// (command `0x30 | 1 << ch`).
///
/// # Panics
/// If `ch >= NUM_CHANNELS` (at compile time, in a `const`).
pub const fn write_through(ch: u8, value: u16) -> [u8; 3] {
    todo!()
}

/// [`write_through`] as a padded 32-bit word.
pub const fn write_through_u32(ch: u8, value: u16) -> u32 {
    todo!()
}

/// A MAX5136 on an [`SpiDevice`] (which owns the chip select).
pub struct Max5136<SPI> {
    spi: SPI,
}

impl<SPI: SpiDevice<u8>> Max5136<SPI> {
    /// Wrap `spi`. Call [`Max5136::init`] before the first [`Max5136::set`].
    pub fn new(spi: SPI) -> Self {
        Self { spi }
    }

    /// Run the power-up linearity calibration.
    pub fn init(&mut self, delay: &mut impl DelayNs) -> Result<(), SPI::Error> {
        todo!()
    }

    /// Set channel `ch` to `value` (0 = zero scale, 65535 = full scale).
    ///
    /// # Panics
    /// If `ch >= NUM_CHANNELS`.
    pub fn set(&mut self, ch: u8, value: u16) -> Result<(), SPI::Error> {
        todo!()
    }

    /// Give the SPI device back.
    pub fn release(self) -> SPI {
        self.spi
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::convert::Infallible;
    use embedded_hal::spi::{ErrorType, Operation};
    use std::vec::Vec;

    /// Records every write, one entry per transaction (one chip-select assertion).
    #[derive(Default)]
    struct FakeSpi {
        frames: Vec<Vec<u8>>,
    }
    impl ErrorType for FakeSpi {
        type Error = Infallible;
    }
    impl SpiDevice<u8> for FakeSpi {
        fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), Infallible> {
            let mut frame = Vec::new();
            for op in ops.iter() {
                if let Operation::Write(bytes) = op {
                    frame.extend_from_slice(bytes);
                }
            }
            self.frames.push(frame);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeDelay {
        total_ns: u64,
    }
    impl DelayNs for FakeDelay {
        fn delay_ns(&mut self, ns: u32) {
            self.total_ns += ns as u64;
        }
    }

    #[test]
    fn write_through_frames_match_the_deluge_words() {
        // The words `deluge_bsp::cv_gate` sent before this crate existed.
        assert_eq!(write_through(0, 0xABCD), [0x31, 0xAB, 0xCD]);
        assert_eq!(write_through_u32(0, 0xABCD), 0x31AB_CD00);
        assert_eq!(write_through_u32(1, 0x1234), 0x3212_3400);
        assert_eq!(write_through_u32(3, 0), 0x3800_0000);
    }

    #[test]
    fn linearity_words_match_their_frames() {
        let as_u32 = |f: [u8; 3]| u32::from_be_bytes([f[0], f[1], f[2], 0]);
        assert_eq!(as_u32(LINEARITY_ON), LINEARITY_ON_U32);
        assert_eq!(as_u32(LINEARITY_OFF), LINEARITY_OFF_U32);
    }

    #[test]
    #[should_panic]
    fn channel_four_is_rejected() {
        let _ = write_through(4, 0);
    }

    #[test]
    fn init_sends_on_waits_then_off() {
        let mut dac = Max5136::new(FakeSpi::default());
        let mut delay = FakeDelay::default();
        dac.init(&mut delay).unwrap();
        let spi = dac.release();
        assert_eq!(spi.frames, [LINEARITY_ON.to_vec(), LINEARITY_OFF.to_vec()]);
        assert!(delay.total_ns >= LINEARITY_SETTLE_MS as u64 * 1_000_000);
    }

    #[test]
    fn set_sends_one_write_through_frame() {
        let mut dac = Max5136::new(FakeSpi::default());
        dac.set(2, 0x8000).unwrap();
        assert_eq!(dac.release().frames, [std::vec![0x34, 0x80, 0x00]]);
    }
}
```

Add `"drivers/max5136",` to the workspace `members` list in `Cargo.toml` (after `"crates/deluge-protocol",`), and add to `[workspace.dependencies]`:

```toml
# Off-chip part drivers, board-agnostic (embedded-hal 1.0).
max5136 = { path = "drivers/max5136" }
```

- [ ] **Step 3: Run the tests and confirm they fail**

Run: `cargo test --target x86_64-unknown-linux-gnu -p max5136`
Expected: tests FAIL with `not yet implemented`.

- [ ] **Step 4: Implement**

```rust
pub const fn write_through(ch: u8, value: u16) -> [u8; 3] {
    assert!(ch < NUM_CHANNELS, "max5136: channel must be 0-3");
    let [hi, lo] = value.to_be_bytes();
    [0x30 | (1 << ch), hi, lo]
}

pub const fn write_through_u32(ch: u8, value: u16) -> u32 {
    let [cmd, hi, lo] = write_through(ch, value);
    u32::from_be_bytes([cmd, hi, lo, 0])
}
```

```rust
    pub fn init(&mut self, delay: &mut impl DelayNs) -> Result<(), SPI::Error> {
        self.spi.write(&LINEARITY_ON)?;
        delay.delay_ms(LINEARITY_SETTLE_MS);
        self.spi.write(&LINEARITY_OFF)
    }

    pub fn set(&mut self, ch: u8, value: u16) -> Result<(), SPI::Error> {
        self.spi.write(&write_through(ch, value))
    }
```

- [ ] **Step 5: Run the tests and confirm they pass**

Run: `cargo test --target x86_64-unknown-linux-gnu -p max5136`
Expected: 5 passed.

- [ ] **Step 6: Use it in `cv_gate`.** In `crates/deluge-bsp/Cargo.toml` `[dependencies]` add `max5136 = { workspace = true }`. In `crates/deluge-bsp/src/cv_gate.rs`:
  - Replace the body of `fn dac_word` with `max5136::write_through_u32(ch, value)`. Keep the function and its doc so call sites don't change, and add a line to the doc: `/// The frame encoding lives in the board-agnostic \`max5136\` crate.`
  - Replace the literal `0x0542_0000` with `max5136::LINEARITY_ON_U32`, and the literal `0x0540_0000` with `max5136::LINEARITY_OFF_U32`.
  - Replace `const LIN_DELAY_MS: u32 = 10;` with `const LIN_DELAY_MS: u32 = max5136::LINEARITY_SETTLE_MS;`.
  - Rewrite the `## MAX5136 32-bit write frame` section of the module doc to point at the `max5136` crate for the encoding. Keep the SPI wiring and gate tables, which are Deluge facts.

- [ ] **Step 7: Run BSP tests and the device build**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib && cargo build-fw -p deluge-controller && cargo build-fw -p demo-firmware`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add drivers/max5136 Cargo.toml Cargo.lock crates/deluge-bsp/Cargo.toml crates/deluge-bsp/src/cv_gate.rs
git commit -m "feat(max5136): extract the MAX5136 DAC protocol into a board-agnostic driver"
```

---

### Task 2: `usb-classes` crate; MIDI packet queues and UMP sizing move first

**Files:**
- Create: `usb/usb-classes/Cargo.toml`, `usb/usb-classes/src/lib.rs`
- Move: `crates/deluge-bsp/src/usb/midi_packets.rs` → `usb/usb-classes/src/midi_packets.rs`
- Modify: `crates/deluge-bsp/src/midi_stream.rs:177-205` (`ump_word_count` and its test move out; re-export instead)
- Modify: `crates/deluge-bsp/src/usb/mod.rs` (drop `pub mod midi_packets;` and its doc bullet)
- Modify: `crates/deluge-bsp/src/usb/classes/midi.rs` (`crate::usb::midi_packets` → `usb_classes::midi_packets`; `crate::midi_stream::ump_word_count` → `usb_classes::midi_packets::ump_word_count`)
- Modify: `Cargo.toml` (members `"usb/usb-classes"`; `[workspace.dependencies]` `usb-classes = { path = "usb/usb-classes" }`), `crates/deluge-bsp/Cargo.toml`, `tools/test.sh`

**Interfaces:**
- Produces: crate `usb-classes` (`usb_classes`) with features `device-audio`, `device-midi`, `device-msc`, `host-midi` and `host-uac`. Under `device-midi` (all targets), it has `usb_classes::midi_packets::*` (everything `deluge_bsp::usb::midi_packets` had) plus `pub fn usb_classes::midi_packets::ump_word_count(first_word: u32) -> usize`.
- `deluge_bsp::midi_stream::ump_word_count` stays available, as a re-export.

- [ ] **Step 1: Create the crate skeleton.** `usb/usb-classes/Cargo.toml`. The dependencies mirror the ones `deluge-bsp` gives this code today: device-only `embassy-usb`/`embassy-time`/`embassy-futures`, and an optional `embassy-usb-host`.

```toml
[package]
name = "usb-classes"
version.workspace = true
edition.workspace = true
authors.workspace = true
license = "MIT OR Apache-2.0"
repository.workspace = true
homepage.workspace = true
publish = false
description = "Board-agnostic embassy-usb device classes (UAC2, USB-MIDI, MSC + Bulk-Only Transport) and embassy-usb-host class drivers (USB-MIDI, UAC)."
categories = ["embedded", "no-std", "hardware-support"]
keywords = ["usb", "embassy", "uac2", "midi", "msc"]

[package.metadata.docs.rs]
default-target = "armv7a-none-eabihf"

[lib]
# Unit tests run on the host / QEMU target; the test harness needs std.
test = false

[features]
default = []
## UAC2 audio device class.
device-audio = []
## USB-MIDI device class and the packet-mode queues behind it.
device-midi = []
## Mass Storage device class and the Bulk-Only Transport / SCSI engine.
device-msc = []
## USB-MIDI host class driver.
host-midi = ["dep:embassy-usb-host"]
## UAC host class driver (capture + playback).
host-uac = ["dep:embassy-usb-host"]

[dependencies]
critical-section = { workspace = true }
embassy-sync = { workspace = true }
embassy-usb-driver = { workspace = true }
embassy-usb-host = { workspace = true, optional = true }
heapless = "0.9"
log = { workspace = true }

[target.'cfg(target_os = "none")'.dependencies]
embassy-futures = { workspace = true }
embassy-time = { workspace = true, features = ["tick-hz-1_000_000"] }
embassy-usb = { workspace = true }

[target.'cfg(not(target_os = "none"))'.dev-dependencies]
critical-section = { workspace = true, features = ["std"] }
embassy-futures = { workspace = true }
embassy-time = { workspace = true, features = ["mock-driver", "generic-queue-8"] }
```

`usb/usb-classes/src/lib.rs`:

```rust
//! Board-agnostic USB classes on Embassy.
//!
//! Device classes are generic over any `embassy_usb::driver::Driver`, and host class drivers
//! over any `embassy_usb_driver::host::UsbHostAllocator`, so nothing here knows which chip or
//! board it runs on. Enable only the classes a firmware uses:
//!
//! | Feature        | Modules |
//! |----------------|---------|
//! | `device-audio` | [`audio`] (UAC2) |
//! | `device-midi`  | [`midi`] (USB-MIDI), [`midi_packets`] (packet-mode queues, UMP sizing) |
//! | `device-msc`   | [`msc`] (Mass Storage), [`bot`] (Bulk-Only Transport / SCSI) |
//! | `host-midi`    | [`host::midi`] |
//! | `host-uac`     | [`host::uac`] |
//!
//! Device classes build only for the bare-metal target (`target_os = "none"`), where
//! `embassy-usb` is a dependency. The packet queues and the host drivers also build, and are
//! tested, under QEMU.

#![cfg_attr(target_os = "none", no_std)]

#[cfg(feature = "device-midi")]
pub mod midi_packets;
```

Add `"usb/usb-classes",` to workspace `members` (after `"crates/deluge-protocol",`). Add to `[workspace.dependencies]`:

```toml
# Board-agnostic USB classes (publish = false until the API settles).
usb-classes = { path = "usb/usb-classes" }
```

- [ ] **Step 2: Move the UMP test first, so it fails.** Append inside `#[cfg(test)] mod tests` of `crates/deluge-bsp/src/usb/midi_packets.rs`. Copy the test body *verbatim* from `midi_stream.rs`'s `ump_word_counts_match_the_message_type_table` (lines ~196-205), renaming the call to `super::ump_word_count`. Then move the file:

```bash
mkdir -p usb/usb-classes/src
git mv crates/deluge-bsp/src/usb/midi_packets.rs usb/usb-classes/src/midi_packets.rs
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes --features device-midi --lib`
Expected: FAIL to compile, `cannot find function ump_word_count`. (Fix any other `crate::…` path the moved file uses: `grep -n 'crate::' usb/usb-classes/src/midi_packets.rs` should print nothing but `crate::midi_packets` self-references.)

- [ ] **Step 3: Move `ump_word_count`.** Cut `pub fn ump_word_count` and its doc comment from `crates/deluge-bsp/src/midi_stream.rs` and paste them into `usb/usb-classes/src/midi_packets.rs`, above its `#[cfg(test)]`. Delete `midi_stream.rs`'s now-moved test `ump_word_counts_match_the_message_type_table`. In `midi_stream.rs`, where the function was, add:

```rust
/// Words in the UMP message starting with `first_word`. Lives with the USB-MIDI packet code in
/// `usb-classes`; re-exported here because UMP sizing is also part of MIDI stream handling.
pub use usb_classes::midi_packets::ump_word_count;
```

In `crates/deluge-bsp/Cargo.toml` `[dependencies]`, add `usb-classes = { workspace = true, features = ["device-midi"] }`.

In `crates/deluge-bsp/src/usb/mod.rs`: delete `pub mod midi_packets;` and its two comment lines, and the `[`midi_packets`]` doc bullet. In `crates/deluge-bsp/src/usb/classes/midi.rs`, replace `crate::usb::midi_packets` with `usb_classes::midi_packets` and `crate::midi_stream::ump_word_count` with `usb_classes::midi_packets::ump_word_count`.

- [ ] **Step 4: Run all affected tests and the build**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes --features device-midi --lib && cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib && cargo build-fw -p deluge-controller`
Expected: PASS. The usb-classes count equals midi_packets' old test count + 1. The BSP count drops by midi_packets' tests + 1.

- [ ] **Step 5: Run the new crate in `tools/test.sh`.** In the QEMU bucket, after the `deluge-bsp` line, add:

```bash
cargo test --target "$QEMU" -p usb-classes --features device-midi,host-midi,host-uac --lib
```

(`host-*` modules don't exist until Task 3; the features already exist, so this line works now.)

Run: `tools/test.sh 2>&1 | tail -2`
Expected: `==> All tests passed.`

- [ ] **Step 6: Commit**

```bash
git add usb Cargo.toml Cargo.lock crates/deluge-bsp tools/test.sh
git commit -m "feat(usb-classes): new crate; USB-MIDI packet queues and UMP sizing move into it"
```

---

### Task 3: Host class drivers move; UAC takes the sample rate as an argument

**Files:**
- Move: `crates/deluge-bsp/src/usb/host/midi.rs` → `usb/usb-classes/src/host/midi.rs`
- Move: `crates/deluge-bsp/src/usb/host/mock.rs` → `usb/usb-classes/src/host/mock.rs`
- Move: `crates/deluge-bsp/src/usb/host/uac/` → `usb/usb-classes/src/host/uac/`
- Create: `usb/usb-classes/src/host/mod.rs`
- Move: `crates/deluge-bsp/src/usb/host/mod.rs` → `crates/deluge-bsp/src/usb/host.rs` (the registry + supervisor stay in the BSP)
- Modify: `crates/deluge-bsp/Cargo.toml` (`usb-host` feature), `hw-tests/uac-host-firmware/Cargo.toml`, `hw-tests/uac-host-firmware/src/tasks/loopback.rs:12`, `hw-tests/uac-host-firmware/src/main.rs:4`

**Interfaces:**
- Consumes: crate `usb-classes` (Task 2).
- Produces:
  - `usb_classes::host::midi::*`, the same items as `deluge_bsp::usb::host::midi`.
  - `usb_classes::host::uac::*`, the same items as `deluge_bsp::usb::host::uac`, with `shared` now `pub` (the BSP supervisor uses it) and one signature change: `Uac::try_register(alloc: &A, addr: u8, split: Option<SplitInfo>, cfg: &ConfigurationDescriptor<'_>, sample_rate_hz: u32) -> Result<Self, UacError>`.
  - `deluge_bsp::usb::host::{usb_host_supervisor, MAX_MIDI_DEVICES, …}` keep their paths.

- [ ] **Step 1: Move the files**

```bash
mkdir -p usb/usb-classes/src/host
git mv crates/deluge-bsp/src/usb/host/midi.rs usb/usb-classes/src/host/midi.rs
git mv crates/deluge-bsp/src/usb/host/mock.rs usb/usb-classes/src/host/mock.rs
git mv crates/deluge-bsp/src/usb/host/uac usb/usb-classes/src/host/uac
git mv crates/deluge-bsp/src/usb/host/mod.rs crates/deluge-bsp/src/usb/host.rs
```

Create `usb/usb-classes/src/host/mod.rs`:

```rust
//! Host-side class drivers, generic over [`UsbHostAllocator`], so they are unit-tested
//! against [`mock::MockAlloc`] under QEMU.
//!
//! [`UsbHostAllocator`]: embassy_usb_driver::host::UsbHostAllocator

#[cfg(feature = "host-midi")]
pub mod midi;
#[cfg(all(test, not(target_os = "none")))]
pub(crate) mod mock;
#[cfg(feature = "host-uac")]
pub mod uac;
```

In `usb/usb-classes/src/lib.rs`, add:

```rust
#[cfg(any(feature = "host-midi", feature = "host-uac"))]
pub mod host;
```

In the moved class files:
- Replace `crate::usb::host::` with `crate::host::`.
- In `uac/mod.rs`, change `pub(crate) mod shared` to `pub mod shared`, and `pub(crate) fn` inside it to `pub fn`.

Delete these lines in `crates/deluge-bsp/src/usb/host.rs`: `pub mod midi;`, `pub mod uac;`, and the `#[cfg(all(test, …))] pub(crate) mod mock;` pair. Then:
- Replace `crate::usb::host::midi::` with `usb_classes::host::midi::`, and `crate::usb::host::uac::` with `usb_classes::host::uac::`.
- Replace `super::uac::shared` with `usb_classes::host::uac::shared`.
- Change its `//!` header to say it holds the board's host *supervisor* and MIDI device registry. The class drivers are now in `usb_classes::host`.

In `crates/deluge-bsp/Cargo.toml`, change the feature to `usb-host = ["dep:embassy-usb-host", "usb-classes/host-midi", "usb-classes/host-uac"]`.

- [ ] **Step 2: Write the failing rate tests.** Append inside `usb/usb-classes/src/host/uac/mod.rs`'s `mod tests`, reusing its existing `uac2_mic_cfg` helper and `MockState`:

```rust
    /// UAC2 `SET CUR` on a clock source: class request, interface recipient, host-to-device.
    fn is_set_cur(setup: &[u8]) -> bool {
        setup[0] == 0x21 && setup[1] == 0x01
    }

    #[test]
    fn register_at_the_requested_rate_skips_set_cur() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
            .unwrap();
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        block_on(Uac::try_register(&alloc, 1, None, &cfg, 48_000)).expect("register at 48 kHz");
        assert!(!state.setups.borrow().iter().any(|s| is_set_cur(s)), "no SET CUR expected");
    }

    #[test]
    fn register_rejects_a_device_that_will_not_switch() {
        // Asked for 44.1 kHz, the device reports 48 kHz before and after SET CUR.
        let state = MockState::leak();
        for _ in 0..2 {
            state
                .control_reads
                .borrow_mut()
                .push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
                .unwrap();
        }
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        let err = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).err();
        assert!(matches!(err, Some(UacError::UnsupportedRate)), "got {err:?}");
    }
```

Add `, 44_100` as the last argument of every existing `Uac::try_register(` call in that test module (10 calls; `grep -n 'try_register(&alloc' usb/usb-classes/src/host/uac/mod.rs`).

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes --features host-midi,host-uac --lib`
Expected: FAIL to compile, `this function takes 4 arguments but 5 arguments were supplied`. Fix any *other* compile error first: those are path rewrites missed in Step 1.

- [ ] **Step 3: Take the rate as an argument.** In `usb/usb-classes/src/host/uac/mod.rs`:
  - Delete the `#[cfg(target_os = "none")] use crate::audio_block::SAMPLE_RATE_HZ;` / `#[cfg(not(…))] const SAMPLE_RATE_HZ: u32 = 44_100;` block and its comment.
  - Add `sample_rate_hz: u32` as the last parameter of `try_register`, and replace the four uses of `SAMPLE_RATE_HZ` in its body with `sample_rate_hz`.
  - Change its doc line `/// Match, claim pipes, negotiate 44.1 kHz, and select the streaming alt.` to `/// Match, claim pipes, negotiate \`sample_rate_hz\` on the clock source, and select the streaming alt.`

In `crates/deluge-bsp/src/usb/host.rs`, change the supervisor's call to `Uac::try_register(handle, addr, dev_info.split(), cfg, crate::audio_block::SAMPLE_RATE_HZ)`.

- [ ] **Step 4: Run the tests and the host-role build**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes --features device-midi,host-midi,host-uac --lib && cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`
Expected: PASS. usb-classes now also has every moved host test + 2.

- [ ] **Step 5: Update the UAC host test firmware.** In `hw-tests/uac-host-firmware/Cargo.toml` `[dependencies]`, add `usb-classes = { workspace = true, features = ["host-uac"] }`. In `src/tasks/loopback.rs:12`, change `use deluge_bsp::usb::host::uac;` to `use usb_classes::host::uac;`. In `src/main.rs:4`'s doc, change `deluge_bsp::usb::host::uac` to `usb_classes::host::uac`.

Run: `cargo build-uac && cargo build-fw -p deluge-controller && cargo build-fw -p demo-firmware`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A usb crates/deluge-bsp hw-tests/uac-host-firmware Cargo.lock
git commit -m "refactor(usb-classes): host MIDI/UAC drivers move out of the BSP; UAC takes its sample rate"
```

---

### Task 4: Device classes and the BOT engine move; BOT goes generic over endpoints

**Files:**
- Move: `crates/deluge-bsp/src/usb/classes/{audio,midi,msc}.rs` → `usb/usb-classes/src/{audio,midi,msc}.rs`
- Move: `crates/deluge-bsp/src/usb/bot.rs` → `usb/usb-classes/src/bot.rs`
- Create: `crates/deluge-bsp/src/usb/sd_block.rs` (`SdBlock`, cut from `bot.rs`)
- Delete: `crates/deluge-bsp/src/usb/classes/mod.rs`
- Modify: `crates/deluge-bsp/src/usb/mod.rs`, `crates/deluge-bsp/Cargo.toml`
- Modify consumers:
  - `firmwares/controller-firmware/{Cargo.toml,src/main.rs:89,:234,src/tasks/audio.rs:7}`
  - `firmwares/demo-firmware/{Cargo.toml,src/main.rs:64,:186,src/tasks/audio.rs:6}`
  - `firmwares/msc-firmware/{Cargo.toml,src/main.rs:66,:158,src/tasks/msc.rs:4,:12,:15}`
  - `firmwares/app-loader/{Cargo.toml,src/usbmsc.rs:20-21}`

**Interfaces:**
- Consumes: `usb-classes` (Tasks 2–3).
- Produces:
  - `usb_classes::audio::*` (was `deluge_bsp::usb::classes::audio`), `usb_classes::midi::*`, `usb_classes::msc::*`.
  - `usb_classes::bot::{BlockDevice, Inquiry, TX_BYTES, RX_BYTES, run, run_until, …}` with `pub async fn run<B: BlockDevice, EI: embassy_usb_driver::EndpointIn, EO: embassy_usb_driver::EndpointOut>(dev: B, ep_in: EI, ep_out: EO) -> !` and `pub async fn run_until<B, EI, EO>(dev: B, ep_in: EI, ep_out: EO, exit: &AtomicBool)`.
  - `deluge_bsp::usb::sd_block::SdBlock` (implements `usb_classes::bot::BlockDevice`).

- [ ] **Step 1: Move the files**

```bash
for f in audio midi msc; do git mv crates/deluge-bsp/src/usb/classes/$f.rs usb/usb-classes/src/$f.rs; done
git rm -q crates/deluge-bsp/src/usb/classes/mod.rs
git mv crates/deluge-bsp/src/usb/bot.rs usb/usb-classes/src/bot.rs
```

In `usb/usb-classes/src/lib.rs` add:

```rust
#[cfg(all(target_os = "none", feature = "device-audio"))]
pub mod audio;
#[cfg(all(target_os = "none", feature = "device-msc"))]
pub mod bot;
#[cfg(all(target_os = "none", feature = "device-midi"))]
pub mod midi;
#[cfg(all(target_os = "none", feature = "device-msc"))]
pub mod msc;
```

- [ ] **Step 2: Split `SdBlock` back into the BSP.** Cut the `// ── Ready-made SD-card backing ──` section from `usb/usb-classes/src/bot.rs`: the `SdBlock` struct, its doc, its `impl BlockDevice for SdBlock`, and any items only it uses. Paste it into a new `crates/deluge-bsp/src/usb/sd_block.rs` headed:

```rust
//! The SD card as a USB Mass Storage medium: a [`BlockDevice`] over [`crate::sd`], for the
//! board-agnostic Bulk-Only Transport engine in `usb_classes::bot`.

use usb_classes::bot::{BlockDevice, Inquiry};
```

(Add any other `usb_classes::bot` item the impl names, e.g. a default `Inquiry` constructor.) In `crates/deluge-bsp/src/usb/mod.rs`, replace the `bot` and `classes` module declarations and their doc bullets with:

```rust
/// The SD card as a Mass Storage medium for `usb_classes::bot`.
#[cfg(target_os = "none")]
pub mod sd_block;
```

and change the module's `//!` text so it says the class layer moved to the `usb-classes` crate, and this module keeps the Deluge's USB identities ([`ids`]), its SD-backed block device and its host supervisor.

In `crates/deluge-bsp/Cargo.toml`, add a device-only dependency that turns on `device-msc` for `SdBlock`:

```toml
[target.'cfg(target_os = "none")'.dependencies.usb-classes]
workspace = true
features = ["device-msc"]
```

- [ ] **Step 3: Make BOT generic over endpoints.** In `usb/usb-classes/src/bot.rs`:
  - Delete `use rza1l_hal::usb::{Rusb1EndpointIn, Rusb1EndpointOut};`.
  - Add `use embassy_usb_driver::{EndpointIn, EndpointOut};`.
  - Replace `crate::usb::classes::msc::take_reset` with `crate::msc::take_reset`.
  - Change the two entry points to:

```rust
pub async fn run<B: BlockDevice, EI: EndpointIn, EO: EndpointOut>(dev: B, ep_in: EI, ep_out: EO) -> ! {
```

```rust
pub async fn run_until<B: BlockDevice, EI: EndpointIn, EO: EndpointOut>(
    mut dev: B,
    mut ep_in: EI,
    mut ep_out: EO,
    exit: &AtomicBool,
) {
```

  - For every private helper that takes `&mut Rusb1EndpointIn` or `&mut Rusb1EndpointOut` (`grep -n 'Rusb1Endpoint' usb/usb-classes/src/bot.rs`), add an `EI: EndpointIn` / `EO: EndpointOut` type parameter and take `&mut EI` / `&mut EO`.
  - BOT only calls the trait methods `write`, `read` and `wait_enabled`, so no body changes.

Run: `grep -rn 'rza1l_hal\|deluge_bsp\|crate::usb::\|crate::sd' usb/usb-classes/src`
Expected: no output.

- [ ] **Step 4: Point consumers at the new crate.** Add to each manifest's `[dependencies]`:
  - `firmwares/controller-firmware/Cargo.toml` and `firmwares/demo-firmware/Cargo.toml`: `usb-classes = { workspace = true, features = ["device-audio"] }`
  - `firmwares/msc-firmware/Cargo.toml` and `firmwares/app-loader/Cargo.toml`: `usb-classes = { workspace = true, features = ["device-msc"] }`

Then rewrite the imports. Paths change only in the module prefix:
  - `deluge_bsp::usb::classes::audio` → `usb_classes::audio` (controller main.rs:89,:234, tasks/audio.rs:7; demo main.rs:64,:186, tasks/audio.rs:6)
  - `deluge_bsp::usb::classes::msc` → `usb_classes::msc` (msc main.rs:66,:158; app-loader usbmsc.rs:21)
  - `use deluge_bsp::usb::bot::{self, SdBlock};` → `use deluge_bsp::usb::sd_block::SdBlock;` + `use usb_classes::bot;` (msc tasks/msc.rs:12)
  - `pub use deluge_bsp::usb::bot::{RX_BYTES, TX_BYTES};` → `pub use usb_classes::bot::{RX_BYTES, TX_BYTES};` (msc tasks/msc.rs:15)
  - `use deluge_bsp::usb::bot::{self, BlockDevice, SdBlock};` → `use deluge_bsp::usb::sd_block::SdBlock;` + `use usb_classes::bot::{self, BlockDevice};` (app-loader usbmsc.rs:20)
  - the doc link at msc tasks/msc.rs:4 → `usb_classes::bot` / `deluge_bsp::usb::sd_block`

Run: `grep -rn 'deluge_bsp::usb::\(classes\|bot\|midi_packets\)' --include='*.rs' --exclude-dir=target crates firmwares hw-tests wren examples`
Expected: no output.

- [ ] **Step 5: Build every device-class user**

Run: `cargo build-fw -p deluge-controller && cargo build-fw -p demo-firmware && cargo build-msc && cargo build-app-loader && cargo build-uac && cargo build-wren && tools/build-examples.sh`
Expected: PASS. Fix compile errors only by path or generic-parameter edits. A behaviour change is a plan defect: stop and ledger it.

- [ ] **Step 6: Run the tests**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes --features device-midi,host-midi,host-uac --lib && cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`
Expected: PASS.

- [ ] **Step 7: Prove the features are independent.** Each firmware builds with only its own class features (Review Focus 5):

Run: `cargo tree -p msc-firmware -e features --target armv7a-none-eabihf 2>/dev/null | grep -o 'usb-classes feature "[a-z-]*"' | sort -u`
Expected: exactly `device-midi` (from `deluge-bsp`) and `device-msc`. `device-audio` must not appear. Repeat for `deluge-controller`: expect `device-audio`, `device-midi` and `device-msc` (the BSP's `SdBlock`), and no `host-*` unless its manifest enables `deluge-bsp/usb-host`.

- [ ] **Step 8: Commit**

```bash
git add -A usb crates/deluge-bsp firmwares Cargo.lock
git commit -m "refactor(usb-classes): UAC2/MIDI/MSC device classes and the BOT engine leave the BSP; BOT is generic over endpoints"
```

---

### Task 5: SCUX path recipes move into `rza1l_hal::scux`; BSP keeps thin wrappers

**Files:**
- Move: `crates/deluge-bsp/src/scux_dvu_path.rs` → `crates/rza1l-hal/src/scux/dvu_path.rs`
- Move: `crates/deluge-bsp/src/scux_src_path.rs` → `crates/rza1l-hal/src/scux/src_path.rs`
- Move: `crates/deluge-bsp/src/scux_usb_tx_path.rs` → `crates/rza1l-hal/src/scux/usb_tx_path.rs`
- Create: `crates/deluge-bsp/src/scux_dvu_path.rs`, `scux_src_path.rs`, `scux_usb_tx_path.rs` (wrappers)
- Modify: `crates/rza1l-hal/src/scux.rs` (declare the three modules)

**Interfaces:**
- Produces:
  - `rza1l_hal::scux::dvu_path::{init(ssi: &SsiConfig, ffd_dma_ch: u8), init_with_src(ssi: &SsiConfig, ffd_dma_ch: u8, engine_hz: u32), …every other item unchanged}`
  - `rza1l_hal::scux::src_path::init(ffd_dma_ch: u8, ffu_dma_ch: u8, fin_hz: u32, fout_hz: u32)`
  - `rza1l_hal::scux::usb_tx_path::init(ffd_dma_ch: u8, ffu_dma_ch: u8, fin_hz: u32, fout_hz: u32)`
  - The BSP modules keep their exact public API (`deluge_bsp::scux_dvu_path::init()`, etc.), so firmware doesn't change.

This task is a move. The moved code drives the SCUX and DMAC registers directly (not through the `mmio` test seam), so it has no host tests today. Behaviour is pinned by the unchanged DMA-channel constants and by the device builds. The functional check is the hardware smoke test (Task 8).

- [ ] **Step 1: Move the files**

```bash
git mv crates/deluge-bsp/src/scux_dvu_path.rs crates/rza1l-hal/src/scux/dvu_path.rs
git mv crates/deluge-bsp/src/scux_src_path.rs crates/rza1l-hal/src/scux/src_path.rs
git mv crates/deluge-bsp/src/scux_usb_tx_path.rs crates/rza1l-hal/src/scux/usb_tx_path.rs
```

In `crates/rza1l-hal/src/scux.rs`, after `mod regs;`:

```rust
/// CPU → SCUX DVU → SSI0 output path (hardware volume, ramps, zero-cross mute).
pub mod dvu_path;
/// CPU → SCUX async SRC → CPU path, for bridging two clock domains.
pub mod src_path;
/// Engine ↔ USB-rate async SRC path (2SRC1/0).
pub mod usb_tx_path;
```

- [ ] **Step 2: Parameterise the board facts.** In each moved file:
  - `rza1l_hal::` → `crate::` (the files now live inside the HAL).
  - Every `crate::system::SCUX_*_DMA_CH` becomes the matching function argument (`ffd_dma_ch` / `ffu_dma_ch`).
  - `crate::system::SSI_CONFIG` becomes the `ssi: &SsiConfig` argument.
  - Uses *outside* `init` (dvu_path.rs:333, src_path.rs:185 and :194, usb_tx_path.rs:183 and :192) read a channel the path stored at init. Add to each file:

```rust
use core::sync::atomic::{AtomicU8, Ordering};

/// DMA channels this path was initialised with (`u8::MAX` = not yet initialised).
static FFD_DMA_CH: AtomicU8 = AtomicU8::new(u8::MAX);
static FFU_DMA_CH: AtomicU8 = AtomicU8::new(u8::MAX); // src_path / usb_tx_path only
```

  - `init`/`init_with_src` store their arguments first thing: `FFD_DMA_CH.store(ffd_dma_ch, Ordering::Relaxed);` (and `FFU_…`).
  - Each later use reads `FFD_DMA_CH.load(Ordering::Relaxed)` (or `FFU_…`).

Then run `tools/check-layering.sh`. If it flags a `deluge` word on a code line of a moved file, reword that line (comments are allowed).

- [ ] **Step 3: Write the BSP wrappers.** `crates/deluge-bsp/src/scux_dvu_path.rs`:

```rust
//! The Deluge's SCUX DVU output path: [`rza1l_hal::scux::dvu_path`] bound to the board's SSI0
//! configuration and DMA channel.

pub use rza1l_hal::scux::dvu_path::*;

/// [`rza1l_hal::scux::dvu_path::init`] with the Deluge's SSI0 config and FFD0 DMA channel.
///
/// # Safety
/// As [`rza1l_hal::scux::dvu_path::init`].
pub unsafe fn init() {
    unsafe {
        rza1l_hal::scux::dvu_path::init(&crate::system::SSI_CONFIG, crate::system::SCUX_FFD0_DMA_CH)
    }
}

/// [`rza1l_hal::scux::dvu_path::init_with_src`] with the Deluge's SSI0 config and FFD0 DMA channel.
///
/// # Safety
/// As [`rza1l_hal::scux::dvu_path::init_with_src`].
pub unsafe fn init_with_src(engine_hz: u32) {
    unsafe {
        rza1l_hal::scux::dvu_path::init_with_src(
            &crate::system::SSI_CONFIG,
            crate::system::SCUX_FFD0_DMA_CH,
            engine_hz,
        )
    }
}
```

`crates/deluge-bsp/src/scux_src_path.rs`:

```rust
//! The Deluge's SCUX async SRC path: [`rza1l_hal::scux::src_path`] on the board's FFD1/FFU1
//! DMA channels.

pub use rza1l_hal::scux::src_path::*;

/// [`rza1l_hal::scux::src_path::init`] on the Deluge's FFD1/FFU1 DMA channels.
///
/// # Safety
/// As [`rza1l_hal::scux::src_path::init`].
pub unsafe fn init(fin_hz: u32, fout_hz: u32) {
    unsafe {
        rza1l_hal::scux::src_path::init(
            crate::system::SCUX_FFD1_DMA_CH,
            crate::system::SCUX_FFU1_DMA_CH,
            fin_hz,
            fout_hz,
        )
    }
}
```

`crates/deluge-bsp/src/scux_usb_tx_path.rs`: the same as the previous file, with `usb_tx_path` and `SCUX_FFD2_DMA_CH` / `SCUX_FFU2_DMA_CH`, and the doc line `//! The Deluge's SCUX USB-rate SRC path: [`rza1l_hal::scux::usb_tx_path`] on the board's FFD2/FFU2 DMA channels.`

(The local `init`/`init_with_src` shadow the glob-imported ones; everything else passes straight through.) Keep any `#[cfg(...)]` the BSP's `lib.rs` already puts on these three `pub mod` lines.

- [ ] **Step 4: Build and test**

Run: `cargo build-fw -p deluge-controller && cargo build-fw -p demo-firmware && cargo build-uac && tools/build-examples.sh && cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib && cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib && tools/check-layering.sh`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates/rza1l-hal/src/scux.rs crates/rza1l-hal/src/scux crates/deluge-bsp/src
git commit -m "refactor(rza1l-hal): SCUX DVU/SRC path recipes move into the HAL; the BSP binds the Deluge's DMA channels"
```

---

### Task 6: The simulator backend leaves the BSP, behind a hook

**Files:**
- Create: `crates/deluge-bsp/src/host_panel.rs`
- Move: `crates/deluge-bsp/src/sim.rs` → `crates/deluge-sdk/src/plat/sim_panel.rs`
- Modify:
  - `crates/deluge-bsp/src/lib.rs:55-56` (the `sim` module → `host_panel`)
  - `crates/deluge-bsp/src/pic.rs:346-375` (host `tx` and `read_byte`)
  - `crates/deluge-bsp/src/oled.rs:486-498` (host `capture_frame`)
  - `crates/deluge-bsp/Cargo.toml` (drop the `sim-link` feature and `deluge-sim-link`; host `embassy-time` stops being optional)
  - `crates/deluge-sdk/Cargo.toml:80` (`sim` feature), `crates/deluge-sdk/src/plat/mod.rs`, `crates/deluge-sdk/src/plat/sim.rs:17,:25`, `crates/deluge-sdk/src/host.rs`, `crates/deluge-sdk/src/lib.rs:772`
  - `crates/deluge-simulator/src/lib.rs:165` (doc reference)

**Interfaces:**
- Produces (BSP, host builds only: `#[cfg(not(target_os = "none"))]`):
  - `pub trait deluge_bsp::host_panel::HostPanel: Sync { fn display(&self, frame: &[u8]); fn pic_command(&self, bytes: &[u8]); fn next_pic_byte(&self) -> Option<u8>; }`
  - `pub fn deluge_bsp::host_panel::install(panel: &'static dyn HostPanel)` (panics on a second call)
  - `pub fn deluge_bsp::host_panel::installed() -> Option<&'static dyn HostPanel>`
- Produces (SDK, `sim` feature): `crate::plat::sim_panel::{install(panel: SharedPanel, audio: BrainEnds), panel() -> Option<&'static SharedPanel>, render_block, BLOCK_FRAMES, SAMPLE_RATE_HZ, Sample}`. These are the same items `deluge_bsp::sim` had.

- [ ] **Step 1: Write the hook with its tests and stubbed bodies.** `crates/deluge-bsp/src/host_panel.rs`:

```rust
//! Host builds only: a hook for a simulated front panel.
//!
//! On the device the PIC drives the pads and LEDs and the OLED is on SPI. On the host, the
//! peripheral modules' host arms call the [`HostPanel`] installed here instead: every outbound
//! PIC command ([`crate::pic`]), every OLED frame ([`crate::oled`]), and the PIC bytes the panel's
//! input would produce. With nothing installed they stay no-ops. The BSP knows nothing about
//! which simulator implements the hook; `deluge-sdk`'s `sim` backend installs one.

use std::sync::OnceLock;

/// A simulated front panel.
pub trait HostPanel: Sync {
    /// Show one OLED frame (`oled::FrameBuffer::as_bytes` layout).
    fn display(&self, frame: &[u8]);
    /// Apply one whole PIC command, as the PIC would.
    fn pic_command(&self, bytes: &[u8]);
    /// The next byte the PIC would send for the panel's input, if any is waiting.
    fn next_pic_byte(&self) -> Option<u8>;
}

/// Holds the one installed panel.
struct Slot(OnceLock<&'static dyn HostPanel>);

impl Slot {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    fn install(&self, panel: &'static dyn HostPanel) {
        todo!()
    }

    fn get(&self) -> Option<&'static dyn HostPanel> {
        todo!()
    }
}

static SLOT: Slot = Slot::new();

/// Install the host panel. Call once, before the program starts.
///
/// # Panics
/// If a panel is already installed.
pub fn install(panel: &'static dyn HostPanel) {
    SLOT.install(panel);
}

/// The installed panel, if any.
pub fn installed() -> Option<&'static dyn HostPanel> {
    SLOT.get()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::vec::Vec;

    /// Records what the BSP sends it, and serves queued input bytes.
    #[derive(Default)]
    pub(crate) struct RecordingPanel {
        pub(crate) frames: Mutex<Vec<Vec<u8>>>,
        pub(crate) commands: Mutex<Vec<Vec<u8>>>,
        pub(crate) input: Mutex<Vec<u8>>,
    }

    impl HostPanel for RecordingPanel {
        fn display(&self, frame: &[u8]) {
            self.frames.lock().unwrap().push(frame.to_vec());
        }
        fn pic_command(&self, bytes: &[u8]) {
            self.commands.lock().unwrap().push(bytes.to_vec());
        }
        fn next_pic_byte(&self) -> Option<u8> {
            let mut input = self.input.lock().unwrap();
            (!input.is_empty()).then(|| input.remove(0))
        }
    }

    pub(crate) fn leak_panel() -> &'static RecordingPanel {
        Box::leak(Box::default())
    }

    #[test]
    fn slot_is_empty_until_installed() {
        let slot = Slot::new();
        assert!(slot.get().is_none());
        slot.install(leak_panel());
        assert!(slot.get().is_some());
    }

    #[test]
    #[should_panic(expected = "host_panel::install")]
    fn installing_twice_panics() {
        let slot = Slot::new();
        slot.install(leak_panel());
        slot.install(leak_panel());
    }
}
```

Tests never touch the global `SLOT`: other BSP tests (e.g. `host_read_byte_parks_and_does_not_resolve`) depend on no panel being installed.

In `crates/deluge-bsp/src/lib.rs`, replace

```rust
#[cfg(all(not(target_os = "none"), feature = "sim-link"))]
pub mod sim;
```

with

```rust
/// Host builds: the hook a simulated front panel plugs into.
#[cfg(not(target_os = "none"))]
pub mod host_panel;
```

Leave `sim.rs` in place for now (it is no longer compiled, and moves in Step 6).

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib -- host_panel`
Expected: both tests FAIL with `not yet implemented`.

- [ ] **Step 2: Implement the slot**

```rust
    fn install(&self, panel: &'static dyn HostPanel) {
        if self.0.set(panel).is_err() {
            panic!("deluge_bsp::host_panel::install called twice");
        }
    }

    fn get(&self) -> Option<&'static dyn HostPanel> {
        self.0.get().copied()
    }
```

Run the same command. Expected: 2 passed.

- [ ] **Step 3: Write the failing dispatch tests.** Append to `crates/deluge-bsp/src/pic.rs`'s `mod tests`:

```rust
    #[test]
    fn tx_without_a_panel_is_a_no_op() {
        embassy_futures::block_on(tx_via(None, &[1, 2, 3]));
    }

    #[test]
    fn tx_forwards_whole_commands_to_the_panel() {
        let panel = crate::host_panel::tests::leak_panel();
        embassy_futures::block_on(tx_via(Some(panel), &[1, 2, 3]));
        assert_eq!(*panel.commands.lock().unwrap(), [vec![1u8, 2, 3]]);
    }

    #[test]
    fn read_byte_returns_the_panels_input() {
        let panel = crate::host_panel::tests::leak_panel();
        panel.input.lock().unwrap().extend([0x90, 7]);
        assert_eq!(embassy_futures::block_on(read_byte_via(Some(panel))), 0x90);
        assert_eq!(embassy_futures::block_on(read_byte_via(Some(panel))), 7);
    }
```

Append to `crates/deluge-bsp/src/oled.rs`'s `mod tests`:

```rust
    #[test]
    fn capture_without_a_panel_needs_none() {
        // No assertion on `captured_frame()`: it is global, and other oled tests capture into it
        // in parallel. The existing capture tests cover the capture surface itself.
        capture_frame_via(None, &FrameBuffer::new());
    }

    #[test]
    fn capture_forwards_the_frame_to_the_panel() {
        let panel = crate::host_panel::tests::leak_panel();
        let fb = FrameBuffer::new();
        capture_frame_via(Some(panel), &fb);
        assert_eq!(*panel.frames.lock().unwrap(), [fb.as_bytes().to_vec()]);
    }
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`
Expected: FAIL to compile, `cannot find function tx_via` / `read_byte_via` / `capture_frame_via`.

- [ ] **Step 4: Route the host arms through the hook.** In `crates/deluge-bsp/src/pic.rs`, replace the host `tx` and host `read_byte` (lines ~346-375) with:

```rust
/// Host/QEMU stand-in for [`tx`]: no PIC UART off-target. With a host panel installed
/// ([`crate::host_panel`]), the command is applied to it as the PIC would apply it; otherwise it
/// is logged and dropped. Every outbound helper funnels through here, one whole command per call.
/// Resolves without ever suspending.
#[cfg(not(target_os = "none"))]
async fn tx(bytes: &[u8]) {
    tx_via(crate::host_panel::installed(), bytes).await
}

#[cfg(not(target_os = "none"))]
async fn tx_via(panel: Option<&dyn crate::host_panel::HostPanel>, bytes: &[u8]) {
    if let Some(panel) = panel {
        panel.pic_command(bytes);
    }
    log::debug!("pic(host): tx {:?}", bytes);
}

/// Host: no PIC co-processor. With a host panel installed, its input arrives as the bytes the
/// PIC would send. Otherwise no bytes ever arrive and this parks forever — the pump task stays
/// alive but quiescent. That is a clean idle, not a hang.
#[cfg(not(target_os = "none"))]
pub async fn read_byte() -> u8 {
    read_byte_via(crate::host_panel::installed()).await
}

/// How often the host [`read_byte`] looks for panel input when it has none.
#[cfg(not(target_os = "none"))]
const HOST_INPUT_POLL_MS: u64 = 1;

#[cfg(not(target_os = "none"))]
async fn read_byte_via(panel: Option<&dyn crate::host_panel::HostPanel>) -> u8 {
    let Some(panel) = panel else {
        return core::future::pending().await;
    };
    loop {
        if let Some(byte) = panel.next_pic_byte() {
            return byte;
        }
        embassy_time::Timer::after_millis(HOST_INPUT_POLL_MS).await;
    }
}
```

In `crates/deluge-bsp/src/oled.rs`, replace `capture_frame` (lines ~486-498) with:

```rust
/// Copy `fb` into the host capture surface, standing in for actually driving the panel, and onto
/// the host panel when one is installed ([`crate::host_panel`]). Shared by the host
/// [`send_frame`] and [`draw_blocking`].
#[cfg(not(target_os = "none"))]
fn capture_frame(fb: &FrameBuffer) {
    capture_frame_via(crate::host_panel::installed(), fb);
}

#[cfg(not(target_os = "none"))]
fn capture_frame_via(panel: Option<&dyn crate::host_panel::HostPanel>, fb: &FrameBuffer) {
    let mut captured = CAPTURED_FRAME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *captured = fb.clone();
    BOOT_FRAME_CAPTURED.store(true, core::sync::atomic::Ordering::Release);
    if let Some(panel) = panel {
        panel.display(fb.as_bytes());
    }
}
```

In `crates/deluge-bsp/Cargo.toml`:
- Delete the `sim-link` feature and its doc lines.
- Delete the host `deluge-sim-link` dependency and its comment.
- Change the host `embassy-time` line to non-optional with the comment `# Polls the host panel's input for \`pic::read_byte\`.`:

```toml
embassy-time = { workspace = true }
```

- [ ] **Step 5: Run the BSP tests**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`
Expected: PASS, including the existing `host_read_byte_parks_and_does_not_resolve`.

- [ ] **Step 6: Move the simulator into the SDK**

```bash
git mv crates/deluge-bsp/src/sim.rs crates/deluge-sdk/src/plat/sim_panel.rs
```

In `crates/deluge-sdk/src/plat/sim_panel.rs`:
- Replace `crate::encoder::`, `crate::pic`, `crate::oled`, `crate::rgb` with `deluge_bsp::encoder::`, `deluge_bsp::pic`, `deluge_bsp::oled`, `deluge_bsp::rgb`.
- Change `pub(crate) fn display`, `pub(crate) fn pic_command` and `fn next_input_byte` to private `fn`.
- Delete the old `pub(crate) async fn read_byte` and its `INPUT_POLL_MS` (the BSP's `read_byte_via` now does the polling).
- Add:

```rust
/// The simulator's side of [`deluge_bsp::host_panel`].
struct SimPanel;

impl deluge_bsp::host_panel::HostPanel for SimPanel {
    fn display(&self, frame: &[u8]) {
        display(frame);
    }
    fn pic_command(&self, bytes: &[u8]) {
        pic_command(bytes);
    }
    fn next_pic_byte(&self) -> Option<u8> {
        next_input_byte()
    }
}
```

and make the last statement of `pub fn install(panel, audio)` `deluge_bsp::host_panel::install(&SimPanel);`. Update the `//!` header: this is the SDK's simulator backend. It keeps the shared panel the simulator window renders, and plugs into the BSP through `deluge_bsp::host_panel`. Keep the bullet list, rewriting `[`crate::pic`]`-style links to `deluge_bsp::…`.

In `crates/deluge-sdk/src/plat/mod.rs`, declare it next to `sim` under the same cfg:

```rust
pub(crate) mod sim_panel;
```

Then rewrite the SDK's uses:
- `lib.rs:772` `deluge_bsp::sim::install(` → `crate::plat::sim_panel::install(`
- `host.rs:14` `deluge_bsp::sim::panel()` → `crate::plat::sim_panel::panel()`, plus its doc sentence naming `deluge_bsp::sim`
- `plat/sim.rs:17` `deluge_bsp::sim::{BLOCK_FRAMES, SAMPLE_RATE_HZ}` → `super::sim_panel::{BLOCK_FRAMES, SAMPLE_RATE_HZ}`, and `:25` `deluge_bsp::sim::render_block` → `super::sim_panel::render_block`

In `crates/deluge-sdk/Cargo.toml`, change the feature to `sim = ["dep:deluge-sim-link", "dep:deluge-simulator"]`. In `crates/deluge-simulator/src/lib.rs:165`, change the doc reference `deluge_bsp::sim::install` to `deluge_sdk`'s sim backend (`deluge::__rt::host`), which installs the panel through `deluge_bsp::host_panel`.

- [ ] **Step 7: Run the SDK sim tests and a simulator build**

Run: `cargo test --target x86_64-unknown-linux-gnu -p deluge-sdk --features sim && grep -rn 'deluge_bsp::sim\|sim-link\|deluge_sim_link' --include='*.rs' --include='Cargo.toml' --exclude-dir=target crates/deluge-bsp crates/deluge-sdk crates/deluge-simulator`
Expected: tests PASS, including `sim_panel`'s moved tests. The grep prints only `deluge-sdk` lines that use `deluge_sim_link`, and nothing under `crates/deluge-bsp`.

Then run an example in the headless simulator: `cargo deluge sim --headless -p blinky` (or the repo's documented headless invocation in `docs/simulator.md`) for a few seconds.
Expected: it starts and logs as it did on the baseline. Record what you ran.

- [ ] **Step 8: Commit**

```bash
git add -A crates/deluge-bsp crates/deluge-sdk crates/deluge-simulator Cargo.lock
git commit -m "refactor(bsp): the simulator backend moves into deluge-sdk behind a host-panel hook"
```

---

### Task 7: Enforce layering rules 3 and 4

**Files:**
- Modify: `tools/check-layering.sh`

- [ ] **Step 1: Add the rules.** Insert before the final `echo "==> Layering check passed."`:

```bash
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
```

Update the header comment's bullet list with the two new rules.

- [ ] **Step 2: Prove each rule catches a regression, then revert**

```bash
echo 'fn _probe() { let _ = rza1l_hal::UNCACHED_MIRROR_OFFSET; }' >> usb/usb-classes/src/lib.rs
tools/check-layering.sh; echo "exit=$?"
git checkout usb/usb-classes/src/lib.rs
echo 'fn _probe() { let _ = deluge_sim_link::ALL_PADS_BYTES; }' >> crates/deluge-bsp/src/lib.rs
tools/check-layering.sh; echo "exit=$?"
git checkout crates/deluge-bsp/src/lib.rs
tools/check-layering.sh
```

Expected: the first two runs print their probe and `exit=1`; the last prints `==> Layering check passed.`

- [ ] **Step 3: Commit**

```bash
git add tools/check-layering.sh
git commit -m "chore(layering): usb/ and drivers/ stay board-agnostic; the BSP never names the simulator"
```

---

### Task 8: Docs, CHANGELOG, spec note, full verification, hardware smoke

**Files:**
- Modify: `CHANGELOG.md`, `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 2 section), `docs/advanced-guide.md` (§11 module map, if it lists moved modules), `crates/deluge-bsp/README.md` (module list, if it lists moved modules)

- [ ] **Step 1: Find stale references**

Run: `grep -rnE 'usb::(classes|bot|midi_packets)|usb::host::(uac|midi)|deluge_bsp::sim\b|sim-link|scux_(dvu|src|usb_tx)_path' --include='*.md' docs crates usb drivers README.md | grep -v '^docs/superpowers/\(plans\|specs\)/2026-0'`
Expected: a list of doc lines. Update each to the new home: `usb_classes::…`, `deluge_bsp::usb::sd_block`, `deluge_bsp::host_panel`, or `rza1l_hal::scux::…`, noting that the `deluge_bsp::scux_*_path` wrappers still exist. Re-run until only intentional lines remain.

- [ ] **Step 2: CHANGELOG.** Under `## [Unreleased]` → `### Changed`, add:

```markdown
- `deluge-bsp` holds only Deluge wiring now:
  - USB device classes (UAC2, USB-MIDI, MSC), the Bulk-Only Transport engine, the USB-MIDI
    packet queues and the USB host MIDI/UAC drivers moved to the new board-agnostic
    `usb-classes` crate (one feature per class). **Breaking:** import them from `usb_classes::…`.
    The SD-backed `SdBlock` is now `deluge_bsp::usb::sd_block::SdBlock`. `bot::run`/`run_until`
    are generic over any `embassy_usb_driver` endpoints. `Uac::try_register` takes the sample
    rate to negotiate.
  - The MAX5136 CV DAC protocol is the new `max5136` embedded-hal driver crate.
  - The SCUX DVU/SRC path recipes are in `rza1l_hal::scux::{dvu_path, src_path, usb_tx_path}`,
    taking their DMA channels; `deluge_bsp::scux_*_path` keep their old API on the Deluge's
    channels.
  - The desktop-simulator backend moved into `deluge-sdk`; the BSP exposes
    `deluge_bsp::host_panel` for it. **Breaking:** the `deluge-bsp/sim-link` feature and
    `deluge_bsp::sim` are gone.
```

- [ ] **Step 3: Spec note.** In the spec's `### Phase 2: split \`deluge-bsp\`` section, append:

```markdown
**Done (2026-10-14 plan):** classes went into one `usb-classes` crate (features per class, unpublished);
the simulator moved behind `deluge_bsp::host_panel`; MAX5136 became `drivers/max5136`. SPI-NOR needed no
extraction — `rza1l_hal::spibsc` already holds the generic controller and `FlashMap`, and
`deluge_bsp::flash` is only the Deluge's part size and flash layout.
```

- [ ] **Step 4: Full verification against the baseline**

```bash
tools/test.sh 2>&1 | tail -2
tools/build-examples.sh 2>&1 | tail -1
cargo build-fw -p deluge-controller 2>&1 | tail -1
cargo build-fw -p demo-firmware 2>&1 | tail -1
cargo build-app-loader 2>&1 | tail -1
cargo build-msc 2>&1 | tail -1
cargo build-uac 2>&1 | tail -1
cargo build-wren 2>&1 | tail -1
tools/check-layering.sh
```

Expected: everything passes; no failures beyond B1.

- [ ] **Step 5: Hardware smoke test on a real Deluge.** Flash each firmware the usual way and check:
  - controller-firmware: CV out tracks a MIDI note (MAX5136 path).
  - controller-firmware: audio plays and the host sees the UAC2 device (device-audio).
  - controller-firmware: master volume fades smoothly (SCUX DVU path).
  - msc-firmware: the SD card mounts on a computer; copy a file onto it and back (BOT generic).
  - uac-host-firmware with a USB audio interface attached: capture loops back (host UAC at 44.1 kHz).
  - app-loader: USB mass-storage mode mounts.

Write down any deviation. Any failure blocks the merge.

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md docs crates/deluge-bsp/README.md
git commit -m "docs: Phase 2 — usb-classes, max5136, SCUX paths in the HAL, host-panel hook"
```
