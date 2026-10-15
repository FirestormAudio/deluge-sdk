# RZ/A1 Phase 4c-1: DelugeFirmware on the 4b SDK Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** DelugeFirmware#next builds and boots on deluge-sdk's current API (Phases 4a and 4b). That needs one SDK addition: a fault-safe PIC receive that shares the PIC task's read position.

**Architecture:** One SDK task, then DelugeFirmware's changes. In the SDK, `UartRx` keeps each ring's read position in a static that the owner and a stolen reader share. `deluge_bsp::uart::steal_pic_rx` hands that stolen reader to fault and freeze paths, whose PIC task is suspended while holding the receiver's lock. In DelugeFirmware:
- `start()` builds the `Board` and hands its peripherals to the BSP's `init_*` calls.
- `start()` brings up the RSPI0 bus and the CV DAC before the executor runs, so the OLED task can't hold the bus at that moment.
- The MIDI/gate timer becomes an `mtu2::OneShot` behind a typed TGI2A binding.
- The fault and freeze paths use `Dma::steal`, `uart::try_write_pic` and `uart::steal_pic_rx`.

A last task pins DelugeFirmware to the pushed SDK revision. It is gated on the human partner pushing deluge-sdk.

**Tech Stack:** Rust nightly. deluge-sdk crates `rza1l-hal` and `deluge-bsp`. DelugeFirmware's `src/bsp/rust` crate (`deluge-bsp-rust`, target `armv7a-deluge-eabihf.json`). `./dbt build` (CMake C++ then cargo, cross-language LTO). embassy-sync 0.8 (`OnceLock`).

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 4). The 4a plan said "Phase 4b … migrates DelugeFirmware#next". The human partner split the remainder into this plan (4c-1) and a later SDK plan (4c-2). 4c-2 covers SDHI, SSI+SCUX, USB and the GPIO IRQs, deletes `dmac::raw` and `gic::register`, and finishes with DelugeFirmware's SGI and USB0 bindings and the SSI API.

## Global Constraints

- **Two repositories:**
  - deluge-sdk at `/home/kate/GitHub/deluge-sdk`. Task 1 works on branch `feat/pic-rx-steal`, cut from `main`.
  - DelugeFirmware at `/home/kate/GitHub/DelugeFirmware`. Tasks 2–3 work on branch `sdk-ownership`, cut from `next` (HEAD `dd3ad98bd`, working tree clean).
- **The local override.** DelugeFirmware pins the SDK at git rev `28bc21cebe6d473a5e23a45e31c09f028c740675` of `https://github.com/FirestormAudio/deluge-embassy.git`, in six dependencies of `src/bsp/rust/Cargo.toml`. The gitignored `src/bsp/rust/local-sdk.toml` patches all six to the sibling deluge-sdk working copy. `./dbt build` passes it automatically; plain cargo needs `--config local-sdk.toml`. Task 2 builds against the deluge-sdk working copy, so deluge-sdk must have `feat/pic-rx-steal` (Task 1) checked out.
- **Cargo.lock.** A build through the override rewrites `src/bsp/rust/Cargo.lock` (the file's own header says so). **Run `git -C /home/kate/GitHub/DelugeFirmware checkout src/bsp/rust/Cargo.lock` before every DelugeFirmware commit**, and never commit a lock that names path sources.
- **DelugeFirmware checks:**
  - Rust device check: `cd /home/kate/GitHub/DelugeFirmware/src/bsp/rust && cargo check --target armv7a-deluge-eabihf.json -Zbuild-std=core,alloc -Zjson-target-spec --no-default-features --features rtt --config local-sdk.toml`.
  - Host tests: `cargo test --config local-sdk.toml` in the same directory. The baseline against the local SDK is 221 passed, 0 failed.
  - Full image: `./dbt build` from the repo root.
- **deluge-sdk checks:**
  - HAL tests: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib`; BSP tests add `-p deluge-bsp --features usb-host`.
  - Suite: `tools/test.sh`.
  - Firmware builds: `cargo build-fw -p <pkg>`.
  - zsh does not word-split a variable that holds a command, so write commands out in full.
- **Stopping points.** Never push either repository, and never publish. Task 3 starts only after the human partner has merged `feat/pic-rx-steal` into deluge-sdk `main` and pushed it to `origin`. Until then, the executor stops after Task 2 and reports.
- **4c-2 scope stays out:** `gic::register` (still used here for SGI 8/9 and USB0), `rza1l_hal::ssi`, `deluge_bsp::sd` and `usb::init_device_mode` keep their current forms.

## Decisions settled before execution

1. **Split and order** (human partner): this plan migrates DelugeFirmware onto the 4b API now, which also unbreaks its local `./dbt build`. SDK Phase 4c-2 follows, then a short DelugeFirmware follow-up.
2. **Fault-safe PIC read shares the read position.** DelugeFirmware's freeze loop used to read the PIC ring through `rza1l_hal::uart::try_read_dma`. That function shared one read pointer with the PIC task, so bytes the freeze consumed (the resume knob press) were not delivered again afterwards. 4b's `try_read_pic` returns `None` while the PIC task is waiting, because that task holds the receiver's lock, so a freeze could never be resumed. A stolen reader with its own read pointer would replay the resume press to the app after the freeze. So `UartRx` keeps each ring's read position in a static shared by the owner and the stolen reader.
3. **CV DAC and the bus come up in `start()`, before the executor.**
   - `cv_gate::init` now requires `bus::init` first.
   - It takes RSPI0 with `try_lock` and panics if an OLED frame holds the bus.
   - DelugeFirmware called it from `deluge_board_init_early`, which C++ runs during `deluge_app_init` in `app_task`. By then `display::oled_render` (spawned first) may be drawing.
   - Moving both calls into `start()`, with interrupts masked and the time driver running (`delay_ms` polls OSTM0), removes the race. The 7-segment variant's `P6_1` SSL mux stays in `deluge_board_init_early` and still runs after `cv_gate::init` configures `P6_1`, as before.
4. **The gate timer keeps its own handler.** `deluge_bsp::midi_gate` only turns a CV gate off. DelugeFirmware's handler calls the app's `midiAndGateTimerGoneOff`. So `signals.rs` builds its own `OneShot` (P0φ/64, priority 5, same as before) with a TGI2A binding. Arm and cancel are no-ops before setup, where they used to write unconfigured registers.
5. **USB0 and SGI 8/9 keep `gic::register` in this plan.** They compile on 4b, and moving them belongs with 4c-2's deletion of `gic::register`.

## Review Focus

1. **Resuming a frozen display.** The select-knob press that resumes `FREEZE_WITH_ERROR` must work while the PIC task is suspended mid-read. That press, and any byte the freeze loop read, must not reach the app again afterwards. Pinned by `a_stolen_receiver_shares_the_owners_read_position_across_the_ring_wrap` (Task 1).
2. **CV bring-up against the OLED.** `cv_gate::init` must run before `oled_render` can hold RSPI0, or boot panics with "cv_gate::init found RSPI0 busy". Pinned structurally (Task 2 calls it in `start()` before `executor.run`), and on hardware (smoke test 1).
3. **Gate-timer arm or cancel before setup.** An early `deluge_midi_gate_timer_arm_at` from the app must not panic or write an unconfigured MTU2 channel. Task 2's `if let Some(..)` guards cover it. `signals.rs` is device-only, so this is checked by review and hardware smoke test 3.
4. **A wedged PIC transmit DMA in a fault flush.** `Dma::steal::<PicTxDma>().stop()` must write what the old `dmac::stop(10)` wrote. Already pinned by 4a's `stopping_an_owned_channel_writes_what_raw_stop_writes`.
5. **Boot order.** `Board::new` must have brought up SDRAM, the MMU, the caches and the time driver before `boot_mem::init_sdram_memory` and the logger, as `init_clocks` did. Pinned by `Board::new`'s documented contract, and by hardware smoke test 1.

---

## File Structure

| Repo | Path | Change |
|---|---|---|
| deluge-sdk | `crates/rza1l-hal/src/uart.rs` | Ring read position in `RX_READ[ring]`; `UartRx::steal` |
| deluge-sdk | `crates/deluge-bsp/src/uart.rs` | `steal_pic_rx`; `try_read_pic`'s doc points fault paths to it |
| DelugeFirmware | `src/bsp/rust/src/runtime/device.rs` | `Board`, BSP init calls with parts, bus + CV before the executor |
| DelugeFirmware | `src/bsp/rust/src/peripherals/board.rs` | `deluge_board_init_early` no longer calls `cv_gate::init` |
| DelugeFirmware | `src/bsp/rust/src/peripherals/signals.rs` | Gate timer on `mtu2::OneShot` |
| DelugeFirmware | `src/bsp/rust/src/peripherals/{fault,display}.rs` | `Dma::steal`, `try_write_pic`, `steal_pic_rx` |
| DelugeFirmware | `src/bsp/rust/src/boot_mem.rs`, `linker/sdram_sections.x`, `docs/dev/image_size.md` | `init_clocks` wording |
| DelugeFirmware | `src/bsp/rust/Cargo.toml`, `Cargo.lock` | Task 3: the new SDK revision |

---

### Task 1: The PIC ring's read position is shared, and a fault path can steal the receiver (deluge-sdk)

**Files:**
- Modify: `crates/rza1l-hal/src/uart.rs` (`UartRx`, `typed_tests`)
- Modify: `crates/deluge-bsp/src/uart.rs`

**Interfaces:**
- Produces:
  - `pub unsafe fn rza1l_hal::uart::UartRx::steal<T: RxDmaInstance, C: rza1l_hal::dmac::Channel>() -> Option<UartRx<'static>>`, which returns `None` before T's ring starts.
  - `pub unsafe fn deluge_bsp::uart::steal_pic_rx() -> Option<rza1l_hal::uart::UartRx<'static>>`.
  - `UartRx` loses its `read` field; the position lives in a private `static RX_READ: [AtomicU32; 2]`.

- [ ] **Step 1: Branch**

```bash
cd /home/kate/GitHub/deluge-sdk && git checkout -b feat/pic-rx-steal main
```

- [ ] **Step 2: Write the failing test**

In `crates/rza1l-hal/src/uart.rs`, `mod typed_tests`, `received_bytes_come_out_in_order_across_the_ring_wrap`: after its last line (`assert_eq!(rx.try_read(), None);`), add:

```rust

        // A stolen receiver (a fault path's, while the owner is suspended) reads from the same
        // position: what it consumes, the owner doesn't deliver again.
        unsafe {
            ring.add(1).write(0x11);
            ring.add(2).write(0x22);
        }
        test::poke32(crda, rx.base + 3);
        let mut stolen = unsafe { UartRx::steal::<SCIF1, DMA_CH12>() }.expect("SCIF1's ring runs");
        assert_eq!(stolen.try_read(), Some(0x11));
        assert_eq!(rx.try_read(), Some(0x22));
        assert_eq!(stolen.try_read(), None);
```

Rename the test to `a_stolen_receiver_shares_the_owners_read_position_across_the_ring_wrap`, so it carries Review Focus 1's name. (The two assertions belong to one test because the ring statics are shared by the tests' threads.) In `a_uart_is_configured_bound_and_moves_both_directions_by_dma`, replace `assert_eq!(rx.read, rx.base, "the ring starts empty");` with:

```rust
        assert_eq!(RX_READ[0].load(Ordering::Relaxed), rx.base, "the ring starts empty");
```

- [ ] **Step 3: Run it to see it fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib uart 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: compile errors: no `steal` on `UartRx`, no `RX_READ`.

- [ ] **Step 4: Implement**

In `crates/rza1l-hal/src/uart.rs`:
1. Change the import `use core::sync::atomic::{AtomicBool, Ordering};` to `use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};`.
2. After the `TX_DMA_ACTIVE` static, add:

```rust
/// Where the CPU reads each receive ring next: a cached address, as CRDA reports it. A ring's
/// owner and a stolen reader ([`UartRx::steal`]) share it, so bytes one consumes the other doesn't
/// deliver again.
static RX_READ: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
```

3. Replace the `UartRx` struct with:

```rust
/// A UART's receiver: a 256-byte ring the DMAC fills from the receive FIFO, with no interrupt.
pub struct UartRx<'d> {
    ch: usize,
    /// Its ring, which indexes `DMA_RX_BUF` and [`RX_READ`].
    ring: usize,
    dma: Dma<'d>,
    /// The ring's start (cached address, as CRDA reports it).
    base: u32,
}
```

4. In `UartRx::start`, replace the final `Self { ch, dma, base: buf_base, read: buf_base }` with:

```rust
            RX_READ[T::RING].store(buf_base, Ordering::Release);
            Self { ch, ring: T::RING, dma, base: buf_base }
```

5. Replace `try_read` with:

```rust
    /// The next received byte, or `None` if the ring is empty.
    pub fn try_read(&mut self) -> Option<u8> {
        let read = RX_READ[self.ring].load(Ordering::Acquire);
        // CRDA: where the DMAC writes next.
        if self.dma.current_dst() == read {
            return None;
        }
        // SAFETY: `read` is inside this UART's ring. Read through the uncached alias: the DMAC
        // wrote physical RAM, and a cached line may be stale.
        let byte = unsafe { crate::uncached(read as usize as *mut u8).read_volatile() };
        let offset = (read - self.base + 1) & (DMA_RX_BUF_SIZE as u32 - 1);
        RX_READ[self.ring].store(self.base + offset, Ordering::Release);
        Some(byte)
    }

    /// SCIF `T`'s receiver, read through its ring's DMA channel `C`, without the owner: for a
    /// fault or freeze path whose owner is suspended mid-read. It reads from the owner's position,
    /// so a byte it consumes isn't delivered to the owner again. `None` until T's ring has
    /// started ([`Uart::new`]).
    ///
    /// # Safety
    /// `C` is the channel T's ring runs on, and nothing else reads T's ring while the stolen
    /// receiver is in use: interrupts masked, or the owner's executor blocked in the caller.
    pub unsafe fn steal<T: RxDmaInstance, C: crate::dmac::Channel>() -> Option<UartRx<'static>> {
        if !RX_DMA_ACTIVE[T::INDEX].load(Ordering::Relaxed) {
            return None;
        }
        // SAFETY: the ring's address only; the caller's contract covers its use.
        let base = unsafe { core::ptr::addr_of!(DMA_RX_BUF[T::RING].0[0]) as u32 };
        Some(UartRx {
            ch: T::INDEX,
            ring: T::RING,
            // SAFETY: the caller's contract: only this reader uses the channel meanwhile, and it
            // only reads CRDA.
            dma: unsafe { Dma::steal::<C>() },
            base,
        })
    }
```

In `crates/deluge-bsp/src/uart.rs`, after `try_read_pic`, add:

```rust
/// The PIC UART's receiver without its lock, for a fault or freeze path: the PIC task may be
/// suspended mid-read holding the lock, which makes [`try_read_pic`] return `None`. It reads from
/// the PIC task's position, so a byte it consumes isn't delivered again. `None` before
/// [`init_pic`].
///
/// # Safety
/// Nothing else reads the PIC ring while the receiver is in use: interrupts masked, or the
/// executor that runs the PIC task blocked in the caller.
pub unsafe fn steal_pic_rx() -> Option<UartRx<'static>> {
    // SAFETY: the PIC ring runs on PicRxDma; the caller's contract covers the rest.
    unsafe { UartRx::steal::<PicScif, PicRxDma>() }
}
```

and append to `try_read_pic`'s doc: `A fault path, where the PIC task may hold the receiver, uses [`steal_pic_rx`].`

- [ ] **Step 5: Run the tests**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED"`
Expected: `ok`.

Run: `tools/test.sh > /tmp/claude-1000/p4c1-t1.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/p4c1-t1.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `exit 0`, `failed 0`.

Run: `for p in app-loader deluge-controller demo-firmware msc-firmware wren-firmware; do printf '%s: ' $p; cargo build-fw -p $p 2>&1 | tail -1; done; tools/link-layout.sh check | tail -1`
Expected: five `Finished` lines; the layout matches.

- [ ] **Step 6: Commit**

```bash
git add crates/rza1l-hal/src/uart.rs crates/deluge-bsp/src/uart.rs
git commit -m "feat(rza1l-hal,bsp): a UART ring's read position is shared, so a fault path can steal the PIC receiver without replaying its bytes"
```

---

### Task 2: DelugeFirmware builds on the 4b SDK (DelugeFirmware)

**Files** (all under `/home/kate/GitHub/DelugeFirmware/src/bsp/rust/` unless noted):
- Modify: `src/runtime/device.rs` (`start`, `:150-206`; `app_task` comment `:284`)
- Modify: `src/peripherals/board.rs` (`deluge_board_init_early`)
- Modify: `src/peripherals/signals.rs` (imports; gate-timer section)
- Modify: `src/peripherals/fault.rs` (`:13`, `deluge_fault_pad_flush`)
- Modify: `src/peripherals/display.rs` (`deluge_display_freeze`)
- Modify: `src/boot_mem.rs:4`, `linker/sdram_sections.x:11`, `/home/kate/GitHub/DelugeFirmware/docs/dev/image_size.md:224`

**Interfaces:**
- Consumes:
  - deluge-sdk main plus Task 1: `deluge_bsp::{Board, system::BOARD_CONFIG}`, `rza1l_hal::init`.
  - `deluge_bsp::uart::{init_pic(Peri<PicScif>, Peri<PicRxDma>, Peri<PicTxDma>, u32), init_midi(Peri<MidiScif>, Peri<MidiRxDma>, u32), try_write_pic(&[u8]) -> usize, steal_pic_rx() -> Option<UartRx<'static>>}`.
  - `deluge_bsp::encoder::irq_init(Peri<EncoderPollTimer>)` (unsafe), `deluge_bsp::bus::init(Peri<SharedSpi>, Peri<OledDma>)`, `deluge_bsp::cv_gate::init()` (unsafe).
  - `rza1l_hal::mtu2::OneShot`, `rza1l_hal::interrupt::{Handler, typelevel::TGI2A}`, `rza1l_hal::bind_interrupts!`, `rza1l_hal::dmac::Dma::steal::<deluge_bsp::board::PicTxDma>()`.
- Produces: `pub(crate) fn signals::gate_timer_setup(timer: rza1l_hal::Peri<'static, deluge_bsp::board::GateTimer>)` (now safe).

- [ ] **Step 1: Branch, and see it fail**

```bash
cd /home/kate/GitHub/DelugeFirmware && git checkout -b sdk-ownership next
git -C /home/kate/GitHub/deluge-sdk branch --show-current   # expect: feat/pic-rx-steal
cd src/bsp/rust && cargo check --target armv7a-deluge-eabihf.json -Zbuild-std=core,alloc -Zjson-target-spec --no-default-features --features rtt --config local-sdk.toml 2>&1 | grep -E "^error" | sort | uniq -c
```

Expected: 16 errors. They cover `init_clocks` missing, the argument counts of `init_pic`, `init_midi` and `irq_init`, `try_read_dma`/`try_write_fifo` missing, and private `mtu2` and `dmac` functions.

- [ ] **Step 2: Boot hands out the board**

In `src/runtime/device.rs` `start()`, replace:

```rust
    // CPG/PLL, MMU, L1+L2 caches, SDRAM controller, GIC, and the embassy-time (OSTM) driver.
    unsafe { deluge_bsp::system::init_clocks() };
```

with:

```rust
    // The chip (CPG/PLL, module clocks, GIC), then the board (MMU, L1+L2 caches, SDRAM controller,
    // the embassy-time OSTM driver). The board's peripherals come back by role.
    let board = deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG));
```

Then replace the block from `    // GIC source registration, before the global interrupt enable below.` through `    unsafe { signals::gate_timer_setup() };` with:

```rust
    // GIC source registration, before the global interrupt enable below. The PIC UART starts at
    // 31250 bps; `control::pic_pump` runs the handshake up to 200000 bps.
    deluge_bsp::uart::init_pic(board.pic_uart, board.dma.pic_rx, board.dma.pic_tx, 31_250);
    deluge_bsp::uart::init_midi(board.midi_uart, board.dma.midi_rx, 31_250);
    // The encoders' A-pin interrupts, and their 1 ms pin poll on MTU2 ch3.
    unsafe { deluge_bsp::encoder::irq_init(board.encoder_poll_timer) };
    // The sample-accurate MIDI/gate one-shot timer (MTU2 ch2), armed at render time through
    // `deluge_midi_gate_timer_arm_at`.
    signals::gate_timer_setup(board.gate_timer);
    // RSPI0, shared by the OLED and the CV DAC, then the DAC's linearity sequence on it. Both run
    // before the executor: `display::oled_render` draws as soon as the PIC is ready, and the DAC
    // bring-up must not find RSPI0 mid-frame.
    deluge_bsp::bus::init(board.shared_spi, board.dma.oled);
    // SAFETY: once, at startup, with interrupts masked and the time driver running (the DAC's
    // settle delay polls OSTM0).
    unsafe { deluge_bsp::cv_gate::init() };
```

In `app_task`, change `// SAFETY: we own this pin; GPIO clocks are up after init_clocks.` to `// SAFETY: we own this pin; GPIO clocks are up after Board::new.`

- [ ] **Step 3: Board init no longer brings up the CV DAC**

In `src/peripherals/board.rs`, `deluge_board_init_early` (device version), replace:

```rust
        // CV DAC over RSPI0 (the BSP driver sets up the SPI peripheral + pins).
        deluge_bsp::cv_gate::init();
```

with:

```rust
        // The CV DAC over RSPI0 is already up: `runtime::device::start` brings up the bus and the
        // DAC before the executor runs, so the OLED can't hold RSPI0 when it does.
```

In its doc comment, change `and trigger-clock input, and the CV DAC SPI.` to `and trigger-clock input.`

- [ ] **Step 4: The gate timer on `mtu2::OneShot`**

In `src/peripherals/signals.rs`, change `use rza1l_hal::{gpio, mtu2};` to:

```rust
use deluge_bsp::board::GateTimer;
use embassy_sync::once_lock::OnceLock;
use rza1l_hal::interrupt::{Handler, typelevel};
use rza1l_hal::mtu2::OneShot;
use rza1l_hal::{Peri, gpio};
```

In the section comment above the gate-timer constants, change `so this module drives `mtu2` directly.` to `so this module builds its own `mtu2::OneShot` on the board's MTU2 channel 2.`

Replace from `/// MTU2 channel for the MIDI/gate one-shot (the C firmware's `TIMER_MIDI_GATE_OUTPUT`).` through the end of `gate_timer_setup` with the following. It keeps `GATE_TIMER_PRESCALER`, `GATE_TIMER_TICKS_PER_SEC` and `GATE_TIMER_PRIORITY` with their docs, and deletes the `GATE_TIMER_CH` constant.

```rust
/// Prescaler P0/64 ≈ 516 kHz: 1 tick ≈ 1.94 µs, longest one-shot ≈ 127 ms.
const GATE_TIMER_PRESCALER: u16 = 64;
/// Timer ticks per second at [`GATE_TIMER_PRESCALER`]: P0 (33 MHz) / 64.
const GATE_TIMER_TICKS_PER_SEC: u64 = 515_616;
/// GIC priority, above USB so gate timing is not delayed behind USB servicing.
const GATE_TIMER_PRIORITY: u8 = 5;

/// The MIDI/gate one-shot (MTU2 channel 2, the C firmware's `TIMER_MIDI_GATE_OUTPUT`), once
/// [`gate_timer_setup`] has run.
static GATE_TIMER: OnceLock<OneShot<'static>> = OnceLock::new();

/// MTU2 channel 2's compare match: clears and disarms the timer, then runs the app handler
/// (which arms again if anything is still to come).
struct GateTimerHandler;

impl Handler<typelevel::TGI2A> for GateTimerHandler {
    unsafe fn on_interrupt() {
        // Filled before the source can be unmasked: only `arm` unmasks it.
        if let Some(timer) = GATE_TIMER.try_get() {
            // Clearing before disarming keeps the GIC line from re-triggering.
            timer.clear_flag();
            timer.disarm();
        }
        // SAFETY: an app-provided C symbol that `libdeluge/signals.h` documents as an ISR callback.
        unsafe { crate::sys::midiAndGateTimerGoneOff() };
    }
}

rza1l_hal::bind_interrupts!(struct GateTimerIrqs {
    TGI2A => GateTimerHandler;
});

/// One-time setup of the gate timer on the board's MTU2 channel 2, stopped until armed.
pub(crate) fn gate_timer_setup(timer: Peri<'static, GateTimer>) {
    let timer = OneShot::new(timer, GATE_TIMER_PRESCALER, GATE_TIMER_PRIORITY, GateTimerIrqs);
    if GATE_TIMER.init(timer).is_err() {
        unreachable!("MTU2 channel 2 is owned once");
    }
}
```

In `deluge_midi_gate_timer_arm_at`, replace the comment and `unsafe { mtu2::arm(GATE_TIMER_CH, ticks) };` with:

```rust
        // `arm` stops the channel and clears its flag before loading the new compare value, so it
        // replaces an armed deadline. Before setup there is no timer to arm.
        if let Some(timer) = GATE_TIMER.try_get() {
            timer.arm(ticks);
        }
```

In `deluge_midi_gate_timer_cancel`, replace the comment and the `unsafe { mtu2::disarm(..); mtu2::clear_flag(..); }` block with:

```rust
        if let Some(timer) = GATE_TIMER.try_get() {
            timer.disarm();
            timer.clear_flag();
        }
```

- [ ] **Step 5: The fault and freeze paths**

In `src/peripherals/fault.rs` `deluge_fault_pad_flush`:
- Delete `let channel = deluge_bsp::system::PIC_DMA_TX_CH;`.
- Inside the `unsafe` block of `critical_section::with`, before `let mut spins = 0u32;`, add `let mut tx_dma = rza1l_hal::dmac::Dma::steal::<deluge_bsp::board::PicTxDma>();`. The SAFETY comment above it already covers it: interrupts are masked, so nothing else drives the PIC's TX DMA channel.
- `rza1l_hal::dmac::transfer_in_flight(channel)` becomes `tx_dma.transfer_in_flight()`, and `rza1l_hal::dmac::stop(channel);` becomes `tx_dma.stop();`.
- `let wrote = rza1l_hal::uart::try_write_fifo(PIC_CH, &STAGE[sent..staged]);` becomes `let wrote = deluge_bsp::uart::try_write_pic(&STAGE[sent..staged]);`.
- Delete `use deluge_bsp::uart::PIC_CH;` if the compiler reports it unused.

In `src/peripherals/display.rs` `deluge_display_freeze`, replace the device block's body (from `let mut next_is_release = false;` through the end of its `loop`) with:

```rust
        // SAFETY: a freeze runs with the executor blocked in this call, or from a fault with
        // interrupts masked, so the PIC task (suspended mid-read, holding its receiver) can't read
        // the ring meanwhile. The stolen receiver shares its read position, so the bytes read
        // here, the resume press included, don't reach the app afterwards.
        let mut pic_rx = unsafe { deluge_bsp::uart::steal_pic_rx() };
        let mut next_is_release = false;
        loop {
            if let Some(byte) = pic_rx.as_mut().and_then(|rx| rx.try_read()) {
                if byte == PIC_NEXT_PAD_OFF {
                    next_is_release = true;
                    continue;
                }
                if byte == PIC_SELECT_KNOB_PRESS && !next_is_release {
                    return;
                }
                next_is_release = false;
            }
            core::hint::spin_loop();
        }
```

- [ ] **Step 6: `init_clocks` wording**

- `src/boot_mem.rs:4`: `Runs after `init_clocks` (SDRAM up)` becomes `Runs after `Board::new` (SDRAM up)`.
- `linker/sdram_sections.x:11`: `after init_clocks` becomes `after Board::new`.
- `docs/dev/image_size.md:224`: `after `init_clocks()` turns the MMU on` becomes `after `Board::new` turns the MMU on`.

Run: `rg -n "init_clocks|enable_write|try_read_dma|try_write_fifo|setup_one_shot|GATE_TIMER_CH" /home/kate/GitHub/DelugeFirmware/src /home/kate/GitHub/DelugeFirmware/docs`
Expected: no output.

- [ ] **Step 7: Check, test, build**

Run the device check from Global Constraints: `... cargo check ... --config local-sdk.toml 2>&1 | grep -E "^(error|warning: unused)" ; echo done`
Expected: no `error` lines and no new `unused` warnings in `src/bsp/rust/src`, then `done`.

Run: `cargo test --config local-sdk.toml > /tmp/claude-1000/p4c1-host.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/p4c1-host.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `exit 0`, `passed 221 failed 0`.

Run: `cd /home/kate/GitHub/DelugeFirmware && ./dbt build > /tmp/claude-1000/p4c1-dbt.log 2>&1; echo "exit $?"; tail -5 /tmp/claude-1000/p4c1-dbt.log`
Expected: `exit 0`, and the image is linked.

- [ ] **Step 8: Restore the lock and commit**

```bash
cd /home/kate/GitHub/DelugeFirmware && git checkout src/bsp/rust/Cargo.lock && git status --short
git add src/bsp/rust/src docs/dev/image_size.md src/bsp/rust/linker/sdram_sections.x
git commit -m "build(bsp): build on deluge-sdk's peripheral ownership: Board::new, the BSP's owned UART, bus and timer inits, and a fault-safe PIC read"
```

`git status --short` before the add must show only the files this task changed, with no `Cargo.lock`.

- [ ] **Step 9: Stop and report**

Task 3 needs the human partner to merge `feat/pic-rx-steal` into deluge-sdk `main` and push it. Report the hardware smoke tests (end of plan) and stop.

---

### Task 3: Pin DelugeFirmware at the pushed SDK revision (DelugeFirmware; after the human partner pushes)

**Precondition:** deluge-sdk `main` contains Task 1, and `git -C /home/kate/GitHub/deluge-sdk status -sb` shows `main` level with `origin/main`. If not, stop and ask.

**Files:**
- Modify: `src/bsp/rust/Cargo.toml` (six `rev = "28bc21ce…"` entries), `src/bsp/rust/Cargo.lock`

- [ ] **Step 1: Bump the revision**

```bash
cd /home/kate/GitHub/DelugeFirmware/src/bsp/rust
NEW=$(git -C /home/kate/GitHub/deluge-sdk rev-parse origin/main); echo $NEW
sed -i "s/28bc21cebe6d473a5e23a45e31c09f028c740675/$NEW/g" Cargo.toml
grep -c "$NEW" Cargo.toml          # expect: 6
```

- [ ] **Step 2: Build exactly what everyone else gets**

```bash
mv local-sdk.toml /tmp/claude-1000/local-sdk.toml.aside
cargo update -p deluge-bsp 2>&1 | tail -3
cargo test --locked > /tmp/claude-1000/p4c1-locked.log 2>&1; echo "exit $?"
grep -E "test result" /tmp/claude-1000/p4c1-locked.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'
cd /home/kate/GitHub/DelugeFirmware && ./dbt build > /tmp/claude-1000/p4c1-dbt-pinned.log 2>&1; echo "exit $?"
mv /tmp/claude-1000/local-sdk.toml.aside src/bsp/rust/local-sdk.toml
```

Expected:
- `cargo update` moves all six git dependencies to `$NEW`, because they share one source.
- The host tests report `exit 0`, `passed 221 failed 0`.
- `./dbt build` reports `exit 0`.

Run: `git -C /home/kate/GitHub/DelugeFirmware diff --stat`
Expected: only `Cargo.toml` and `Cargo.lock`. The lock must name `git+https://github.com/FirestormAudio/deluge-embassy.git?rev=$NEW`, never a path.

- [ ] **Step 3: Commit**

```bash
cd /home/kate/GitHub/DelugeFirmware && git add src/bsp/rust/Cargo.toml src/bsp/rust/Cargo.lock
git commit -m "build(bsp): pin deluge-sdk at ${NEW:0:8}, the revision with peripheral ownership"
```

---

## Hardware smoke tests (the human partner's merge gate)

These need a Deluge running the DelugeFirmware image from `./dbt build`. None of deluge-sdk Phases 1–4b has been run on hardware yet, so the first of these is the first real boot on the new SDK.

1. **Boot.** The image boots and the SYNC LED blinks six times. The OLED shows the app's UI, and boot doesn't panic with "cv_gate::init found RSPI0 busy" (Review Focus 2 and 5).
2. **Panel.** Pads, buttons and LEDs work through the 200 000 bps PIC handshake. Encoders turn, including a slow turn that stops on a B transition.
3. **Timed output.** MIDI DIN in and out work, and CV and gate outputs follow notes. Note lengths on the gate outputs are right, which exercises the gate timer on `OneShot` (Review Focus 3).
4. **Freeze and resume.** Trigger a `FREEZE_WITH_ERROR`; the select-knob press resumes it. Then confirm the press did not also act in the app (Review Focus 1).
5. **Fault report.** A CPU fault draws its pattern on the pads (`try_write_pic`, `Dma::steal` on the PIC TX channel).
6. **Unchanged paths.** The SD card mounts and samples load; USB MIDI enumerates. Neither changes in this plan.
