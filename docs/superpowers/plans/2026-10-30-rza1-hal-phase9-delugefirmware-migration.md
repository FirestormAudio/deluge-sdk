# rza1-hal Phase 9: DelugeFirmware#next Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** DelugeFirmware's Rust BSP (`src/bsp/rust`) builds and runs on the finished pre-0.1 SDK API, on a DelugeFirmware branch, pinned to a pushed deluge-sdk revision.

**Architecture:** Five tasks:
1. A small SDK change: `BoardPins` gains the battery LED and the voltage-sense pin. The SDK main is then pushed; the user approved this on 2026-10-30.
2. DelugeFirmware's manifests move to that revision and to the `rza1-hal` crate, and every call site moves to the current API. Its audio C ABI fills the SSI rings by offset through the positional window; its USB audio runs on `usb::iso` streams.
3. A behaviour fix the SDK's SD cancel safety calls for.
4. Verification on host, image and bench.
5. The SDK drops the hidden paths kept for DelugeFirmware (local commit; pushing it is asked separately).

**Tech Stack:** Rust nightly (DelugeFirmware's `rust-toolchain.toml`), `./dbt` (CMake + cargo, clang/lld cross-language LTO), embassy 0.10/0.5, J-Link bench on pi-top.

**Spec:** `docs/superpowers/specs/2026-10-21-rza1-hal-pre-release-api-design.md`. Relevant parts:
- §5 item 9: "**DelugeFirmware#next migration.** A follow-up DF branch against the finished API, built through `local-sdk.toml`, with the C ABI's ring-by-offset fill on the positional window."
- the user decision "DelugeFirmware#next gets **one** bump at the end (phase 9)".

## Global Constraints

- **Repositories:**
  - SDK: `/home/kate/GitHub/deluge-sdk`, remote `https://github.com/FirestormAudio/deluge-sdk.git`. Local main is 88 commits ahead of `origin/main` (f3c07e86) and fast-forwards.
  - DelugeFirmware: `/home/kate/GitHub/DelugeFirmware`, branch `next` (= `origin/next`, d1d1b359f). The work goes on a new branch, `sdk-pre-0.1-api`, cut from `next`.
- **Pushes:**
  - The SDK main push in Task 1 is approved (2026-10-30). It must be a fast-forward (`git push origin main`, never `--force`).
  - Nothing else is pushed without asking: not the DelugeFirmware branch, and not Task 5's SDK commit.
- **SDK layering rule:** nothing under `rza1/` names the Deluge in code.
- **SDK verification:** `scratchpad/verify.sh` (`tools/test.sh`, check-layering, link-layout, every firmware/hw-test/example build) PASS, and `rza1/tools/test.sh` passes.
- **DelugeFirmware's pin after Task 1:**
  - `Cargo.toml` deps use `git = "https://github.com/FirestormAudio/deluge-sdk.git", rev = "<the pushed main SHA>"`.
  - `local-sdk.toml`'s header becomes `[patch."https://github.com/FirestormAudio/deluge-sdk.git"]`. It must match the URL in `Cargo.toml` character for character.
  - `local-sdk.toml` stays gitignored and is never committed.
- **DelugeFirmware lockfiles:** never commit a `Cargo.lock` produced through `local-sdk.toml` (paths). The committed lockfiles come from a build without it.
- **DelugeFirmware features:**
  - `deluge-bsp`: `default-features = false` (DelugeFirmware links its own `linker/memory*.x` and its own C `rza1_cpu_fault_report`), `features = ["flash"]`.
  - `rza1-hal`: `features = ["rza1l", "unstable-driver-steal"]` (`Dma::steal` in `peripherals/fault.rs:109`).
- **Interrupt priorities:** DelugeFirmware's SGI8 = `Priority::P20` and SGI9 = `Priority::P24` (both below PMR 31), unchanged.
- **DelugeFirmware builds and tests:**
  - device: `./dbt build` and `./dbt build relwithdebinfo`;
  - host: `cd src/bsp/rust && cargo test --locked`;
  - emulator: `./dbt sim`;
  - harnesses: `./dbt test --build-only golden-vt-render` and the fs-differential equivalent;
  - image size: `./dbt sizediff` against `next`.

## Review Focus

1. **The boot header in DelugeFirmware's image.**
   - Failure: DelugeFirmware links with clang/lld, cross-language LTO and `--gc-sections`, unlike the SDK's firmwares.
   - Expected: `.boot_header` lands at `_start + 0x20`, so the FSB accepts the flash image.
   - Test: `python3 /home/kate/GitHub/deluge-sdk/tools/check-boot-header.py <DF ELF>` on the debug and release images (Task 4).
2. **Audio ring reads before the rings exist.**
   - Failure: `capture_stamp::now_for` (MIDI arrival stamps) and `deluge_audio_frames_until_block_offset` can run before `deluge_board_init_audio`.
   - Expected: a play/record head of 0, no panic, no fault.
   - Test: `tx_play_frame_is_zero_before_audio_init` in `src/audio/device.rs` (Task 2, host).
3. **Nested ring access.**
   - Failure: `audio::with_tx` / `with_rx` return `None` when nested, so calling `tx_play_frame()` inside a `with_tx` closure would silently read 0.
   - Expected: the fill loop uses the ring it already holds.
   - Test: `flush_writes_at_the_play_head`, which seeds the host ring position and asserts the samples land at it, not at 0 (Task 2, host). If the host build cannot reach a ring, the bench check in Task 4, Step 4 (glitch-free `speaker-test`, exact `arecord` sizes) covers it.
4. **An SD transfer dropped mid-flight.**
   - Failure: the SDK now resets the controller and reports not-ready until `sd::init`. DelugeFirmware re-inits only on an insertion edge, so the card stays dead until it is re-inserted.
   - Expected: a card that is inserted but not ready is re-initialised.
   - Test: `card_service_reinits_an_inserted_card_that_is_not_ready` (Task 3, host).
5. **Image size growth past the budget** (`docs/dev/image_size.md`).
   - Failure: the HAL's iso scratch is now 4 × 1 KiB, and the drivers changed.
   - Expected: within the documented budget.
   - Test: `./dbt sizediff` against `next`, with the result in the ledger and the commit message (Task 4).

---

### Task 1: SDK — battery LED and voltage-sense pins; push SDK main

**Repo:** deluge-sdk, branch `feat/df-pins` from main.

**Files:**
- Modify:
  - `crates/deluge-bsp/src/board.rs` (`BoardPins`, `Board::new`, the `board_pins_name_every_pin_the_bsp_drives` test);
  - `crates/deluge-bsp/src/battery.rs`;
  - `CHANGELOG.md`.

**Interfaces:**
- Produces:
  - `BoardPins.battery_led: Peri<'static, P1_1>`, documented "The battery LED (P1_1), open-drain: low lights it".
  - `BoardPins.volt_sense: Peri<'static, P1_13>`, documented "The battery-voltage sense input (P1_13, AN5; [`crate::battery::init`])".
  - `pub fn battery::init(pin: Peri<'static, P1_13>)`: muxes P1_13 to alternative 1 (AN5) with `Flex::new(pin).set_as_alternate(Alternate::Alt1)`. It keeps no handle; the pin stays muxed. `start_conversion` / `read_raw` are unchanged.
- [ ] **Step 1: Write the failing tests** (deluge-bsp, QEMU via `scratchpad/bt.sh`):
  - Extend `board_pins_name_every_pin_the_bsp_drives` to include P1_1 and P1_13.
  - Add `battery_init_muxes_volt_sense_to_an5`: after `mmio::test::reset()` and `battery::init(unsafe { P1_13::steal() })`, PMC1 bit 13 is set and the PFC/PFCE/PFCAE bits for P1_13 select alternative 1. Copy the expected register sequence from an existing alternate-mux test (e.g. `trigger_clock`'s P1_14 Alt2).
- [ ] **Step 2: Run them to verify they fail**
  Run: `scratchpad/bt.sh battery` and `scratchpad/bt.sh board_pins`
  Expected: FAIL (no field / no `battery::init`).
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Verify**
  Run: Step 2's commands, then `verify.sh`.
  Expected: all PASS.
- [ ] **Step 5: Commit, merge, push**
  - Commit `deluge-bsp: BoardPins hands out the battery LED and the voltage-sense pin`.
  - No-ff merge into main, run `verify.sh` on main, then `git push origin main` (fast-forward; approved).
  - Record the pushed SHA in the ledger: Task 2 pins it.

### Task 2: DelugeFirmware on the current API

**Repo:** DelugeFirmware, new branch `sdk-pre-0.1-api` from `next`.

**Files:**
- Modify, under `src/bsp/rust/`:
  - `Cargo.toml`: lines 31-46, 57, 105-107 and 185; the URL, the rev, `rza1-hal`, the features, and `[profile.release.package.rza1-hal]`;
  - `local-sdk.toml`: the header URL, and a `rza1-hal = { path = "../../../deluge-sdk/rza1/rza1-hal" }` line in place of `rza1l-hal`; fix the stale "three SDK entries" comment;
  - `linker/memory.x:26-27`, `linker/memory_rtt.x:54-55` (drop `RZA1_STATUS_LED_*`);
  - `src/main.rs`;
  - `src/runtime/device.rs`;
  - `src/peripherals/{board.rs, signals.rs, usb.rs, usb_audio.rs, fault.rs, display.rs, flash.rs, cv_gate.rs, midi.rs}`;
  - `src/audio/{device.rs, capture_stamp.rs}`.
- Modify, outside `src/bsp/rust/`: the lockfiles `src/bsp/rust/Cargo.lock`, `harness/golden_vt_render/Cargo.lock` and `tests/component/differential/fs_differential/Cargo.lock`, regenerated without `local-sdk.toml`.

**Interfaces:**
- **Crate rename:** `rza1l_hal::` becomes `rza1_hal::` everywhere. The old crate name stays only in history and comments that mean it.
- **Interrupts** (`runtime/device.rs:241-267`): `gic::set_priority(id, p)` becomes `<SGI8 as rza1_hal::interrupt::Interrupt>::set_priority(Priority::P20)` (SGI9: `P24`). `gic::enable(id)` becomes `unsafe { SGI8::enable() }`. Drop the now-redundant `unsafe` around `interrupt::bind`.
- **Pins:** take `board.pins` and pass each group to the init that takes it:
  - `uart::init_pic(scif, pins.pic, rx_dma, tx_dma, baud)`;
  - `uart::init_midi(scif, pins.midi, rx_dma, baud)`;
  - `sd::init_hw(card, pins.sd, tx, rx)`;
  - `unsafe encoder::irq_init(timer, pins.encoders)`;
  - `bus::init(spi, pins.spi, oled_dma)`;
  - `cv_gate::init(pins.cv_gate)` (safe now);
  - `unsafe trigger_clock::irq_init(pins.trigger)`;
  - `audio::init(ssi, pins.audio, tx, rx)`;
  - `jacks::init(pins.jacks)`;
  - `battery::init(pins.volt_sense)`.
  - The owned outputs live in statics: the SYNC LED is `Output::new(pins.sync_led, Level::Low)`, and the battery LED is `Output::new(pins.battery_led, Level::High)` (off).
- **Signals** (`peripherals/signals.rs`, `board.rs:69-95`):
  - `deluge_signal_write(BATTERY_LED, on)` drives the battery-LED output (on = low), and `SYNC_LED` drives the SYNC LED output.
  - Jack reads and the speaker enable use `jacks::is_inserted(Jack::..)` / `jacks::set_speaker_enable`.
  - `CODEC_ENABLE` becomes a no-op with a comment: `audio::init` powers the codec, and the app only ever writes `true` (`deluge.cpp:598`).
  - The P1_14 and P6_1 mux calls go: `trigger_clock::irq_init` and `cv_gate::init` own those pins.
- **`mtu2::OneShot::new(timer, 64, Irqs)`:** no priority argument; `Board::new` sets TGI2A to P5.
- **`display.rs:129-132`:** `uart::steal_pic_rx().and_then(|rx| rx.try_read().ok().flatten())`.
- **Audio** (`src/audio/device.rs`), the positional window, the spec's "ring-by-offset fill":
  - `TX_FRAMES = deluge_bsp::audio::SSI_TX_WORDS / 2` and `RX_FRAMES = SSI_RX_WORDS / 2`, with the masks from them.
  - `fn tx_play_frame() -> usize`: `audio::with_tx(|t| t.position()).unwrap_or(0) / 2 & TX_MASK`. `rx_write_frame` is the same with `with_rx`.
  - `deluge_audio_start` caches `RING_TX: *mut u32` from `with_tx(|t| t.ring_mut().as_mut_ptr())` and `RING_RX: *const u32` from `with_rx(|r| r.ring().as_ptr())`, in `AtomicPtr`s.
  - `flush_pending` and `prepare_input_block` keep today's volatile loops over those pointers. This keeps the copy out of a critical section, so the P5 gate timer and the USB interrupt are not delayed.
  - Samples are `i32 as u32` / `u32 as i32` at the boundary.
  - Null pointers (before init) mean no-op.
- **USB audio** (`src/peripherals/usb_audio.rs`), modelled on `deluge-sdk/firmwares/demo-firmware/src/tasks/audio.rs` (`SpeakerSink` 32-41/98-169, `MicSource` 266-339):
  - `struct SpeakerSink;` with `impl IsoOutConsumer` (`on_packet`) holds the current `iso_out_hook` body. `struct CaptureSource;` with `impl IsoInProducer` (`fill_packet -> usize`) holds the `iso_in_hook` body.
  - Both are `static mut`, lent with `&mut *addr_of_mut!(..)`.
  - `build` binds them with `ep_out.into_iso_stream(..)` / `ep_in.into_iso_stream(..)` and keeps the two streams in `AudioHandler`, which lives in a `'static` slot.
  - Delete `USB_PORT`, the `out_pipe`/`in_pipe` fields, `in_mps`, and the alt-1 `pipe_*` / `regs::usb` workaround in `set_alternate_setting`, keeping its alt/generation/epoch stores.
- **Runtime:** `src/main.rs` keeps its `extern "C" fn main` (the reset handler's `bl main` still calls it). Only the stale `rza1l-hal` comments change.
- [ ] **Step 1: Write the failing host tests** in `src/audio/device.rs` (`#[cfg(test)]`, run by `cargo test`):
  - `tx_play_frame_is_zero_before_audio_init`: with no ring set up, `tx_play_frame() == 0` and `rx_write_frame() == 0`.
  - `flush_writes_at_the_play_head`: point `RING_TX` at a test buffer of `SSI_TX_WORDS` words, set the play head the fill code reads to frame 100, push 4 frames through `flush_pending`, and assert they land at words 200..208 of the test buffer.
  - If the play head comes only from `with_tx` (not settable on host), factor the fill into a pure `fn fill_at(ring: *mut u32, play_frame: usize, frames: &[[i32; 2]])` and test that. Ledger it.
- [ ] **Step 2: Run them to verify they fail**
  Run: `cd src/bsp/rust && cargo test --config local-sdk.toml audio::device`
  Expected: FAIL (not compiling: the crate is not migrated yet).
- [ ] **Step 3: Implement**
  - Manifests first, with the Task 1 SHA. Check that the patch applies with `cargo tree --config local-sdk.toml -i rza1-hal`, which must show the local path.
  - Then the call sites, as in Interfaces, until `./dbt build` (through `local-sdk.toml`) links.
- [ ] **Step 4: Verify**
  Run:
  - `cargo test --config local-sdk.toml` (all host tests);
  - `./dbt build`;
  - `./dbt build relwithdebinfo`;
  - `./dbt sim`.
  Expected: all pass/link, with no new warnings in `src/bsp/rust`.
- [ ] **Step 5: Lockfiles**
  - Move `local-sdk.toml` aside, run `cargo generate-lockfile` (or `cargo update -p deluge-bsp -p rza1-hal …`) in `src/bsp/rust` and in the two harness packages, then `cargo test --locked`.
  - Restore `local-sdk.toml`.
  - Expected: `--locked` passes against the pushed rev.
- [ ] **Step 6: Commit** in DelugeFirmware's style (`build(sdk): …`, `refactor(bsp): …`), one commit per area: manifests+lockfiles, rename+interrupts+pins, audio window, USB iso streams.

### Task 3: Re-initialise an inserted card the SDK reports not ready

**Repo:** DelugeFirmware branch.

**Files:**
- Modify: `src/bsp/rust/src/storage/sd.rs` (`card_service`, around lines 504-532).

**Interfaces:**
- Produces: in `card_service`, besides the insertion edge, a card that `sd::is_inserted() && !sd::is_ready()` (after an SDK cancel reset, `bsp/src/sd.rs:117-131`) gets the same re-init request.
  - Rate-limit it to the service's existing poll period. A card whose init keeps failing must not spin.
  - Factor the decision into `fn needs_reinit(inserted: bool, ready: bool, edge: bool) -> bool` for the host test.
- [ ] **Step 1: Write the failing test** `card_service_reinits_an_inserted_card_that_is_not_ready`:
  - `needs_reinit(true, false, false) == true`;
  - `needs_reinit(true, true, false) == false`;
  - `needs_reinit(false, false, false) == false`;
  - `needs_reinit(true, true, true) == true`.
- [ ] **Step 2: Run it to verify it fails**
  Run: `cd src/bsp/rust && cargo test --locked needs_reinit`
  Expected: FAIL (no `needs_reinit`).
- [ ] **Step 3: Implement** as in Interfaces.
- [ ] **Step 4: Verify**
  Run: `cargo test --locked`, then `./dbt build`.
  Expected: PASS.
- [ ] **Step 5: Commit** `fix(sd): re-initialise an inserted card the SDK reset after a dropped transfer`

### Task 4: Verification — image, size, bench

**Repo:** DelugeFirmware branch; bench pi-top ([[deluge-testbench-pi-top]]).

**Files:**
- Modify: DelugeFirmware's `docs/dev/image_size.md`, only if the budget note needs updating.
- [ ] **Step 1: Boot header**
  Run: `python3 /home/kate/GitHub/deluge-sdk/tools/check-boot-header.py <debug ELF>`, then the same on the release ELF. `./dbt build` prints the ELF paths.
  Expected: no output, exit 0. If it fails, fix in DelugeFirmware's link setup (keep `.boot_header` / `__rza1_boot_header` alive under LTO) and ledger the fix.
- [ ] **Step 2: Size**
  Run: `./dbt sizediff` against `next`.
  Expected: within `docs/dev/image_size.md`'s budget. Record the delta.
- [ ] **Step 3: Harnesses**
  Run: `./dbt test --build-only golden-vt-render` and the fs-differential build.
  Expected: build.
- [ ] **Step 4: Bench** (RAM run: from DelugeFirmware, `python3 tools/deluge_jlink.py <ELF> --select IP=pi-top --timeout 40 --fail-on '\bfault|PANIC'`).
  Expected:
  - it boots to the UI with no fault and a clean RTT log (OLED drawn, PIC link up, SD mounted);
  - on the Pi, `lsusb` shows the Deluge;
  - `speaker-test -D hw:Deluge -F S24_3LE -c 2 -r 44100` plays;
  - a 3 s `arecord` at S24_3LE is exactly 793,844 bytes, and at S16_LE 529,244 bytes (the alt-1 path the workaround used to cover);
  - USB MIDI enumerates.
- [ ] **Step 5: Hands-on (the user):** pads, encoders, OLED, audio out and in, DIN MIDI, CV/gate, the battery LED. List these as open in the final message if not done.
- [ ] **Step 6: Commit** any fixes found (one per finding, test first where host-testable).

### Task 5: SDK — retire the paths kept for DelugeFirmware

**Repo:** deluge-sdk, branch `feat/retire-df-paths` from main.

**Files:**
- Modify:
  - `rza1/rza1-hal/src/usb/mod.rs` (`pipe` and `regs` become `pub(crate) mod`, no `#[doc(hidden)]`);
  - `rza1/rza1-hal/src/usb/pipe.rs` (the four functions become `pub(crate)`);
  - `rza1/rza1-hal/src/usb/regs.rs` (`usb` becomes `pub(crate)`);
  - `rza1/tools/test.sh` (drop the `unstable_paths` run);
  - `CHANGELOG.md`;
  - `crates/deluge-bsp/Cargo.toml` (delete the no-op `app-fault-reporter`; DelugeFirmware no longer passes it after Task 2).
- Delete: `rza1/rza1-hal/tests/unstable_paths.rs`.
- [ ] **Step 1: Write the failing check.** Add to `tools/check-layering.sh`: `grep -rn 'doc(hidden)' rza1/rza1-hal/src/usb/{mod.rs,pipe.rs,regs.rs}` must print nothing. Run it: FAIL.
- [ ] **Step 2: Implement** the Files list.
- [ ] **Step 3: Verify**
  Run: `verify.sh`, `rza1/tools/test.sh`.
  Expected: PASS. Also, from DelugeFirmware, `./dbt build` through `local-sdk.toml` still links (it no longer uses the paths).
- [ ] **Step 4: Commit** `rza1-hal: the USB pipe and register internals are crate-private; DelugeFirmware no longer reaches them`. Merge locally; **ask before pushing**.
