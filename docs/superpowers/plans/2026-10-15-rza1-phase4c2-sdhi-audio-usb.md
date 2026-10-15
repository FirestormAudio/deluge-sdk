# RZ/A1 Phase 4c-2: SDHI, Audio, USB, and the End of `raw` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish Phase 4. SDHI, SSI and SCUX, and USB are built from their singletons and typed bindings. The GPIO IRQs and SGIs bind through `bind_interrupts!`. `dmac::raw` and `gic::register` leave the public API, and no driver keeps a public `unsafe fn new()`. DelugeFirmware follows.

**Architecture:**
- **DMA.** `Dma` gains a constructor that leaves the completion interrupt masked, for polled rings. A `BlockDma` type with a typed `BlockInterruptHandler` serves the per-block ring that `audio-irq` waits on.
- **Drivers hold their channels.** Every HAL driver keeps the `Dma` (or `BlockDma`) handles it was given and drives them through their methods. SDHI keeps them behind async mutexes, SSI and SCUX in statics for their pointer accessors. `dmac::raw` becomes private to `dmac`: nothing outside it programs a channel by number.
- **SDHI.** `Sdhi<'d>` has `&self` methods, so the BSP keeps it in a `OnceLock`. `deluge_bsp::sd::init_hw` brings the controller up once; `sd::init` re-runs only the card protocol.
- **SSI and SCUX.** The init functions take the SSIF0 singleton and typed DMA channels instead of channel numbers.
- **USB.** `init_device_mode`/`init_host_mode` take the port singleton and a binding to one of three handler types: device, host, or dual-role. The dual-role handler dispatches on a mode the HAL records.
- **GPIO IRQs and SGIs.** The encoders' IRQ0–4/7, the trigger clock's IRQ6, and DelugeFirmware's SGI 8/9 get typelevel names and typed handlers.
- **Board and parts.** `Board` and `deluge::parts` gain the SD card, the audio SSI, and USB0 when the SDK doesn't own it.

**Tech Stack:** Rust nightly, `embassy-hal-internal` 0.5, `embassy-sync` 0.8 (`OnceLock`, `Mutex`), `embassy-usb` 0.6 / `embassy-usb-host`, the HAL's MMIO test seam, QEMU ARM host tests, `armv7a-none-eabihf` firmware builds, DelugeFirmware's `./dbt build`.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md`, Phase 4. Its exit: "no public `unsafe fn new()` constructors; enabling both `usb-log` and `usb-serial` fails to compile instead of panicking." This plan completes the first half; 4a did the second.

## Global Constraints

- **Layering (spec §2):**
  - `rza1l-hal` never names `deluge` in code or depends on a `deluge-*` crate.
  - `wren/` and `examples/baremetal/` reach the platform only through the `deluge` facade.
  - `usb/` and `drivers/` stay board-agnostic.
  - `deluge-bsp` never depends on the simulator.
  - `tools/check-layering.sh` enforces these rules (it runs first in `tools/test.sh`).
- **Spec on migrations:** "Old `unsafe` free functions move to `#[doc(hidden)] pub mod raw` during migration, then go."
- **No memory-layout change:** `tools/link-layout.sh check` passes after every task.
- **deluge-sdk checks:**
  - HAL tests: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib`.
  - BSP tests: add `-p deluge-bsp --features usb-host`.
  - Suite: `tools/test.sh`.
  - zsh does not word-split a variable holding a command, so write commands out.
- **Firmware build check**, run at the end of every deluge-sdk task:
  ```bash
  for p in app-loader deluge-controller demo-firmware msc-firmware wp-probe vbus-probe sd-bench uac-host-firmware wren-firmware; do printf '%s: ' $p; cargo build-fw -p $p 2>&1 | tail -1; done
  tools/build-examples.sh > /tmp/claude-1000/p4c2-examples.log 2>&1; echo "examples exit $?"
  ```
  Expected: nine `Finished` lines and `examples exit 0`, with no new warnings in files the task touched.
- **Repositories and branches:**
  - deluge-sdk at `/home/kate/GitHub/deluge-sdk`: Tasks 1–6 on branch `refactor/rza1-ownership-4c2`, cut from `main` (`07ea8237`, pushed).
  - DelugeFirmware at `/home/kate/GitHub/DelugeFirmware`: Task 7 on branch `sdk-ownership-4c2`, cut from `next` (`043ee3db2`).
  - DelugeFirmware builds against this working copy through its gitignored `src/bsp/rust/local-sdk.toml`.
- **DelugeFirmware checks:**
  - Device: `cd /home/kate/GitHub/DelugeFirmware/src/bsp/rust && cargo check --target armv7a-deluge-eabihf.json -Zbuild-std=core,alloc -Zjson-target-spec --no-default-features --features rtt --config local-sdk.toml`
  - Host: `cargo test --config local-sdk.toml`. The baseline is 221 passed, 0 failed.
  - Image: `./dbt build` from the repo root.
  - **Run `git -C /home/kate/GitHub/DelugeFirmware checkout src/bsp/rust/Cargo.lock` before every DelugeFirmware commit.** The override rewrites the lock.
  - DelugeFirmware's pre-commit hook runs rustfmt and clang-format. If it rejects a commit, run `cargo fmt` in `src/bsp/rust`, re-check, and commit again.
- **Never push either repository.** Task 7's last step (the pin bump) runs only after the human partner has merged and pushed deluge-sdk. Until then the executor stops and reports.

## Decisions settled before execution

1. **`dmac::raw` becomes private to `dmac` (human partner's decision).** Every HAL driver holds the typed handles it was given and calls their methods; the channel-number functions survive only as `Dma`'s implementation. Where a driver needs a channel from several places:
   - **SDHI** keeps its two `Dma`s in async `Mutex`es: a block transfer holds one across its awaits, and the `&self` methods stay shareable.
   - **SSI** keeps its rings in `OnceLock` statics: its pointer accessors only need `&self` reads (CRSA/CRDA).
   - **SCUX** keeps its FIFO channels in a critical-section `Mutex<RefCell<[Option<Dma>; 4]>>`: `start` needs `&mut` for `channel_start`, briefly and with no await.
   - **`BlockDma`** wraps a `Dma` and derefs to it, so SSI programs either ring the same way.
2. **Polled rings never enable their completion interrupt.** The SSI TX/RX rings and the SCUX FFD rings run with DEM=0, so the DMAC raises DMAINT on every pass. Today that is harmless only because nothing enables those GIC lines. `Dma::new_without_interrupt` builds a channel with its line masked and nothing bound, and the SSI/SCUX/SDHI-TX init paths also call `mask_interrupt()` defensively. Building them with `Dma::new` would storm.
3. **SCUX gets no ownership token.** Its three paths share the block, each using its own FIFO and SRC units. The paths' typed DMA channels are their ownership proof, and the path `init` functions stay `unsafe` ("no other path claims the same SRC unit"). `peripherals::SCUX` stays unused.
4. **USB drivers borrow their port (human partner's decision).**
   - `Rusb1Driver<'d>` and `Rusb1HostDriver<'d>` carry the lifetime of the `Peri<'d, T>` that `init_*_mode` took, so a second init while the first driver lives fails to compile.
   - Firmware signatures become `Rusb1Driver<'static>`: about 50 type positions in 15 files, plus 5 in DelugeFirmware. That is mechanical, and the compiler flags each one with E0106/E0726.
   - app-loader brings USB0 up once per session. It steals `USB0` per session under its existing one-session-at-a-time contract; a stolen singleton is `'static`.
5. **One dual-role USB handler instead of firmware flags.** `DualRoleInterruptHandler<T>` dispatches on a per-port mode flag that `init_*_mode` sets. controller-firmware and demo-firmware drop their hand-written host/device closures. `USB0_HOST_MODE` stays only as their boot-time choice.
6. **SDHI's SDIO interrupt stays masked.** The old code bound it to an empty handler and enabled it. Only the status (card detect) and operation interrupts are bound and enabled now.
7. **SDHI hardware bring-up runs once.**
   - `sd::init_hw` muxes the pins, builds the `Sdhi`, and stores it. Its pin-mux moves out of `sd::init`, which re-runs on every card change.
   - `sd::init` keeps the bounce-buffer cache clean, the timers, and the protocol.
   - `deluge_bsp::sd::controller()` exposes the `Sdhi` for sd-bench's clock experiments.
8. **The unused `Sdhi<PORT>` ZST and SDHI's unused poll/IRQ functions** (`register_irqs`, `enable_card_detect_irq`, `*_poll`) become private or are deleted; nothing calls them.
9. **DelugeFirmware's audio init keeps its C++-called position** (`deluge_board_init_audio`, during `deluge_app_init`). `start()` stashes the SSI parts in a static for it, because moving the codec bring-up earlier would change its timing relative to the C++ GPIO setup.

## Review Focus

1. **Audio rings and DMAINT.** Building the SSI or SCUX rings must not enable their DMA completion interrupt (DEM=0, raised every pass): an interrupt storm silences audio and starves everything below priority 13. Pinned by `ssi_rings_keep_their_completion_interrupts_masked` (Task 4).
2. **The `audio-irq` block ring** must bind and enable the block handler before the ring starts, and each block must wake `wait_rx_block` exactly once. Pinned by `a_block_interrupt_clears_tc_and_wakes_the_waiter_once` (Task 1) and `the_block_ring_binds_its_interrupt_before_starting` (Task 4).
3. **SD card re-identification** after a card swap must keep working: `sd::init` re-runs the protocol on the `Sdhi` that `init_hw` stored, without re-binding or re-muxing. That is structural (Task 3 removes those calls from `init`), and checked by hardware smoke test 4.
4. **USB dual-role dispatch** must route each interrupt to the handler of the mode the port is in. Pinned by `dual_role_dispatch_follows_the_recorded_mode` (Task 5).
5. **app-loader's repeated USB sessions** (MSC, then CDC dev-upload, then MSC again) must each enumerate. That is structural (Task 5 steals per session and re-binds idempotently), and checked by hardware smoke test 6.

---

## File Structure

| Path | Change |
|---|---|
| `crates/rza1l-hal/src/dmac.rs`, `dmac/raw.rs` | `Dma::new_without_interrupt`, `BlockDma` (derefs to `Dma`), `BlockInterruptHandler`; `raw` becomes private to `dmac` (Task 6) |
| `crates/rza1l-hal/src/interrupt.rs` | Typelevel `IRQ0`–`IRQ7`, `SDHI0_STATUS`/`OP`/`SDIO`, `SDHI1_*` |
| `crates/rza1l-hal/src/gic.rs` | `register` deleted, `set_handler` `pub(crate)` (Task 6) |
| `crates/rza1l-hal/src/sdhi.rs` | MMIO seam; `Instance`, two handler types, `Sdhi<'d>`; free functions private |
| `crates/rza1l-hal/src/ssi.rs` | Init functions take `Peri<SSIF0>` and typed DMA; `SsiConfig` goes; `wait_rx_block` |
| `crates/rza1l-hal/src/scux.rs`, `scux/*.rs` | FIFO DMA and path inits take typed `Dma` |
| `crates/rza1l-hal/src/usb/{mod,driver,host}.rs` | `Instance`, three handler types, safe `init_*_mode`, mode flag; `new` → `pub(crate)` |
| `crates/deluge-bsp/src/{encoder,trigger_clock}.rs` | Typed GPIO-IRQ handlers |
| `crates/deluge-bsp/src/sd.rs` | `init_hw`, `controller()`; `init` protocol-only |
| `crates/deluge-bsp/src/{audio,audio_block,scux_*_path,system,board}.rs` | Parts-taking audio init; Board `sd_card`, `audio_ssi` |
| `crates/deluge-sdk/src/{parts,lib,usb0,usb_debug,usb_serial,plat/device}.rs` | New parts; SD/audio/USB capabilities take them |
| `firmwares/*`, `hw-tests/*` | Pass the board's parts; typed USB bindings |
| DelugeFirmware `src/bsp/rust/src/{runtime/device.rs, peripherals/{usb,board}.rs}`, `Cargo.toml` | SGI/USB0 bindings, `sd::init_hw`, audio parts; pin bump |

---

### Task 1: Masked DMA channels, the typed block ring, and the interrupt names 4c-2 needs

**Files:** Modify `crates/rza1l-hal/src/dmac.rs`, `crates/rza1l-hal/src/interrupt.rs`.

**Interfaces:**
- Produces:
  - `pub fn Dma::new_without_interrupt<C: Channel>(Peri<'d, C>) -> Dma<'d>`
  - `pub struct BlockInterruptHandler<C: Channel>`, which implements `Handler<C::Interrupt>`
  - `pub struct BlockDma<'d>`, with `pub fn new<C: Channel>(Peri<'d, C>, irq: impl Binding<C::Interrupt, BlockInterruptHandler<C>> + 'd) -> Self` and `pub async fn wait_block(&self)`. It implements `Deref<Target = Dma<'d>>` and `DerefMut`, so every `Dma` method (`number`, `init_with_link_descriptor`, `channel_start`, `current_dst`, …) works on it.
  - `interrupt::typelevel::{IRQ0..IRQ7 (32..39), SDHI0_STATUS = 302, SDHI0_OP = 303, SDHI0_SDIO = 304, SDHI1_STATUS = 305, SDHI1_OP = 306, SDHI1_SDIO = 307}`

- [ ] **Step 1: Branch**

```bash
cd /home/kate/GitHub/deluge-sdk && git checkout -b refactor/rza1-ownership-4c2 main
```

- [ ] **Step 2: Write the failing tests**

Append to `dmac.rs` `mod typed_tests` (add `DMA_CH3, DMA_CH5` to its peripherals import):

```rust
    crate::bind_interrupts!(struct BlockIrqs {
        DMAINT5 => BlockInterruptHandler<DMA_CH5>;
    });

    #[test]
    fn a_channel_without_interrupt_masks_its_line_and_binds_nothing() {
        test::reset();
        let dma = Dma::new_without_interrupt(unsafe { DMA_CH3::steal() });
        assert_eq!(dma.number(), 3);
        // GICD_ICENABLER1 (0xE820_1184), bit 44 - 32 = 12; nothing set-enabled, no handler.
        assert_eq!(test::writes(), [(0xE820_1184, 1 << 12)]);
        assert!(crate::gic::handler(44).is_none());
    }

    #[test]
    fn a_block_interrupt_clears_tc_and_wakes_the_waiter_once() {
        test::reset();
        let ring = BlockDma::new(unsafe { DMA_CH5::steal() }, BlockIrqs);
        assert_eq!(ring.number(), 5, "derefs to its Dma");
        assert!(crate::gic::handler(46).is_some(), "DMAINT5 bound");
        // Priority 13 into IPRIORITYR11 lane 2, then ISENABLER1 bit 14.
        assert_eq!(test::writes(), [(0xE820_142C, (13 << 3) << 16), (0xE820_1104, 1 << 14)]);

        test::reset();
        unsafe { <BlockInterruptHandler<DMA_CH5> as Handler<_>>::on_interrupt() };
        // CHCTRL (channel 5 at 0xE820_0140 + 0x28) gets CLRTC only: the ring keeps running.
        assert_eq!(test::writes(), [(0xE820_0168, 1 << 6)]);
        embassy_futures::block_on(ring.wait_block());
        // Consumed: a second wait would pend. Poll it once to show it isn't ready.
        let mut second = core::pin::pin!(ring.wait_block());
        let waker = Waker::from(Arc::new(CountWakes(AtomicU32::new(0))));
        let mut cx = core::task::Context::from_waker(&waker);
        assert!(second.as_mut().poll(&mut cx).is_pending());
    }
```

Add `use core::future::Future;` to that module's imports.

Append to `interrupt.rs` `mod tests`:

```rust
    #[test]
    fn the_4c_interrupt_ids_match_the_hardware_manual() {
        // IRQn (external pin interrupts) are GIC 32 + n.
        assert_eq!(
            [
                typelevel::IRQ0::ID, typelevel::IRQ1::ID, typelevel::IRQ2::ID, typelevel::IRQ3::ID,
                typelevel::IRQ4::ID, typelevel::IRQ5::ID, typelevel::IRQ6::ID, typelevel::IRQ7::ID
            ],
            [32, 33, 34, 35, 36, 37, 38, 39]
        );
        // SDHIn: status (card detect), operation, SDIO.
        assert_eq!(
            [typelevel::SDHI0_STATUS::ID, typelevel::SDHI0_OP::ID, typelevel::SDHI0_SDIO::ID],
            [302, 303, 304]
        );
        assert_eq!(
            [typelevel::SDHI1_STATUS::ID, typelevel::SDHI1_OP::ID, typelevel::SDHI1_SDIO::ID],
            [305, 306, 307]
        );
    }
```

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib dmac interrupt 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: compile errors: no `new_without_interrupt`, `BlockDma`, `BlockInterruptHandler`, `IRQ0`, `SDHI0_STATUS`.

- [ ] **Step 4: Implement**

In `interrupt.rs`'s `interrupts!`, after the SGI line add:

```rust
        // External pin interrupts IRQ0–IRQ7.
        IRQ0 = 32, IRQ1 = 33, IRQ2 = 34, IRQ3 = 35, IRQ4 = 36, IRQ5 = 37, IRQ6 = 38, IRQ7 = 39,
```

and after the SCIF lines:

```rust
        // SDHI channel n's status (card detect), operation and SDIO interrupts.
        SDHI0_STATUS = 302, SDHI0_OP = 303, SDHI0_SDIO = 304,
        SDHI1_STATUS = 305, SDHI1_OP = 306, SDHI1_SDIO = 307,
```

In `dmac.rs`, add to `impl<'d> Dma<'d>` after `new`:

```rust
    /// Take channel `C` for a transfer nobody awaits (a polled ring): its completion interrupt is
    /// masked at the GIC and nothing is bound to it. A ring with DEM clear raises DMAINT on every
    /// pass, so an enabled line with no one clearing TC would storm.
    pub fn new_without_interrupt<C: Channel>(_channel: Peri<'d, C>) -> Self {
        let mut dma = Self { ch: C::NUMBER, _channel: PhantomData };
        dma.mask_interrupt();
        dma
    }
```

After `stop_all`, add:

```rust
/// Channel `C`'s per-block interrupt, for a descriptor ring whose descriptors each end with an
/// interrupt: clears TC without stopping the ring and wakes [`BlockDma::wait_block`].
pub struct BlockInterruptHandler<C: Channel> {
    _channel: PhantomData<C>,
}

impl<C: Channel> Handler<C::Interrupt> for BlockInterruptHandler<C> {
    unsafe fn on_interrupt() {
        on_block_int(C::NUMBER);
    }
}

/// An owned DMAC channel running a descriptor ring with a per-block interrupt. It derefs to
/// its [`Dma`], which programs the ring.
pub struct BlockDma<'d> {
    dma: Dma<'d>,
}

impl<'d> BlockDma<'d> {
    /// Take channel `C`, install its block handler, and enable its interrupt: before the ring
    /// starts, so no block's interrupt is lost.
    pub fn new<C: Channel>(
        _channel: Peri<'d, C>,
        irq: impl Binding<C::Interrupt, BlockInterruptHandler<C>> + 'd,
    ) -> Self {
        interrupt::bind(irq);
        // SAFETY: this channel's own interrupt, at the DMAC priority.
        unsafe {
            crate::gic::set_priority(C::Interrupt::ID, DMAC_IRQ_PRIORITY);
            crate::gic::enable(C::Interrupt::ID);
        }
        Self { dma: Dma { ch: C::NUMBER, _channel: PhantomData } }
    }

    /// Wait for the next block to complete. Recurring: call it in a loop. Takes `&self`, so a
    /// ring kept in a static can be waited on.
    pub async fn wait_block(&self) {
        raw::wait_block(self.dma.ch).await
    }
}

impl<'d> core::ops::Deref for BlockDma<'d> {
    type Target = Dma<'d>;
    fn deref(&self) -> &Dma<'d> {
        &self.dma
    }
}

impl<'d> core::ops::DerefMut for BlockDma<'d> {
    fn deref_mut(&mut self) -> &mut Dma<'d> {
        &mut self.dma
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED"`
Expected: `ok`.

- [ ] **Step 6: Build check and commit**

Run the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/dmac.rs crates/rza1l-hal/src/interrupt.rs
git commit -m "feat(rza1l-hal): masked DMA channels for polled rings, a typed block-interrupt ring, and the IRQ0–7 and SDHI interrupt types"
```

---

### Task 2: The encoders and the trigger clock bind typed GPIO-IRQ handlers

**Files:** Modify `crates/deluge-bsp/src/encoder.rs`, `crates/deluge-bsp/src/trigger_clock.rs`.

**Interfaces:**
- Consumes: Task 1's `typelevel::{IRQ0..IRQ7}`.
- Produces: no API change (`encoder::irq_init(poll_timer)` and `trigger_clock::irq_init()` keep their signatures). Neither file calls `gic::register` any more.

- [ ] **Step 1: The failing check**

These are device-only code paths. The checks are compile-time: a `WIRING` entry whose GIC ID disagrees with its bound interrupt must fail to build. In `encoder.rs`, after `WIRING`, add (device-only, like `WIRING`'s users):

```rust
// Each encoder's A pin is bound to the typed interrupt below; its GIC ID must agree.
#[cfg(target_os = "none")]
const _: () = {
    use rza1l_hal::interrupt::{Interrupt, typelevel::*};
    assert!(WIRING[0].gic_id == IRQ3::ID && WIRING[1].gic_id == IRQ2::ID);
    assert!(WIRING[2].gic_id == IRQ4::ID && WIRING[3].gic_id == IRQ1::ID);
    assert!(WIRING[4].gic_id == IRQ0::ID && WIRING[5].gic_id == IRQ7::ID);
};
```

Run: `cargo build-fw -p deluge-controller 2>&1 | grep -E "^error" | head -3`
Expected: none. The constants already agree, so this check pins them rather than failing first. The red step is Step 3's compile failure when the binding names a handler that doesn't exist yet.

- [ ] **Step 2: Encoder handlers**

In `encoder.rs`, replace the six `rza1l_hal::gic::register(WIRING[i].gic_id, || enc_irq_handler(i));` lines with:

```rust
        use rza1l_hal::interrupt::{self, typelevel::*};
        interrupt::bind::<IRQ3, EncoderHandler<0>>(EncoderIrqs);
        interrupt::bind::<IRQ2, EncoderHandler<1>>(EncoderIrqs);
        interrupt::bind::<IRQ4, EncoderHandler<2>>(EncoderIrqs);
        interrupt::bind::<IRQ1, EncoderHandler<3>>(EncoderIrqs);
        interrupt::bind::<IRQ0, EncoderHandler<4>>(EncoderIrqs);
        interrupt::bind::<IRQ7, EncoderHandler<5>>(EncoderIrqs);
```

and above `irq_init` (device-only) add:

```rust
/// Encoder `I`'s A-pin interrupt.
#[cfg(target_os = "none")]
struct EncoderHandler<const I: usize>;

#[cfg(target_os = "none")]
macro_rules! encoder_handlers {
    ($($i:literal => $irq:ident),* $(,)?) => {$(
        impl rza1l_hal::interrupt::Handler<rza1l_hal::interrupt::typelevel::$irq>
            for EncoderHandler<$i>
        {
            unsafe fn on_interrupt() {
                enc_irq_handler($i);
            }
        }
    )*};
}

#[cfg(target_os = "none")]
encoder_handlers!(0 => IRQ3, 1 => IRQ2, 2 => IRQ4, 3 => IRQ1, 4 => IRQ0, 5 => IRQ7);

#[cfg(target_os = "none")]
rza1l_hal::bind_interrupts!(struct EncoderIrqs {
    IRQ3 => EncoderHandler<0>;
    IRQ2 => EncoderHandler<1>;
    IRQ4 => EncoderHandler<2>;
    IRQ1 => EncoderHandler<3>;
    IRQ0 => EncoderHandler<4>;
    IRQ7 => EncoderHandler<5>;
});
```

The priority/enable loop over `WIRING` stays.

- [ ] **Step 3: Trigger-clock handler**

In `trigger_clock.rs`, replace `const TRIG_GIC_ID: u16 = 38;` (and its doc) with nothing, and use `IRQ6::ID` where it was used. Add:

```rust
/// P1_14's IRQ6: one trigger-clock edge.
#[cfg(target_os = "none")]
struct TriggerHandler;

#[cfg(target_os = "none")]
impl rza1l_hal::interrupt::Handler<rza1l_hal::interrupt::typelevel::IRQ6> for TriggerHandler {
    unsafe fn on_interrupt() {
        trig_irq_handler();
    }
}

#[cfg(target_os = "none")]
rza1l_hal::bind_interrupts!(struct TriggerIrqs {
    IRQ6 => TriggerHandler;
});
```

In `irq_init`, replace `rza1l_hal::gic::register(TRIG_GIC_ID, trig_irq_handler);` with `rza1l_hal::interrupt::bind(TriggerIrqs);`. Replace `TRIG_GIC_ID` in the `set_priority`/`enable` calls with `<rza1l_hal::interrupt::typelevel::IRQ6 as rza1l_hal::interrupt::Interrupt>::ID`. In the `info!` text, "GIC 38" stays.

Run: `rg -n "gic::register" crates/deluge-bsp/src`
Expected: no output.

- [ ] **Step 4: Tests, build check, commit**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib 2>&1 | grep -E "test result|FAILED"` → `ok`. Then run the **Firmware build check** and the link-layout check.

```bash
git add crates/deluge-bsp/src/encoder.rs crates/deluge-bsp/src/trigger_clock.rs
git commit -m "feat(bsp): the encoders and the trigger clock bind typed IRQ0–7 handlers"
```

---

### Task 3: SDHI is an owned driver, and the SD card's hardware comes up once

**Files:**
- Modify: `crates/rza1l-hal/src/sdhi.rs`
- Modify: `crates/deluge-bsp/src/{sd.rs,board.rs}`
- Modify: `crates/deluge-sdk/src/{parts.rs,lib.rs,plat/device.rs}`
- Modify: `firmwares/{app-loader,controller-firmware,msc-firmware}/src/main.rs`, `firmwares/demo-firmware/src/main.rs`, `hw-tests/{wp-probe,sd-bench}/src/{main.rs,tasks/*.rs}`

**Interfaces:**
- Consumes: Task 1's `Dma::new_without_interrupt` and `typelevel::SDHI1_STATUS`/`SDHI1_OP`.
- Produces:
  - `rza1l_hal::sdhi::{Instance, StatusInterruptHandler<T>, OperationInterruptHandler<T>, Sdhi}`, with `Sdhi::new<T: Instance>(Peri<'d, T>, tx_dma: Dma<'d>, rx_dma: Dma<'d>, irq: impl Binding<T::StatusInterrupt, StatusInterruptHandler<T>> + Binding<T::OperationInterrupt, OperationInterruptHandler<T>> + Copy + 'd, sd_option: u16) -> Sdhi<'d>`.
  - `&self` methods:
    - safe: `set_clock_div(u16)`, `set_clock_fast()`, `set_arg(u32)`, `async send_cmd(u16) -> Result<(), SdhiError>`, `read_r1() -> u32`, `read_r2() -> [u32; 4]`, `set_block_count(u32)`, `async stop_transfer() -> Result<(), SdhiError>`, `card_inserted() -> bool`, `card_write_protected() -> bool`, `take_card_detect_events() -> (bool, bool)`, `set_card_blocks(u32)`, `card_size_blocks() -> u32`, `async read_status_block_sw(u16, u32, &mut [u8; 64]) -> Result<(), SdhiError>`;
    - unsafe (raw buffers): `async read_blocks_sw(*mut u8, u32)`, `async write_blocks_sw(*const u8, u32)`, `async read_blocks_dma(u16, *mut u8, u32)`, `async write_blocks_dma(u16, *const u8, u32)`.
  - `deluge_bsp::sd::{init_hw(Peri<'static, SdCard>, Peri<'static, SdTxDma>, Peri<'static, SdRxDma>), controller() -> &'static Sdhi<'static>}`.
  - `Board.sd_card: Peri<'static, SdCard>` with `pub type SdCard = SDHI1`.
  - `Parts.{sd_card, sd_tx_dma, sd_rx_dma}`.

- [ ] **Step 1: SDHI registers onto the MMIO seam**

In `sdhi.rs`, change the helpers to:

```rust
#[inline]
fn reg16(base: usize, off: usize) -> crate::mmio::Reg16 {
    crate::mmio::Reg16(base + off)
}

#[inline]
fn reg32(base: usize, off: usize) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(base + off)
}
```

In `init`, `let stbcr12 = STBCR12 as *mut u8;` becomes `let stbcr12 = crate::mmio::Reg8(STBCR12);`. Then:

```bash
sed -i -E 's/\.write_volatile\(/.write(/g; s/\.read_volatile\(\)/.read()/g' crates/rza1l-hal/src/sdhi.rs
```

Every volatile access in `sdhi.rs` is on a register handle: the FIFO accesses are on `reg32(base, OFF_BUF0)`, and `p.read_unaligned()` is a plain read, so it doesn't match. Run `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib sdhi 2>&1 | grep -E "^error|test result"` → `ok` (the 6 existing tests).

- [ ] **Step 2: Write the failing tests**

Append to `sdhi.rs`:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod typed_tests {
    use super::*;
    use crate::dmac::{self, Dma};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH11, DMA_CH14, SDHI1};

    crate::bind_interrupts!(struct Irqs {
        SDHI1_STATUS => StatusInterruptHandler<SDHI1>;
        SDHI1_OP => OperationInterruptHandler<SDHI1>;
        DMAINT14 => dmac::InterruptHandler<DMA_CH14>;
    });

    fn sdhi1() -> Sdhi<'static> {
        let tx = Dma::new_without_interrupt(unsafe { DMA_CH11::steal() });
        let rx = Dma::new(unsafe { DMA_CH14::steal() }, Irqs);
        Sdhi::new(unsafe { SDHI1::steal() }, tx, rx, Irqs, 0x00BD)
    }

    #[test]
    fn a_controller_comes_up_clocked_reset_and_bound_with_sdio_masked() {
        test::reset();
        test::poke8(STBCR12, 0xFF);
        let _sdhi = sdhi1();
        // SDHI1's clocks (STBCR12 bits 1:0) run; SDHI0's stay stopped.
        assert_eq!(test::peek8(STBCR12), 0xFF & !STBCR12_SDHI1_MASK);
        assert_eq!(test::peek16(SDHI1_BASE + OFF_OPTION), 0x00BD);
        assert_eq!(test::peek16(SDHI1_BASE + OFF_SIZE), 512);
        assert!(crate::gic::handler(305).is_some(), "status bound");
        assert!(crate::gic::handler(306).is_some(), "operation bound");
        // SDIO (307) is neither bound nor enabled: GICD_ISENABLER9 (0xE820_1124) bit 307-288=19.
        assert!(crate::gic::handler(307).is_none());
        assert_eq!(test::peek32(0xE820_1124) & (1 << 19), 0);
    }

    #[test]
    fn card_detect_reads_info1_of_its_own_port() {
        test::reset();
        let sdhi = sdhi1();
        test::poke16(SDHI1_BASE + OFF_INFO1, INFO1_INS_CD);
        assert!(sdhi.card_inserted());
        test::poke16(SDHI1_BASE + OFF_INFO1, 0);
        assert!(!sdhi.card_inserted());
    }
}
```

Check `card_inserted`'s current body (`:1247`) for the exact INFO1 bit it tests (`INFO1_INS_CD` or `INFO1_CD_LEVEL`) and use that constant in the test.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib sdhi 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: compile errors: no `Sdhi::new` taking a peripheral, no `StatusInterruptHandler`.

- [ ] **Step 3: The owned `Sdhi`**

In `sdhi.rs`:

1. **Make private** (drop `pub`): `init`, `set_clock_div`, `set_clock_fast`, `register_irqs`, `interrupt_handler`, `set_arg`, `wait_clk_stable`, `send_cmd`, `read_r1`, `read_r2`, `read_blocks_sw`, `write_blocks_sw`, `read_status_block_sw`, `read_blocks_dma`, `write_blocks_dma`, `set_block_count`, `stop_transfer`, `card_inserted`, `card_write_protected`, `take_card_detect_events`, `set_card_blocks`, `card_size_blocks`, `send_cmd_poll`, `read_blocks_poll`, `write_blocks_poll`.
2. **Delete** `register_irqs`, `enable_card_detect_irq`, the six `irq_sdhi*` functions, and the `Sdhi<const PORT: u8>` struct with its impl block (`:1553-1675`).
3. **DMA calls go through the `Dma`.** The private `read_blocks_dma`/`write_blocks_dma` take `dma: &mut crate::dmac::Dma<'_>` in place of `dma_ch: u8`. In their bodies:
   - `crate::dmac::raw::init_register_mode_rx(dma_ch, chcfg_sdhi_rx(dma_ch), sd_buf0, dmars_rx)` → `dma.init_register_mode_rx(chcfg_sdhi_rx(dma.number()), sd_buf0, dmars_rx)`
   - `crate::dmac::raw::start_transfer_rx(dma_ch, buf as u32, count * 512)` → `dma.start_transfer_rx(buf as u32, count * 512)`
   - `crate::dmac::raw::wait_transfer_complete(dma_ch).await` → `dma.wait_transfer_complete().await`
   - TX likewise, with `init_register_mode` and `start_transfer`.

   Update their doc comments' "register_completion_irq must be called" note to "`dma` must have its completion interrupt (built with `Dma::new`)". After this, `rg -n "dmac::raw" crates/rza1l-hal/src/sdhi.rs` prints nothing.
4. **Add**, before `#[cfg(all(test, …))] mod tests`:

```rust
// ── Owned driver ─────────────────────────────────────────────────────────────

use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use crate::Peri;
use crate::dmac::Dma;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// An SDHI channel singleton (`peripherals::SDHI0`, `SDHI1`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The port number, 0 or 1.
    const PORT: u8;
    /// Its status (card-detect) interrupt.
    type StatusInterrupt: Interrupt;
    /// Its operation (command and transfer) interrupt.
    type OperationInterrupt: Interrupt;
}

macro_rules! instances {
    ($($sdhi:ident = $n:literal, $status:ident, $op:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$sdhi {}
        impl Instance for crate::peripherals::$sdhi {
            const PORT: u8 = $n;
            type StatusInterrupt = crate::interrupt::typelevel::$status;
            type OperationInterrupt = crate::interrupt::typelevel::$op;
        }
    )*};
}

instances! {
    SDHI0 = 0, SDHI0_STATUS, SDHI0_OP;
    SDHI1 = 1, SDHI1_STATUS, SDHI1_OP;
}

/// SDHI `T`'s status interrupt (card insert/remove): latches the status and wakes the waiter.
pub struct StatusInterruptHandler<T: Instance> {
    _sdhi: PhantomData<T>,
}

impl<T: Instance> Handler<T::StatusInterrupt> for StatusInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: the GIC dispatches this for T's status interrupt only.
        unsafe { interrupt_handler(T::PORT) }
    }
}

/// SDHI `T`'s operation interrupt (response, transfer end, errors).
pub struct OperationInterruptHandler<T: Instance> {
    _sdhi: PhantomData<T>,
}

impl<T: Instance> Handler<T::OperationInterrupt> for OperationInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: as above, for T's operation interrupt.
        unsafe { interrupt_handler(T::PORT) }
    }
}

/// An SD host controller, with its DMA channels.
///
/// Its methods take `&self`, so firmware can keep it in a static. The card protocol is a
/// sequence: issue one command (and its transfer) at a time.
pub struct Sdhi<'d> {
    port: u8,
    /// The DMA channels, each held by one block transfer at a time across its awaits.
    tx_dma: Mutex<CriticalSectionRawMutex, Dma<'d>>,
    rx_dma: Mutex<CriticalSectionRawMutex, Dma<'d>>,
    _sdhi: PhantomData<&'d mut ()>,
}

impl<'d> Sdhi<'d> {
    /// Bring SDHI `T` up: module clock, soft reset, interrupt masks, `sd_option` (bus width and
    /// timeouts), 512-byte blocks, identification clock. Binds and enables its status and
    /// operation interrupts; SDIO stays masked.
    ///
    /// `rx_dma` must have its completion interrupt (a block read waits for its last burst);
    /// `tx_dma` needs none.
    pub fn new<T: Instance>(
        _sdhi: Peri<'d, T>,
        mut tx_dma: Dma<'d>,
        rx_dma: Dma<'d>,
        irq: impl Binding<T::StatusInterrupt, StatusInterruptHandler<T>>
        + Binding<T::OperationInterrupt, OperationInterruptHandler<T>>
        + Copy
        + 'd,
        sd_option: u16,
    ) -> Self {
        // A write never waits for its DMA completion (DATA_TRNS ends it), so its line stays off.
        tx_dma.mask_interrupt();
        // SAFETY: SDHI T is owned.
        unsafe { init(T::PORT, sd_option) };
        interrupt::bind::<T::StatusInterrupt, StatusInterruptHandler<T>>(irq);
        interrupt::bind::<T::OperationInterrupt, OperationInterruptHandler<T>>(irq);
        // SAFETY: T's own interrupts.
        unsafe {
            crate::gic::set_priority(T::StatusInterrupt::ID, SDHI_IRQ_PRIORITY);
            crate::gic::set_priority(T::OperationInterrupt::ID, SDHI_IRQ_PRIORITY);
            crate::gic::enable(T::StatusInterrupt::ID);
            crate::gic::enable(T::OperationInterrupt::ID);
        }
        Self {
            port: T::PORT,
            tx_dma: Mutex::new(tx_dma),
            rx_dma: Mutex::new(rx_dma),
            _sdhi: PhantomData,
        }
    }

    /// Set the SD clock divider (`CLK_DIV_*`). `CLK_DIV_2` needs a High-Speed card.
    pub fn set_clock_div(&self, div: u16) {
        // SAFETY: the owned controller.
        unsafe { set_clock_div(self.port, div) }
    }

    /// The fast (25 MHz) clock.
    pub fn set_clock_fast(&self) {
        // SAFETY: the owned controller.
        unsafe { set_clock_fast(self.port) }
    }

    /// The next command's argument.
    pub fn set_arg(&self, arg: u32) {
        // SAFETY: the owned controller.
        unsafe { set_arg(self.port, arg) }
    }

    /// Issue a command and wait for its response.
    pub async fn send_cmd(&self, cmd_val: u16) -> Result<(), SdhiError> {
        // SAFETY: the owned controller.
        unsafe { send_cmd(self.port, cmd_val).await }
    }

    /// The short (R1) response.
    pub fn read_r1(&self) -> u32 {
        // SAFETY: a read of the owned controller.
        unsafe { read_r1(self.port) }
    }

    /// The long (R2) response.
    pub fn read_r2(&self) -> [u32; 4] {
        // SAFETY: a read of the owned controller.
        unsafe { read_r2(self.port) }
    }

    /// The next transfer's block count.
    pub fn set_block_count(&self, count: u32) {
        // SAFETY: the owned controller.
        unsafe { set_block_count(self.port, count) }
    }

    /// End a multi-block transfer (CMD12).
    pub async fn stop_transfer(&self) -> Result<(), SdhiError> {
        // SAFETY: the owned controller.
        unsafe { stop_transfer(self.port).await }
    }

    /// Whether a card is in the slot.
    pub fn card_inserted(&self) -> bool {
        // SAFETY: a read of the owned controller.
        unsafe { card_inserted(self.port) }
    }

    /// Whether the card's write-protect switch is on.
    pub fn card_write_protected(&self) -> bool {
        // SAFETY: a read of the owned controller.
        unsafe { card_write_protected(self.port) }
    }

    /// Card removal and insertion edges since the last call, as `(removed, inserted)`.
    pub fn take_card_detect_events(&self) -> (bool, bool) {
        // SAFETY: the owned controller.
        unsafe { take_card_detect_events(self.port) }
    }

    /// Record the card's size in 512-byte blocks.
    pub fn set_card_blocks(&self, blocks: u32) {
        set_card_blocks(self.port, blocks)
    }

    /// The card's size in 512-byte blocks, as recorded.
    pub fn card_size_blocks(&self) -> u32 {
        card_size_blocks(self.port)
    }

    /// Issue `cmd_val` with `arg` and read its 64-byte data block by PIO (CMD6 status).
    pub async fn read_status_block_sw(
        &self,
        cmd_val: u16,
        arg: u32,
        buf: &mut [u8; 64],
    ) -> Result<(), SdhiError> {
        // SAFETY: the owned controller; `buf` is a live 64-byte buffer.
        unsafe { read_status_block_sw(self.port, cmd_val, arg, buf).await }
    }

    /// Read `count` blocks into `buf` by PIO.
    ///
    /// # Safety
    /// `buf` is valid for `count * 512` bytes of writes.
    pub async unsafe fn read_blocks_sw(&self, buf: *mut u8, count: u32) -> Result<(), SdhiError> {
        unsafe { read_blocks_sw(self.port, buf, count).await }
    }

    /// Write `count` blocks from `buf` by PIO.
    ///
    /// # Safety
    /// `buf` is valid for `count * 512` bytes of reads.
    pub async unsafe fn write_blocks_sw(&self, buf: *const u8, count: u32) -> Result<(), SdhiError> {
        unsafe { write_blocks_sw(self.port, buf, count).await }
    }

    /// Issue `cmd_val` and read `count` blocks into `buf` by DMA.
    ///
    /// # Safety
    /// `buf` is `count * 512` bytes of uncached memory, valid and unaliased until this returns.
    pub async unsafe fn read_blocks_dma(
        &self,
        cmd_val: u16,
        buf: *mut u8,
        count: u32,
    ) -> Result<(), SdhiError> {
        let mut rx = self.rx_dma.lock().await;
        unsafe { read_blocks_dma(self.port, cmd_val, buf, count, &mut rx).await }
    }

    /// Issue `cmd_val` and write `count` blocks from `buf` by DMA.
    ///
    /// # Safety
    /// `buf` is `count * 512` bytes of uncached memory, valid until this returns.
    pub async unsafe fn write_blocks_dma(
        &self,
        cmd_val: u16,
        buf: *const u8,
        count: u32,
    ) -> Result<(), SdhiError> {
        let mut tx = self.tx_dma.lock().await;
        unsafe { write_blocks_dma(self.port, cmd_val, buf, count, &mut tx).await }
    }
}
```

Check `read_status_block_sw`'s current signature (`:966`). If it takes a raw pointer instead of `&mut [u8; 64]`, pass `buf.as_mut_ptr()` in the wrapper. Run the HAL tests: `ok`.

- [ ] **Step 4: The BSP brings SDHI up once**

`crates/deluge-bsp/src/board.rs`: add `pub type SdCard = SDHI1;` (doc: "SDHI1: the SD card slot ([`crate::sd`])."), a `pub sd_card: Peri<'static, SdCard>` field (doc "SDHI1, the SD card slot."), and `sd_card: p.SDHI1,` in `Board::new`.

`crates/deluge-bsp/src/sd.rs` (device module):

- Add to its imports: `use embassy_sync::once_lock::OnceLock; use rza1l_hal::Peri; use rza1l_hal::dmac::{self, Dma}; use rza1l_hal::sdhi::Sdhi; use crate::board::{SdCard, SdRxDma, SdTxDma};`.
- Add:

```rust
rza1l_hal::bind_interrupts!(struct Irqs {
    SDHI1_STATUS => rza1l_hal::sdhi::StatusInterruptHandler<SdCard>;
    SDHI1_OP => rza1l_hal::sdhi::OperationInterruptHandler<SdCard>;
    DMAINT14 => dmac::InterruptHandler<SdRxDma>;
});

/// The SD host controller, once [`init_hw`] has run.
static SDHI: OnceLock<Sdhi<'static>> = OnceLock::new();

/// Bring the SD slot's hardware up, once: pins, SDHI1 with its DMA channels and interrupts.
/// [`init`] then identifies the card, and runs again after a card change.
pub fn init_hw(card: Peri<'static, SdCard>, tx: Peri<'static, SdTxDma>, rx: Peri<'static, SdRxDma>) {
    // SAFETY: P7_0..P7_7 are the SD slot's, mux 3 (SDHI1).
    unsafe {
        for pin in 0..=7 {
            gpio::set_pin_mux(7, pin, 3);
        }
    }
    let sdhi = Sdhi::new(card, Dma::new_without_interrupt(tx), Dma::new(rx, Irqs), Irqs, SD_OPTION);
    if SDHI.init(sdhi).is_err() {
        unreachable!("SDHI1 is owned once");
    }
}

/// The SD host controller, for code that drives it directly (clock experiments).
///
/// # Panics
/// Before [`init_hw`].
pub fn controller() -> &'static Sdhi<'static> {
    SDHI.try_get().expect("deluge_bsp::sd::init_hw hasn't run")
}
```

  Adjust the pin loop to match the existing `set_pin_mux(7, …, 3)` lines at `:308-315`: same pins, same mux. Keep their comments if any.
- In `init`, delete the pin-mux lines, `sdhi::init(SD_PORT, SD_OPTION)`, `sdhi::register_irqs(SD_PORT)` and `dmac::raw::register_completion_irq(SD_DMA_RX_CH)`, with their comments. Keep the `cache::dma_clean_inv_range` call. Fix the "once" comment so it says the protocol runs on every call while the hardware came up in `init_hw`.
- Replace every `sdhi::<fn>(SD_PORT, …)` call with `controller().<fn>(…)`, dropping the port argument. For the DMA reads and writes, drop the trailing channel argument: `controller().read_blocks_dma(cmd, buf, count)`. Remove `SD_PORT` and any now-unused `use` of `sdhi`'s free functions, `SD_DMA_RX_CH`/`SD_DMA_TX_CH` (they stay defined in `system.rs` for the board test).

Run: `rg -n "sdhi::[a-z_]+\(" crates/deluge-bsp/src/sd.rs`
Expected: no output.

- [ ] **Step 5: Callers**

- `crates/deluge-sdk/src/parts.rs`: add fields `sd_card: Option<Peri<'static, SdCard>>`, `sd_tx_dma: Option<Peri<'static, SdTxDma>>`, `sd_rx_dma: Option<Peri<'static, SdRxDma>>` (docs "for `deluge_bsp::sd::init_hw`"), and import the aliases. In `lib.rs` `run`, add `sd_card: Some(board.sd_card), sd_tx_dma: Some(board.dma.sd_tx), sd_rx_dma: Some(board.dma.sd_rx),` to the stash.
- `crates/deluge-sdk/src/plat/device.rs` `sd_init_card`:

```rust
pub(crate) async fn sd_init_card() -> Result<(), deluge_bsp::sd::SdError> {
    use core::sync::atomic::{AtomicBool, Ordering};
    static HW_UP: AtomicBool = AtomicBool::new(false);
    if !HW_UP.swap(true, Ordering::Relaxed) {
        let card = crate::parts::take("SDHI1 (the SD card slot)", |p| p.sd_card.take());
        let tx = crate::parts::take("the SD transmit DMA channel", |p| p.sd_tx_dma.take());
        let rx = crate::parts::take("the SD receive DMA channel", |p| p.sd_rx_dma.take());
        deluge_bsp::sd::init_hw(card, tx, rx);
    }
    deluge_bsp::sd::init().await
}
```

- Firmwares holding a `board` (app-loader, controller-firmware, msc-firmware, wp-probe, sd-bench): add `deluge_bsp::sd::init_hw(board.sd_card, board.dma.sd_tx, board.dma.sd_rx);` in `main` after `Board::new` and the BSP UART inits, before the executor runs.
- `firmwares/demo-firmware/src/main.rs` `setup()`:

```rust
    let (card, sd_tx, sd_rx) =
        deluge::parts::with(|p| (p.sd_card.take(), p.sd_tx_dma.take(), p.sd_rx_dma.take()));
    deluge_bsp::sd::init_hw(
        card.expect("SDHI1 is free"),
        sd_tx.expect("the SD TX DMA channel is free"),
        sd_rx.expect("the SD RX DMA channel is free"),
    );
```

- `hw-tests/sd-bench/src/tasks/bench.rs`: `sdhi::set_clock_div(1, sdhi::CLK_DIV_4)` becomes `deluge_bsp::sd::controller().set_clock_div(sdhi::CLK_DIV_4)`; likewise `CLK_DIV_2`.
- `hw-tests/wp-probe/src/tasks/probe.rs:52`: `rza1l_hal::sdhi::card_write_protected(SD_PORT)` becomes `deluge_bsp::sd::is_write_protected()`. Delete `SD_PORT` if unused.

Run: `rg -n "rza1l_hal::sdhi::[a-z_]+\(|sdhi::(init|register_irqs|card_write_protected|set_clock_div)\(" -g '*.rs' crates firmwares hw-tests | grep -v "^crates/rza1l-hal/src/sdhi.rs"`
Expected: no output.

- [ ] **Step 6: Suite, build check, commit**

Run: `tools/test.sh > /tmp/claude-1000/p4c2-t3.log 2>&1; echo "exit $?"` → `exit 0`. Run the **Firmware build check** and the link-layout check.

```bash
git add crates/rza1l-hal/src/sdhi.rs crates/deluge-bsp/src crates/deluge-sdk/src firmwares hw-tests
git commit -m "feat(rza1l-hal,bsp): Sdhi is built from its singleton, DMA channels and bindings; the SD slot's hardware comes up once in sd::init_hw"
```

---

### Task 4: SSI and SCUX take the audio singleton and typed DMA channels

**Files:**
- Modify: `crates/rza1l-hal/src/ssi.rs`, `crates/rza1l-hal/src/scux.rs`, `crates/rza1l-hal/src/scux/{dvu_path,src_path,usb_tx_path}.rs`
- Modify: `crates/deluge-bsp/src/{audio.rs,audio_block.rs,scux_dvu_path.rs,scux_src_path.rs,scux_usb_tx_path.rs,system.rs,board.rs}`
- Modify: `crates/deluge-sdk/src/{parts.rs,lib.rs,audio.rs}`
- Modify: `firmwares/{controller-firmware,demo-firmware}/src/main.rs`

**Interfaces:**
- Consumes: Task 1's `Dma::new_without_interrupt`, `BlockDma` (and its `wait_block(&self)` and `Deref` to `Dma`), and `BlockInterruptHandler`.
- Produces:
  - `ssi::init(Peri<'static, SSIF0>, tx: Dma<'static>, rx: Dma<'static>, ssicr: u32)`, `ssi::init_block_irq(Peri<'static, SSIF0>, tx: Dma<'static>, rx: BlockDma<'static>, ssicr: u32)`, `ssi::init_rx_only(Peri<'static, SSIF0>, rx: Dma<'static>, ssicr: u32)`, all safe, each storing its channels in SSI's statics; `pub async fn ssi::wait_rx_block()`. `SsiConfig` and the `TX_DMA_CH_ACTIVE`/`RX_DMA_CH_ACTIVE` statics are removed. The pointer, buffer and `tx_dma_ch`/`rx_dma_ch` accessors keep their signatures.
  - `unsafe scux::init_ffd_dma(ffd_ch: u8, dma: Dma<'static>, src_buf: *const u32, buf_bytes: usize)` and the same shape for `init_ffu_dma`, each storing the channel in SCUX's FIFO-channel table; `pub(crate) fn scux::{ffd_current_src(ffd: usize) -> u32, ffu_current_dst(ffu: usize) -> u32}`, which panic "scux path used before init" for an empty slot.
  - `unsafe dvu_path::init(ssi: Peri<'static, SSIF0>, ssi_rx: Dma<'static>, ssicr: u32, ffd: Dma<'static>)`, `unsafe dvu_path::init_with_src(ssi, ssi_rx, ssicr, ffd, engine_hz: u32)`, `unsafe src_path::init(ffd: Dma<'static>, ffu: Dma<'static>, fin_hz, fout_hz)`, `unsafe usb_tx_path::init(ffd: Dma<'static>, ffu: Dma<'static>, fin_hz, fout_hz)`.
  - BSP:
    - `audio::init(ssi: Peri<'static, AudioSsi>, tx: Peri<'static, SsiTxDma>, rx: Peri<'static, SsiRxDma>)`, safe
    - `audio::init_with_scux(ssi: Peri<'static, AudioSsi>, rx: Peri<'static, SsiRxDma>, ffd0: Peri<'static, ScuxFfd0Dma>)`, safe
    - `scux_dvu_path::{init, init_with_src}`, `scux_src_path::init` and `scux_usb_tx_path::init` take the matching peripherals
    - `system::SSICR: u32 = 0x002B_C020` replaces `SSI_CONFIG`
    - `Board.audio_ssi: Peri<'static, AudioSsi>` (`pub type AudioSsi = SSIF0`)
    - `Parts.{audio_ssi, ssi_tx_dma, ssi_rx_dma, scux_ffd0_dma}`

- [ ] **Step 1: Write the failing tests**

Append to `ssi.rs`:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod typed_tests {
    use super::*;
    use crate::dmac::{self, BlockDma, Dma};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH6, DMA_CH7, SSIF0};

    crate::bind_interrupts!(struct BlockIrqs {
        DMAINT7 => dmac::BlockInterruptHandler<DMA_CH7>;
    });

    #[test]
    fn ssi_rings_keep_their_completion_interrupts_masked() {
        test::reset();
        let tx = Dma::new_without_interrupt(unsafe { DMA_CH6::steal() });
        let rx = Dma::new_without_interrupt(unsafe { DMA_CH7::steal() });
        init(unsafe { SSIF0::steal() }, tx, rx, 0x002B_C020);
        assert_eq!((tx_dma_ch(), rx_dma_ch()), (6, 7));
        // DMAINT6/7 (GIC 47/48) never set-enabled: GICD_ISENABLER1 (0xE820_1104) bits 15, 16.
        assert_eq!(test::peek32(0xE820_1104) & ((1 << 15) | (1 << 16)), 0);
        // SSI0's TX/RX request routes: DMARS3 (channels 6 and 7) holds 0xE1 low, 0xE2 high.
        assert_eq!(test::peek32(0xFCFE_100C), 0x00E2_00E1);
    }

    #[test]
    fn the_block_ring_binds_its_interrupt_before_starting() {
        test::reset();
        let mut tx = Dma::new_without_interrupt(unsafe { DMA_CH6::steal() });
        let mut rx = BlockDma::new(unsafe { DMA_CH7::steal() }, BlockIrqs);
        let enabled_at = test::writes().iter().position(|w| *w == (0xE820_1104, 1 << 16));
        // The private bring-up, not `init_block_irq`: SSI's statics hold one ring set per
        // process, and `ssi_rings_keep_their_completion_interrupts_masked` stores it.
        unsafe { start_block_irq(0x002B_C020, &mut tx, &mut rx) };
        // Channel 7's SETEN (CHCTRL at 0xE820_01E8, value 1) comes after its interrupt is enabled.
        let started_at = test::writes().iter().rposition(|w| *w == (0xE820_01E8, 1));
        assert!(enabled_at.is_some() && started_at.is_some() && enabled_at < started_at);
        assert!(crate::gic::handler(48).is_some(), "DMAINT7 bound");
    }
}
```

Only the first test goes through the public `init`, which stores the channels in SSI's once-only statics. The second drives the private `start_block_irq` on handles it keeps, so the two can run in parallel.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib ssi 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: compile errors (`init` takes `&SsiConfig`).

- [ ] **Step 2: SSI onto the seam, with typed parameters**

In `ssi.rs`:

1. **Register helpers onto the seam.** `ssi_reg(off)` returns `crate::mmio::Reg32(SSI0_BASE + off)`, and the `SWRSTCR1` access becomes `crate::mmio::Reg8(SWRSTCR1)`. Convert their `.write_volatile(`/`.read_volatile()` calls to `.write(`/`.read()`. **Leave the descriptor writes (`tx_desc_u.add(n).write_volatile`, `rx_desc_u…`, the ring `…`) as raw volatile memory writes.** Do this by hand, not with a whole-file sed: grep `volatile` and convert only `ssi_reg(…)`, `fcr`/`swrstcr`-style register bindings.
2. **Uncached aliases.** Replace `(… as usize + UNCACHED_MIRROR_OFFSET) as *mut u32` with `crate::uncached(… as *mut u32)` (and `*const` likewise), so the descriptors are host-testable.
3. **Remove `SsiConfig` and the channel-number statics.** Rename the bodies to private functions that drive the handles: `init` → `unsafe fn start(ssicr: u32, tx: &mut Dma<'_>, rx: &mut Dma<'_>)`, `init_block_irq` → `unsafe fn start_block_irq(ssicr: u32, tx: &mut Dma<'_>, rx: &mut Dma<'_>)` (a `BlockDma` derefs to the `rx`), `init_rx_only` → `unsafe fn start_rx_only(ssicr: u32, rx: &mut Dma<'_>)`. Inside them:
   - `cfg.ssicr` → `ssicr`; `tx_chcfg(cfg.tx_dma_ch)` → `tx_chcfg(tx.number())`, and likewise for RX.
   - `dmac::raw::init_with_link_descriptor(cfg.tx_dma_ch, desc, DMARS_SSI0_TX)` → `tx.init_with_link_descriptor(desc, DMARS_SSI0_TX)`, and likewise for RX.
   - `dmac::raw::channel_start(cfg.tx_dma_ch)` → `tx.channel_start()`, and likewise for RX.
   - Delete the `TX_DMA_CH_ACTIVE`/`RX_DMA_CH_ACTIVE` stores, including the old `init`'s duplicate at `:299-300`, and the statics themselves.
4. **Hold the channels in statics.** Add:

```rust
use embassy_sync::once_lock::OnceLock;

/// SSI0's rings, once an `init*` has run: the pointer accessors read their positions.
static TX_DMA: OnceLock<Dma<'static>> = OnceLock::new();
static RX_DMA: OnceLock<Dma<'static>> = OnceLock::new();
/// The RX ring when it runs with per-block interrupts ([`init_block_irq`]).
static RX_BLOCK: OnceLock<BlockDma<'static>> = OnceLock::new();

fn keep<T>(slot: &OnceLock<T>, value: T) {
    if slot.init(value).is_err() {
        unreachable!("SSI0 and its DMA channels are owned once");
    }
}

/// The RX ring, polled or block-interrupt.
fn rx_dma() -> Option<&'static Dma<'static>> {
    RX_DMA.try_get().or_else(|| RX_BLOCK.try_get().map(|ring| &**ring))
}
```

   Then rewrite the accessors on them:
   - `tx_dma_ch()` returns `TX_DMA.try_get().map_or(0, |d| d.number())`, and `rx_dma_ch()` returns `rx_dma().map_or(0, |d| d.number())`. Both keep "0 before init".
   - In `tx_current_ptr`, replace `dmac::raw::current_src(TX_DMA_CH_ACTIVE.load(…))` with `TX_DMA.try_get().map_or(tx_buf_start() as u32, |d| d.current_src())`.
   - In `rx_current_ptr`, replace the `current_dst` read with `rx_dma().map_or(rx_buf_start() as u32, |d| d.current_dst())`.

   The rest of each function, including the conversion of the address to a pointer, is unchanged.
5. **Add the public typed entry points:**

```rust
use crate::Peri;
use crate::dmac::{BlockDma, Dma};
use crate::peripherals::SSIF0;

/// Start SSI0 as I²S master at `ssicr`, its TX and RX rings on `tx`/`rx`. The rings are polled
/// (see [`tx_current_ptr`]), so their completion interrupts stay masked.
pub fn init(_ssi: Peri<'static, SSIF0>, mut tx: Dma<'static>, mut rx: Dma<'static>, ssicr: u32) {
    tx.mask_interrupt();
    rx.mask_interrupt();
    // SAFETY: SSI0 and both channels are owned.
    unsafe { start(ssicr, &mut tx, &mut rx) };
    keep(&TX_DMA, tx);
    keep(&RX_DMA, rx);
}

/// [`init`] with the RX ring split into blocks whose interrupts [`wait_rx_block`] waits on.
pub fn init_block_irq(
    _ssi: Peri<'static, SSIF0>,
    mut tx: Dma<'static>,
    mut rx: BlockDma<'static>,
    ssicr: u32,
) {
    tx.mask_interrupt();
    // SAFETY: SSI0 and both channels are owned; `rx`'s block interrupt is already bound and on.
    unsafe { start_block_irq(ssicr, &mut tx, &mut rx) };
    keep(&TX_DMA, tx);
    keep(&RX_BLOCK, rx);
}

/// Start SSI0 with only its RX ring (TX comes from SCUX).
pub fn init_rx_only(_ssi: Peri<'static, SSIF0>, mut rx: Dma<'static>, ssicr: u32) {
    rx.mask_interrupt();
    // SAFETY: SSI0 and the channel are owned.
    unsafe { start_rx_only(ssicr, &mut rx) };
    keep(&RX_DMA, rx);
}

/// Wait for the next RX block of the ring [`init_block_irq`] started.
///
/// # Panics
/// If [`init_block_irq`] hasn't run.
pub async fn wait_rx_block() {
    RX_BLOCK.try_get().expect("ssi::init_block_irq hasn't run").wait_block().await
}
```

  Update the module and function docs that mention `SsiConfig` or `register_block_irq`.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib ssi 2>&1 | grep -E "^error|test result"`
Expected: `ok` (8 tests). Fix the existing `mod tests` if it constructed an `SsiConfig`.

- [ ] **Step 3: SCUX takes typed channels**

In `scux.rs`:
- Replace `FFD_DMA_CH_STORED`, `FFU_DMA_CH_STORED` and `recorded_dma_channel` with a table of the channels themselves:

```rust
use core::cell::RefCell;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use crate::dmac::Dma;

/// Each FIFO's DMA channel, once its path's `init` has handed it over.
type FifoChannels = Mutex<CriticalSectionRawMutex, RefCell<[Option<Dma<'static>>; 4]>>;
static FFD_DMA: FifoChannels = Mutex::new(RefCell::new([None, None, None, None]));
static FFU_DMA: FifoChannels = Mutex::new(RefCell::new([None, None, None, None]));

/// Run `f` on FIFO `n`'s channel in `table`.
///
/// # Panics
/// If no path has handed that FIFO a channel: reading a DMA position before `init` would read a
/// channel that isn't there.
fn with_fifo<R>(table: &FifoChannels, n: usize, f: impl FnOnce(&mut Dma<'static>) -> R) -> R {
    table.lock(|t| f(t.borrow_mut()[n].as_mut().expect("scux path used before init")))
}

/// FFD `n`'s DMA read position (CRSA).
pub(crate) fn ffd_current_src(n: usize) -> u32 {
    with_fifo(&FFD_DMA, n, |d| d.current_src())
}

/// FFU `n`'s DMA write position (CRDA).
pub(crate) fn ffu_current_dst(n: usize) -> u32 {
    with_fifo(&FFU_DMA, n, |d| d.current_dst())
}
```

- `init_ffd_dma(ffd_ch: u8, dma_ch: u8, …)` becomes `init_ffd_dma(ffd_ch: u8, mut dma: Dma<'static>, …)`.
  - Its body's first line becomes `dma.mask_interrupt();`.
  - `crate::dmac::raw::init_with_link_descriptor(dma_ch, desc, …)` becomes `dma.init_with_link_descriptor(desc, …)`, and any `chcfg(dma_ch)` becomes `chcfg(dma.number())`.
  - The `FFD_DMA_CH_STORED` store becomes, at the end, `FFD_DMA.lock(|t| t.borrow_mut()[ffd_ch as usize] = Some(dma));`.
- `init_ffu_dma` the same way, with `FFU_DMA`.
- In `start` and `start_path`, each `crate::dmac::raw::channel_start(<stored ch>)` becomes `with_fifo(&FFD_DMA, i, |d| unsafe { d.channel_start() })`, or `FFU_DMA` for the FFU loop. `i` is the loop's FIFO index; the masks gate it as before.
- `pub fn ffd_dma_ch(ffd_ch: u8) -> u8` returns `FFD_DMA.lock(|t| t.borrow()[ffd_ch as usize].as_ref().map_or(u8::MAX, |d| d.number()))`.

In `scux/dvu_path.rs`:
- Delete the `FFD_DMA_CH` static.
- `init(ssi: &SsiConfig, ffd_dma_ch: u8)` becomes `init(ssi: Peri<'static, SSIF0>, ssi_rx: Dma<'static>, ssicr: u32, ffd: Dma<'static>)`.
  - `ssi::init_rx_only(ssi)` becomes `ssi::init_rx_only(ssi, ssi_rx, ssicr)`.
  - It passes `ffd` (moved) to `scux::init_ffd_dma(0, ffd, …)`.
- `init_with_src` takes `(ssi, ssi_rx, ssicr, ffd, engine_hz)` the same way.
- `tx_current_ptr`'s `crate::dmac::raw::current_src(scux::recorded_dma_channel(&FFD_DMA_CH))` becomes `scux::ffd_current_src(0)`.

In `src_path.rs` and `usb_tx_path.rs`:
- Delete the `FFD_DMA_CH`/`FFU_DMA_CH` statics.
- `init(ffd_dma_ch: u8, ffu_dma_ch: u8, …)` becomes `init(ffd: Dma<'static>, ffu: Dma<'static>, …)`, which passes them to `init_ffd_dma`/`init_ffu_dma`.
- The position reads become `scux::ffd_current_src(n)` / `scux::ffu_current_dst(n)`, with `n` = 1 for `src_path` and 2 for `usb_tx_path`, the FIFOs each path already used.
- Keep each `init`'s `unsafe`, and add to its `# Safety`: "No other path uses this path's SRC unit and FIFO channels."

The five `*_before_init_panics` tests in `scux.rs` keep passing: an empty slot panics with the same message. Run `rg -n "dmac::raw|recorded_dma_channel|DMA_CH_STORED" crates/rza1l-hal/src/scux.rs crates/rza1l-hal/src/scux crates/rza1l-hal/src/ssi.rs`; expect no output.

Run the HAL tests: `ok`.

- [ ] **Step 4: The BSP audio takes its parts**

- **`board.rs`:** add `pub type AudioSsi = SSIF0;` (doc "SSIF0: the codec's I²S port ([`crate::audio`])."), a `pub audio_ssi: Peri<'static, AudioSsi>` field, and `audio_ssi: p.SSIF0,`.
- **`system.rs`:** replace `SSI_CONFIG` with:

```rust
/// SSI0's SSICR: I²S master, 24-bit data in 32-bit words, clock ÷4 (44.1 kHz with the Deluge's
/// audio crystal).
pub const SSICR: u32 = 0x002B_C020;
```

  In `board.rs`'s test, replace `SSI_CONFIG.tx_dma_ch`/`rx_dma_ch` with the literals `6`/`7` and a comment that SSI's channels are the board's choice, made here.
- **`audio.rs`:** `init` becomes:

```rust
pub fn init(ssi: Peri<'static, AudioSsi>, tx: Peri<'static, SsiTxDma>, rx: Peri<'static, SsiRxDma>) {
    log::debug!("audio: pin-mux SSI0 (direct, no SCUX)");
    // SAFETY: SSI0's pins and the codec-power GPIO, which only this module drives.
    unsafe {
        gpio::set_pin_mux(7, 11, 6); // AUDIO_XOUT
        gpio::set_pin_mux(6, 8, 3); // SSITXD0
        gpio::set_pin_mux(6, 9, 3); // SSISCK0
        gpio::set_pin_mux(6, 10, 3); // SSIWS0
        gpio::set_pin_mux(6, 11, 3); // SSIRXD0
        gpio::set_as_output(CODEC_PORT, CODEC_PIN);
        gpio::write(CODEC_PORT, CODEC_PIN, false);
    }
    let tx = Dma::new_without_interrupt(tx);
    #[cfg(not(feature = "audio-irq"))]
    ssi::init(ssi, tx, Dma::new_without_interrupt(rx), crate::system::SSICR);
    // `audio-irq`: RX runs as a per-block descriptor ring; its interrupt is bound before it starts.
    #[cfg(feature = "audio-irq")]
    ssi::init_block_irq(ssi, tx, BlockDma::new(rx, BlockIrqs), crate::system::SSICR);
    ostm::delay_ms(CODEC_POWER_DELAY_MS);
    // SAFETY: the codec-power GPIO.
    unsafe { gpio::write(CODEC_PORT, CODEC_PIN, true) };
    log::debug!("audio: CODEC_POWER asserted (direct SSI path)");
}
```

  Add the imports and the binding, the binding only under `audio-irq`:

```rust
#[cfg(feature = "audio-irq")]
rza1l_hal::bind_interrupts!(struct BlockIrqs {
    DMAINT7 => rza1l_hal::dmac::BlockInterruptHandler<SsiRxDma>;
});
```

  `init_with_scux` becomes safe and takes `(ssi: Peri<'static, AudioSsi>, rx: Peri<'static, SsiRxDma>, ffd0: Peri<'static, ScuxFfd0Dma>)`. Its `scux_dvu_path::init()` becomes `scux_dvu_path::init(ssi, rx, ffd0)`, and its GPIO calls go in `unsafe` blocks as in `init`.
- **`scux_dvu_path.rs`:** `init(ssi: Peri<'static, AudioSsi>, rx: Peri<'static, SsiRxDma>, ffd0: Peri<'static, ScuxFfd0Dma>)` calls `unsafe { rza1l_hal::scux::dvu_path::init(ssi, Dma::new_without_interrupt(rx), crate::system::SSICR, Dma::new_without_interrupt(ffd0)) }`, with a SAFETY comment that the DVU path's SRC0/0 and FFD0 are its own. `init_with_src(ssi, rx, ffd0, engine_hz)` likewise.
- **`scux_src_path.rs`** and **`scux_usb_tx_path.rs`:** `init(ffd: Peri<'static, ScuxFfdNDma>, ffu: Peri<'static, ScuxFfuNDma>, fin_hz, fout_hz)`, with N matching the old constants (1 and 2), wrapping each in `Dma::new_without_interrupt`.
- **`audio_block.rs`:** `wait_block()`'s body becomes `rza1l_hal::ssi::wait_rx_block().await`.

Run: `rg -n "SSI_CONFIG|SsiConfig|register_block_irq|dmac::raw" -g '*.rs' crates/deluge-bsp crates/deluge-sdk firmwares hw-tests`
Expected: none outside `board.rs`'s test comment, which must not name `SSI_CONFIG` either.

- [ ] **Step 5: Callers**

- **`parts.rs`:** add `audio_ssi`, `ssi_tx_dma`, `ssi_rx_dma` and `scux_ffd0_dma` fields (`Option<Peri<'static, …>>` of `AudioSsi`, `SsiTxDma`, `SsiRxDma`, `ScuxFfd0Dma`), and stash `board.audio_ssi`, `board.dma.ssi_tx`, `board.dma.ssi_rx`, `board.dma.scux_ffd0` in `run`.
- **`crates/deluge-sdk/src/audio.rs` `ensure_init`:** replace the `unsafe { deluge_bsp::audio::init() }` and its SAFETY comment with:

```rust
    // Brings up the codec on the direct SSI TX+RX DMA path (no SCUX); blocks ~5 ms. Owns the
    // codec, so it is incompatible with the USB UAC2 device tasks.
    let ssi = crate::parts::take("SSIF0 (the codec's I²S port)", |p| p.audio_ssi.take());
    let tx = crate::parts::take("the SSI transmit DMA channel", |p| p.ssi_tx_dma.take());
    let rx = crate::parts::take("the SSI receive DMA channel", |p| p.ssi_rx_dma.take());
    deluge_bsp::audio::init(ssi, tx, rx);
```

- **`firmwares/controller-firmware/src/main.rs`:** `unsafe { deluge_bsp::audio::init_with_scux() };` becomes `deluge_bsp::audio::init_with_scux(board.audio_ssi, board.dma.ssi_rx, board.dma.scux_ffd0);`.
- **`firmwares/demo-firmware/src/main.rs` `setup()`:** replace it with:

```rust
    let (ssi, ssi_rx, ffd0) =
        deluge::parts::with(|p| (p.audio_ssi.take(), p.ssi_rx_dma.take(), p.scux_ffd0_dma.take()));
    deluge_bsp::audio::init_with_scux(
        ssi.expect("SSIF0 is free"),
        ssi_rx.expect("the SSI RX DMA channel is free"),
        ffd0.expect("SCUX FFD0's DMA channel is free"),
    );
```

- [ ] **Step 6: Suite, build check, commit**

Run `tools/test.sh` (exit 0), the **Firmware build check** (the `audio_passthru_irq` example builds with `audio-irq`), and the link-layout check.

```bash
git add crates/rza1l-hal/src/ssi.rs crates/rza1l-hal/src/scux.rs crates/rza1l-hal/src/scux crates/deluge-bsp/src crates/deluge-sdk/src firmwares
git commit -m "feat(rza1l-hal,bsp): SSI and SCUX take SSIF0 and typed DMA channels, their polled rings keep DMAINT masked, and audio-irq waits on a typed block ring"
```

---

### Task 5: USB ports are initialised from their singleton and a typed handler

**Files:**
- Modify: `crates/rza1l-hal/src/usb/{mod.rs,driver.rs,host.rs}`
- Modify: `crates/deluge-sdk/src/{usb0.rs,usb_debug.rs,usb_serial.rs,parts.rs,lib.rs}`
- Modify: `firmwares/{app-loader/src/{main.rs,usbmsc.rs,devupload.rs},controller-firmware/src/main.rs,demo-firmware/src/main.rs,msc-firmware/src/main.rs}`, `hw-tests/{uac-host-firmware,vbus-probe}/src/main.rs`

**Interfaces:**
- Produces:
  - `rza1l_hal::usb::{Instance, DeviceInterruptHandler<T>, HostInterruptHandler<T>, DualRoleInterruptHandler<T>, ServesDevice, ServesHost}`.
  - `pub fn init_device_mode<'d, T: Instance, H: ServesDevice<T>>(usb: Peri<'d, T>, irq: impl Binding<T::Interrupt, H>) -> (UsbPort<Device>, Rusb1Driver<'d>)`.
  - `pub fn init_host_mode<'d, T: Instance, H: ServesHost<T>>(usb: Peri<'d, T>, irq: impl Binding<T::Interrupt, H>) -> (UsbPort<Host>, Rusb1HostDriver<'d>)`.
  - `Rusb1Driver<'d>` and `Rusb1HostDriver<'d>` carry the port's borrow; every firmware use is `Rusb1Driver<'static>` / `Rusb1HostDriver<'static>`.
  - `pub fn disconnect<T: Instance>(usb: Peri<'_, T>)`.
  - `Rusb1Driver::new` and `Rusb1HostDriver::new` become `pub(crate)`. `UsbPort::{into_host_mode, into_device_mode}` are deleted. `dcd_int_handler`/`hcd_int_handler` stay `pub unsafe`.
  - `Parts.usb0: Option<Peri<'static, USB0>>`, filled only when neither `usb-log` nor `usb-serial` claims USB0.

- [ ] **Step 1: Write the failing test**

Append to `usb/mod.rs` `mod tests`:

```rust
    #[test]
    fn dual_role_dispatch_follows_the_recorded_mode() {
        set_host_mode(1, false);
        assert_eq!(role(1), Role::Device);
        set_host_mode(1, true);
        assert_eq!(role(1), Role::Host);
        set_host_mode(1, false);
        assert_eq!(role(1), Role::Device);
    }
```

Run it: compile errors (no `set_host_mode`, `role`, `Role`).

- [ ] **Step 2: Implement**

In `usb/mod.rs`, add after `int_disable`:

```rust
// ---------------------------------------------------------------------------
// Typed ports and interrupt handlers
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicBool, Ordering};

use crate::Peri;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// A USB port singleton (`peripherals::USB0`, `USB1`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The port number, 0 or 1.
    const PORT: u8;
    /// Its interrupt.
    type Interrupt: Interrupt;
}

impl sealed::Sealed for crate::peripherals::USB0 {}
impl Instance for crate::peripherals::USB0 {
    const PORT: u8 = 0;
    type Interrupt = crate::interrupt::typelevel::USB0;
}
impl sealed::Sealed for crate::peripherals::USB1 {}
impl Instance for crate::peripherals::USB1 {
    const PORT: u8 = 1;
    type Interrupt = crate::interrupt::typelevel::USB1;
}

/// Whether each port was last initialised in host mode: the dual-role handler's dispatch.
static HOST_MODE: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];

fn set_host_mode(port: u8, host: bool) {
    HOST_MODE[port as usize].store(host, Ordering::Release);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Device,
    Host,
}

fn role(port: u8) -> Role {
    if HOST_MODE[port as usize].load(Ordering::Acquire) { Role::Host } else { Role::Device }
}

/// Port `T`'s interrupt in device mode.
pub struct DeviceInterruptHandler<T: Instance> {
    _usb: PhantomData<T>,
}
impl<T: Instance> Handler<T::Interrupt> for DeviceInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: the GIC dispatches this for T's interrupt.
        unsafe { dcd_int_handler(T::PORT) }
    }
}

/// Port `T`'s interrupt in host mode.
pub struct HostInterruptHandler<T: Instance> {
    _usb: PhantomData<T>,
}
impl<T: Instance> Handler<T::Interrupt> for HostInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: as above.
        unsafe { hcd_int_handler(T::PORT) }
    }
}

/// Port `T`'s interrupt for firmware that picks device or host mode at run time: dispatches to
/// the mode the port was last initialised in.
pub struct DualRoleInterruptHandler<T: Instance> {
    _usb: PhantomData<T>,
}
impl<T: Instance> Handler<T::Interrupt> for DualRoleInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: as above; the mode is the one `init_*_mode` recorded.
        unsafe {
            match role(T::PORT) {
                Role::Host => hcd_int_handler(T::PORT),
                Role::Device => dcd_int_handler(T::PORT),
            }
        }
    }
}

/// A handler that serves port `T` in device mode.
pub trait ServesDevice<T: Instance>: Handler<T::Interrupt> + sealed::Sealed {}
/// A handler that serves port `T` in host mode.
pub trait ServesHost<T: Instance>: Handler<T::Interrupt> + sealed::Sealed {}

impl<T: Instance> sealed::Sealed for DeviceInterruptHandler<T> {}
impl<T: Instance> sealed::Sealed for HostInterruptHandler<T> {}
impl<T: Instance> sealed::Sealed for DualRoleInterruptHandler<T> {}
impl<T: Instance> ServesDevice<T> for DeviceInterruptHandler<T> {}
impl<T: Instance> ServesDevice<T> for DualRoleInterruptHandler<T> {}
impl<T: Instance> ServesHost<T> for HostInterruptHandler<T> {}
impl<T: Instance> ServesHost<T> for DualRoleInterruptHandler<T> {}
```

Replace `init_device_mode`, `init_host_mode` and `disconnect` with:

```rust
/// Bring port `T` up in **device** mode: module clock, then the RUSB1 device driver for
/// `embassy-usb`. `irq` binds a handler that serves device mode; the driver unmasks the
/// interrupt when it starts.
///
/// The driver borrows `usb` for `'d`, so the port can't be brought up again while it lives.
/// Firmware that brings it up once per session [`disconnect`]s the previous session first.
pub fn init_device_mode<'d, T: Instance, H: ServesDevice<T>>(
    _usb: Peri<'d, T>,
    irq: impl Binding<T::Interrupt, H>,
) -> (UsbPort<Device>, Rusb1Driver<'d>) {
    interrupt::bind(irq);
    set_host_mode(T::PORT, false);
    // SAFETY: port T is owned for 'd; its clock and driver are this call's.
    unsafe {
        module_clock_enable(T::PORT);
        (UsbPort { port: T::PORT, _mode: PhantomData }, Rusb1Driver::new(T::PORT))
    }
}

/// Bring port `T` up in **host** mode: module clock, then the RUSB1 host driver for
/// `embassy-usb-host`, which unmasks the interrupt. `irq` binds a handler that serves host mode.
pub fn init_host_mode<'d, T: Instance, H: ServesHost<T>>(
    _usb: Peri<'d, T>,
    irq: impl Binding<T::Interrupt, H>,
) -> (UsbPort<Host>, Rusb1HostDriver<'d>) {
    interrupt::bind(irq);
    set_host_mode(T::PORT, true);
    // SAFETY: as above.
    unsafe {
        module_clock_enable(T::PORT);
        (UsbPort { port: T::PORT, _mode: PhantomData }, Rusb1HostDriver::new(T::PORT))
    }
}

/// Disconnect a device-mode port from the host and stop its SIE: the D+ pull-up off (the host
/// sees an unplug), USB interrupts masked, `USBE` cleared. A later [`init_device_mode`]
/// re-attaches.
pub fn disconnect<T: Instance>(_usb: Peri<'_, T>) {
    // SAFETY: port T is owned; its old driver's session is over.
    unsafe {
        use regs::{Rusb1Regs, SYSCFG_DPRPU, SYSCFG_USBE, rmw, wr};
        let port = T::PORT;
        int_disable(port);
        let regs = Rusb1Regs::ptr(port);
        // Pull-up off: host sees a disconnect.
        rmw(core::ptr::addr_of_mut!((*regs).syscfg0), SYSCFG_DPRPU, 0);
        wr(core::ptr::addr_of_mut!((*regs).intenb0), 0);
        wr(core::ptr::addr_of_mut!((*regs).brdyenb), 0);
        wr(core::ptr::addr_of_mut!((*regs).bempenb), 0);
        // Clear USBE to reset the SIE.
        let cur = regs::rd(core::ptr::addr_of!((*regs).syscfg0));
        wr(core::ptr::addr_of_mut!((*regs).syscfg0), cur & !SYSCFG_USBE);
    }
}
```

Then:
- Delete `UsbPort::into_host_mode`, `UsbPort::into_device_mode` and `quiesce_port`.
- **The drivers borrow their port.**
  - In `driver.rs`, `pub struct Rusb1Driver { port: u8 }` becomes `pub struct Rusb1Driver<'d> { port: u8, _usb: core::marker::PhantomData<&'d mut ()> }`.
  - `impl Rusb1Driver` becomes `impl<'d> Rusb1Driver<'d>`, and `impl<'d> Driver<'d> for Rusb1Driver` becomes `impl<'d> Driver<'d> for Rusb1Driver<'d>`.
  - Its `new` becomes `pub(crate) unsafe fn new(port: u8) -> Self`, building `_usb: PhantomData`.
  - In `host.rs`, do the same for `Rusb1HostDriver<'d>`, its inherent impl, and `impl<'d> UsbHostController<'d> for Rusb1HostDriver<'d>`.
  - Any other mention of the two types inside `rza1l-hal/src/usb/`, such as a doc example or the `UsbPort` return types, gains `<'d>` or `<'static>` as the compiler asks.
- Replace the module docs' "ISR wiring" example with:

```rust
//! ## ISR wiring
//!
//! Bind the port's interrupt to the handler for the mode you run, and pass the binding to
//! `init_device_mode` / `init_host_mode`:
//!
//! ```ignore
//! rza1l_hal::bind_interrupts!(struct Irqs { USB0 => usb::DeviceInterruptHandler<USB0>; });
//! let (_port, driver) = usb::init_device_mode(p.USB0, Irqs);
//! ```
//!
//! Firmware that picks device or host mode at run time binds `DualRoleInterruptHandler`,
//! which dispatches to the mode the port was last brought up in.
```

Run the HAL tests: `ok`.

- [ ] **Step 3: The SDK**

- **`crates/deluge-sdk/src/usb0.rs`** becomes:

```rust
//! USB0 in device mode: its interrupt binding, for whichever SDK feature owns the port (`usb-log`
//! or `usb-serial`, never both).

rza1l_hal::bind_interrupts!(pub(crate) struct Irqs {
    USB0 => rza1l_hal::usb::DeviceInterruptHandler<rza1l_hal::peripherals::USB0>;
});
```

- **`usb_debug::build(usb0)`:** `let (_port, driver) = unsafe { init_device_mode(0) };` becomes `let (_port, driver) = init_device_mode(usb0, crate::usb0::Irqs);`. Delete the later `rza1l_hal::interrupt::bind(crate::usb0::Irqs);` and rename the parameter from `_usb0` to `usb0`.
- **`usb_serial`:** `stash` drops its `interrupt::bind` line. `start(spawner, id, usb0)` calls `init_device_mode(usb0, crate::usb0::Irqs)` instead of `unsafe { init_device_mode(0) }`. Adjust the SAFETY comments that cited the old call.
- **`parts.rs`:** add `pub usb0: Option<Peri<'static, USB0>>` (doc: "USB0, when neither `usb-log` nor `usb-serial` claims it, for `rza1l_hal::usb::init_*_mode`").
- **`lib.rs` `run`:** compute it before the stash:

```rust
            #[cfg(not(any(feature = "usb-log", feature = "usb-serial")))]
            let usb0_part = Some(usb0);
            #[cfg(any(feature = "usb-log", feature = "usb-serial"))]
            let usb0_part = None;
```

  Then add `usb0: usb0_part,` to the `Parts`. Nothing reorders. With `usb-serial`, `usb0` was already moved into `usb_serial::stash`. With `usb-log`, the `None` branch leaves `usb0` for `usb_debug::build` after `setup()`, as now. With neither feature, the part goes to the app. Step 5 runs the 4a feature probes to confirm.

- [ ] **Step 4: Firmwares**

- **app-loader (`usbmsc.rs`, `devupload.rs`):** delete `USB_IRQ_REGISTERED` and its `gic::register` block, and add a module-level binding:

```rust
rza1l_hal::bind_interrupts!(struct UsbIrqs {
    USB0 => rza1l_hal::usb::DeviceInterruptHandler<rza1l_hal::peripherals::USB0>;
});
```

  `let (_port, driver) = init_device_mode(0);` becomes:

```rust
        // SAFETY: one USB session at a time (this function's contract): the previous session's
        // driver is gone, so this session is USB0's only user.
        let usb0 = rza1l_hal::peripherals::USB0::steal();
        let (_port, driver) = init_device_mode(usb0, UsbIrqs);
```

  In `main.rs:488`, `unsafe { rza1l_hal::usb::disconnect(0) };` becomes `rza1l_hal::usb::disconnect(unsafe { rza1l_hal::peripherals::USB0::steal() });`. Keep the comment and add "(the listener's session is over)". `board.usb0` is unused in app-loader: `let _ = board.usb0;` is not needed, because partial moves don't warn.
- **controller-firmware and demo-firmware:**
  - Replace the `gic::register(rza1l_hal::usb::USB0_IRQ, || { if USB0_HOST_MODE … })` block and its comment with a module-level `bind_interrupts!(struct UsbIrqs { USB0 => rza1l_hal::usb::DualRoleInterruptHandler<rza1l_hal::peripherals::USB0>; });`.
  - The two init sites become `init_host_mode(usb0, UsbIrqs)` / `init_device_mode(usb0, UsbIrqs)`, where `usb0` is `board.usb0` (controller) or `deluge::parts::with(|p| p.usb0.take()).expect("USB0 is free")` (demo, taken once before the `if`).
  - `USB0_HOST_MODE` stays as the boot-time choice; delete its doc's mention of the ISR.
- **msc-firmware:** `DeviceInterruptHandler` binding; `init_device_mode(board.usb0, UsbIrqs)`; delete the `gic::register` block.
- **uac-host-firmware:** `HostInterruptHandler` binding; `init_host_mode(board.usb0, UsbIrqs)`; delete the `gic::register` block.
- **vbus-probe:** `HostInterruptHandler` binding; `init_host_mode(board.usb0, UsbIrqs)`. It had no handler before; now its interrupts are serviced.

- **The lifetime in firmware signatures.** Every type position naming `Rusb1Driver` or `Rusb1HostDriver` outside the HAL becomes `Rusb1Driver<'static>` / `Rusb1HostDriver<'static>`: `UsbDevice<'static, Rusb1Driver>`, `CdcAcmClass<'static, Rusb1Driver>`, `midi::Sender<'static, Rusb1Driver>`, task arguments, `Option<…>` locals, and `usb_host_supervisor(driver: Rusb1HostDriver, …)` in `deluge-bsp/src/usb/host.rs`. `use` lines don't change. The files:
  - `firmwares/app-loader/src/{usbmsc,devupload}.rs`
  - `firmwares/msc-firmware/src/tasks/usb.rs`
  - `firmwares/demo-firmware/src/{main.rs,tasks/midi.rs,tasks/usb_host.rs,tasks/usb.rs}`
  - `firmwares/controller-firmware/src/{main.rs,tasks/midi.rs,tasks/cdc.rs,tasks/usb_host.rs,tasks/usb.rs}`
  - `crates/deluge-bsp/src/usb/host.rs`
  - `crates/deluge-sdk/src/{usb_serial,usb_debug}.rs`

  Build each firmware and fix what E0106 ("missing lifetime specifier") or E0726 ("implicit elided lifetime not allowed") points at, until the **Firmware build check** is clean.

Run: `rg -n "gic::register|init_(device|host)_mode\((0|1)\)|usb::disconnect\((0|1)\)|USB_IRQ_REGISTERED" -g '*.rs' crates firmwares hw-tests examples wren`
Expected: exactly two lines, both in `crates/rza1l-hal/src/dmac/raw.rs` (`register_completion_irq` and `register_block_irq`, which Task 6 deletes).

- [ ] **Step 5: Suite, build check, commit**

Run `tools/test.sh` (exit 0) and the **Firmware build check**. Then run the two 4a feature probes:

```bash
cargo build-fw -p usb_log 2>&1 | tail -1
cargo build-fw -p blinky --features deluge/usb-serial 2>&1 | tail -1
cargo build-fw -p usb_log --features deluge/usb-serial 2>&1 | grep -m1 -o "both drive USB0"
```

Expected: `Finished`, `Finished`, then `both drive USB0`. Then the link-layout check.

```bash
git add crates/rza1l-hal/src/usb crates/deluge-sdk/src firmwares hw-tests
git commit -m "feat(rza1l-hal): USB ports are brought up from their singleton and a device, host or dual-role handler binding, and the drivers borrow the port"
```

---

### Task 6: `raw` and `gic::register` leave the public API; docs

**Files:**
- Modify: `crates/rza1l-hal/src/{dmac.rs,dmac/raw.rs,gic.rs,interrupt.rs}`, `crates/deluge-bsp/src/board.rs` (doc)
- Modify: `CHANGELOG.md`, `crates/rza1l-hal/README.md`, `crates/deluge-bsp/README.md`, `docs/advanced-guide.md`, the spec

- [ ] **Step 1: Close the API**

- **`dmac.rs`:** `#[doc(hidden)] pub mod raw;` becomes `mod raw;`, private to `dmac`. Its doc line ("what drivers call until Phase 4b migrates them") becomes "The DMAC by channel number: `Dma`'s implementation."
  - Delete `register_completion_irq` and `register_block_irq` from `raw.rs`, plus `DMA_INT_HANDLERS`, `DMA_BLOCK_HANDLERS`, the `dma_int_N`/`dbi_N` functions and the `block_handlers!` macro in `dmac.rs`. Their only users were `gic::register` paths, which Tasks 3–5 replaced.
  - Keep `on_dma_int` and `on_block_int`, which the typed handlers call.
  - Drop `pub` from the remaining `raw` functions where the compiler doesn't need it. They're module-private either way.
- **`gic.rs`:** delete `pub unsafe fn register`, and make `set_handler` `pub(crate)`. Update the module doc ("Register handlers via [`register`]" → "Drivers install handlers through [`crate::interrupt::bind`]").
- **`interrupt.rs`:** no change. `bind` stays the one public way to install a handler.

Run:
- `rg -n "pub unsafe fn new\(" crates/rza1l-hal/src` → no output.
- `rg -n "gic::register|dmac::raw" -g '*.rs' crates firmwares hw-tests usb examples wren drivers linux | grep -v "^crates/rza1l-hal/src/dmac"` → no output. The HAL's other modules don't name `raw` either: Tasks 3 and 4 moved SDHI, SSI and SCUX onto their `Dma` handles, and the UART already was.
- `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|^error"` → `ok`. Fix any in-crate test that called `raw::register_*` by building a typed `Dma`/`BlockDma` instead.

- [ ] **Step 2: Docs**

- **`board.rs`:** the `BoardDma` doc paragraph becomes "Each role's driver takes its channel (`bus::init`, `uart::init_*`, `sd::init_hw`, `audio::init*`); a channel the board's firmware doesn't use stays here, unused."
- **`CHANGELOG.md`, top of `### Changed`:**

```markdown
- Phase 4 is complete: every `rza1l-hal` driver is built from its peripheral singleton and a typed
  interrupt binding. **Breaking:** `sdhi::Sdhi::new(Peri<SDHIn>, tx, rx, Irqs, sd_option)` replaces
  the port-numbered SDHI functions; `ssi::init*` take `Peri<SSIF0>` and typed DMA channels
  (`SsiConfig` is gone); the SCUX FIFO and path inits take `dmac::Dma`; `usb::init_device_mode` /
  `init_host_mode` / `disconnect` take the port singleton and a `DeviceInterruptHandler`,
  `HostInterruptHandler` or `DualRoleInterruptHandler` binding; `dmac::raw` and `gic::register` are
  no longer public (bind handlers with `bind_interrupts!`; drivers hold typed `Dma` handles). New: `Dma::new_without_interrupt` for
  polled rings, `dmac::BlockDma` + `BlockInterruptHandler` for per-block rings, typelevel
  `IRQ0`–`IRQ7` and SDHI interrupts. `usb::Rusb1Driver<'d>` and `Rusb1HostDriver<'d>` borrow their
  port, so a firmware type is `Rusb1Driver<'static>`.
- `deluge_bsp::Board` also hands out SDHI1 (`sd_card`) and SSIF0 (`audio_ssi`). **Breaking:**
  `sd::init_hw(card, tx, rx)` brings the SD slot up once (`sd::init` now runs only the card
  protocol); `audio::init(ssi, tx, rx)` and `audio::init_with_scux(ssi, rx, ffd0)` take their
  parts; `system::SSI_CONFIG` is `system::SSICR`. `deluge::parts` gains the SD, audio and (when the
  SDK doesn't own it) USB0 peripherals.
```

- **`crates/rza1l-hal/README.md`:** in the module table, the `sdhi`, `ssi`, `scux`, `usb`, `dmac` and `gic` rows describe the typed constructors above, and the `dmac` row no longer mentions `raw`. The paragraph about SDHI/SSI/SCUX/USB keeping `unsafe` init until Phase 4c becomes "Every driver takes its singleton and binding; the SCUX path inits stay `unsafe` because the paths share the block (each must claim distinct SRC units)."
- **`crates/deluge-bsp/README.md`:**
  - The `board` row lists `sd_card` and `audio_ssi`.
  - The boot example adds `deluge_bsp::sd::init_hw(board.sd_card, board.dma.sd_tx, board.dma.sd_rx);`.
  - `deluge_bsp::audio::init()` becomes `deluge_bsp::audio::init(board.audio_ssi, board.dma.ssi_tx, board.dma.ssi_rx);`, outside the `unsafe` block.
- **`docs/advanced-guide.md`:** search for `init_device_mode(0)`, `gic::register`, `audio::init()` and `sd::init()` and update each to the new call.
- **Spec,** after the "4b done" paragraph:

```markdown
**4c done (2026-10-15 plans 4c-1, 4c-2):** DelugeFirmware#next builds on the owned API (4c-1, with
a shared-position `UartRx::steal` for its fault paths). SDHI (`Sdhi`), SSI and SCUX (typed DMA,
masked polled rings, typed block ring), USB (singleton + device/host/dual-role binding) and the
GPIO IRQs and SGIs are typed. Every HAL driver holds its typed DMA handles; `dmac::raw` is
private to `dmac` and `gic::register` is gone. The USB drivers borrow their port. No public
`unsafe fn new()` remains, which with 4a's USB0 exclusivity meets the Phase 4 exit.
```

- [ ] **Step 3: Final verification and commit**

- `tools/test.sh > /tmp/claude-1000/p4c2-final.log 2>&1; echo "exit $?"` → `exit 0`, with `failed 0` in the summed results.
- `tools/link-layout.sh check && tools/check-manifests.sh` → pass.
- The **Firmware build check**.
- The simulator check from the 4b plan (`oled_hello` headless, `cv 0 0 0 0` / `gate 0 0 0 0`; `synced` follows the LED's blink).

```bash
git add -A crates CHANGELOG.md docs/advanced-guide.md docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md
git commit -m "feat(rza1l-hal)!: dmac::raw is private to dmac and gic::register is gone; Phase 4 docs"
```

---

### Task 7: DelugeFirmware follows (DelugeFirmware)

**Files:** under `/home/kate/GitHub/DelugeFirmware/src/bsp/rust/`: `src/runtime/device.rs`, `src/peripherals/{usb.rs,board.rs}`, and after the push `Cargo.toml` and `Cargo.lock`.

**Interfaces:**
- Consumes:
  - Task 5's `usb::{init_device_mode, DeviceInterruptHandler}`.
  - Task 3's `sd::init_hw`.
  - Task 4's `audio::init(ssi, tx, rx)`.
  - Task 1's `typelevel::{SGI8, SGI9}`, and `interrupt::bind`.

- [ ] **Step 1: Branch, and see it fail**

```bash
cd /home/kate/GitHub/DelugeFirmware && git checkout -b sdk-ownership-4c2 next
git -C /home/kate/GitHub/deluge-sdk branch --show-current   # expect refactor/rza1-ownership-4c2
cd src/bsp/rust && cargo check --target armv7a-deluge-eabihf.json -Zbuild-std=core,alloc -Zjson-target-spec --no-default-features --features rtt --config local-sdk.toml 2>&1 | grep -E "^error" | sort | uniq -c
```

Expected errors: `gic::register` not found (SGI 8/9 and USB0), `init_device_mode` arguments, `audio::init` arguments. Nothing in DelugeFirmware calls `deluge_bsp::sd::init_hw` yet; it panics at run time rather than failing to compile, so Step 2 adds it.

- [ ] **Step 2: SGIs, USB0, the SD slot and the audio parts**

In `src/runtime/device.rs`:
- Turn `audio_sgi_handler` and `stream_sgi_handler` into handler types. Keep their bodies and docs:

```rust
/// Drives [`AUDIO_EXEC`] from [`AUDIO_SGI`]. …(keep the existing doc)…
struct AudioSgi;

impl rza1l_hal::interrupt::Handler<rza1l_hal::interrupt::typelevel::SGI8> for AudioSgi {
    unsafe fn on_interrupt() {
        // …the existing audio_sgi_handler body…
    }
}

/// Drives [`STREAM_EXEC`] from [`STREAM_SGI`]. …
struct StreamSgi;

impl rza1l_hal::interrupt::Handler<rza1l_hal::interrupt::typelevel::SGI9> for StreamSgi {
    unsafe fn on_interrupt() {
        // …the existing stream_sgi_handler body…
    }
}

rza1l_hal::bind_interrupts!(struct SgiIrqs {
    SGI8 => AudioSgi;
    SGI9 => StreamSgi;
});
```

- Add a compile-time pin that the constants match: `const _: () = assert!(AUDIO_SGI as u16 == <rza1l_hal::interrupt::typelevel::SGI8 as rza1l_hal::interrupt::Interrupt>::ID && STREAM_SGI as u16 == <rza1l_hal::interrupt::typelevel::SGI9 as rza1l_hal::interrupt::Interrupt>::ID);`
- In `start()`, the `rza1l_hal::gic::register(AUDIO_SGI as u16, audio_sgi_handler);` and `…(STREAM_SGI as u16, stream_sgi_handler);` lines become `rza1l_hal::interrupt::bind::<rza1l_hal::interrupt::typelevel::SGI8, AudioSgi>(SgiIrqs);` and the SGI9/`StreamSgi` equivalent. The `set_priority`/`enable` calls stay.
- After `deluge_bsp::uart::init_midi(…)`, add `deluge_bsp::sd::init_hw(board.sd_card, board.dma.sd_tx, board.dma.sd_rx);`.
- `let usb_midi = unsafe { usb::build() };` becomes `let usb_midi = unsafe { usb::build(board.usb0) };`.
- After it, add `board::stash_audio(board.audio_ssi, board.dma.ssi_tx, board.dma.ssi_rx);`. `start()` already imports the `peripherals::board` module, and the path `board::stash_audio` resolves to the module while `board.audio_ssi` resolves to the local `Board`, exactly as `board.pic_uart` does today.

In `src/peripherals/usb.rs`, every `Rusb1Driver` type position (the `UsbMidi.device` field, the task arguments) becomes `Rusb1Driver<'static>`; there are five across the crate, `rg -n Rusb1Driver src` lists them. `pub unsafe fn build() -> UsbMidi` becomes `pub unsafe fn build(usb0: rza1l_hal::Peri<'static, rza1l_hal::peripherals::USB0>) -> UsbMidi`. Its `gic::register` line is deleted, and `init_device_mode(0)` becomes `init_device_mode(usb0, UsbIrqs)` with:

```rust
rza1l_hal::bind_interrupts!(struct UsbIrqs {
    USB0 => rza1l_hal::usb::DeviceInterruptHandler<rza1l_hal::peripherals::USB0>;
});
```

In `src/peripherals/board.rs` (device):

```rust
/// SSI0 and its DMA channels, from `runtime::device::start` until the app's
/// `deluge_board_init_audio` brings the codec up (its place in the C++ init sequence).
#[cfg(target_os = "none")]
static AUDIO_PARTS: embassy_sync::blocking_mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    core::cell::RefCell<Option<(
        rza1l_hal::Peri<'static, deluge_bsp::board::AudioSsi>,
        rza1l_hal::Peri<'static, deluge_bsp::board::SsiTxDma>,
        rza1l_hal::Peri<'static, deluge_bsp::board::SsiRxDma>,
    )>>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(None));

/// Keep the audio parts for [`deluge_board_init_audio`].
#[cfg(target_os = "none")]
pub(crate) fn stash_audio(
    ssi: rza1l_hal::Peri<'static, deluge_bsp::board::AudioSsi>,
    tx: rza1l_hal::Peri<'static, deluge_bsp::board::SsiTxDma>,
    rx: rza1l_hal::Peri<'static, deluge_bsp::board::SsiRxDma>,
) {
    AUDIO_PARTS.lock(|p| *p.borrow_mut() = Some((ssi, tx, rx)));
}
```

and `deluge_board_init_audio`'s body becomes:

```rust
    let (ssi, tx, rx) = AUDIO_PARTS
        .lock(|p| p.borrow_mut().take())
        .expect("deluge_board_init_audio runs once, after runtime::device::start");
    deluge_bsp::audio::init(ssi, tx, rx);
```

with its SAFETY comment removed.

- [ ] **Step 3: Check, test, build, commit**

- The device check → exit 0, with no new warnings in touched files (the existing `oled_display_off` bench-only warning stays).
- `cargo test --config local-sdk.toml` → `passed 221 failed 0`.
- `./dbt build` → exit 0.

```bash
cd /home/kate/GitHub/DelugeFirmware && git checkout src/bsp/rust/Cargo.lock
git add src/bsp/rust/src
git commit -m "build(bsp): typed SGI and USB0 bindings, the SD slot's init_hw, and the audio parts for the C++ init sequence"
```

- [ ] **Step 4: Stop, then pin after the push**

Stop and report. Once the human partner has merged `refactor/rza1-ownership-4c2` into deluge-sdk `main` and pushed it:
- bump the six `rev`s in `src/bsp/rust/Cargo.toml` from `e1a96c044cbaed7537e01ea6fda0c7f88c9f1dbe` to the new `origin/main`;
- `cargo update -p deluge-bsp` with `local-sdk.toml` moved aside;
- `cargo test --locked` → 221/0;
- `./dbt build` → exit 0;
- restore `local-sdk.toml`;
- commit `build(bsp): pin deluge-sdk at <8 chars>, the revision that completes Phase 4`.

These are the same steps as 4c-1's Task 3.

---

## Hardware smoke tests (the human partner's merge gate)

1. **Audio.** `audio_passthru` and `audio_passthru_irq` pass line-in to line-out without glitches or a stall (Review Focus 1 and 2). `deluge-controller` plays audio through SCUX and its volume encoder works.
2. **Encoders and trigger clock.** All six encoders turn (typed IRQ0–4/7). `clock_jacks` counts edges on the trigger-clock input (IRQ6).
3. **SD.** `sd_demo` reads the card. `sd-bench` runs at both clock dividers.
4. **SD card swap.** In DelugeFirmware or `msc-firmware`, pull the card and reinsert it: it is re-identified (Review Focus 3).
5. **USB.**
   - `usb_log` logs and an app with `usb-serial` enumerates.
   - `msc-firmware` mounts as a disk.
   - `deluge-controller` works in device mode (USB MIDI) and, built with `USB0_HOST_MODE` true, in host mode with a MIDI keyboard (Review Focus 4).
   - `uac-host-firmware` enumerates a USB audio device.
6. **app-loader sessions.** DATA TRANSFER (MSC), back, dev-upload (CDC), back, DATA TRANSFER again: each enumerates (Review Focus 5).
7. **DelugeFirmware.** Boots; audio plays; USB MIDI enumerates; the SD card loads samples; the audio and storage executors run (SGI 8/9).
