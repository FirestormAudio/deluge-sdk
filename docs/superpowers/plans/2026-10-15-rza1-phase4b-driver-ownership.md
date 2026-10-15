# RZ/A1 Phase 4b: Driver Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move RSPI, the SCIF UART, MTU2 and OSTM with the time driver onto Phase 4a's ownership model: each driver is an owned handle built from its peripheral singleton and its interrupt binding, the board hands those singletons out by role, and the SDK keeps the ones no capability has claimed yet.

**Architecture:** Each HAL driver gets a sealed `Instance` trait over its `peripherals::*` singletons and an owned type (`rspi::Rspi`, `uart::{Uart, UartRx, UartTx}`, `mtu2::OneShot`) whose constructor takes `Peri<'d, T>`. The channel-number functions become private. The time driver takes `OSTM0`/`OSTM1` and two typed handlers, and `ostm::delay_ms` becomes safe because only the time driver can start OSTM0. `deluge_bsp::Board` gains the RSPI0, SCIF0/1 and MTU2 2/3 singletons by role. The BSP's bus, UART, MIDI-gate and encoder modules take them in their `init` functions. The SDK runtime keeps the board's unclaimed peripherals in `deluge::parts` until a capability takes them, and a hybrid firmware takes its own through `deluge::parts::with`. Fault and hand-off code that can't reach an owner uses explicit `unsafe` escape hatches: `Dma::steal`, `dmac::stop_all`, `Rspi::steal` and `time_driver::stop`.

**Tech Stack:** Rust nightly, `embassy-hal-internal` 0.5 (`Peri`, `peripherals!`), `embassy-sync` 0.8 (`Mutex`, `MutexGuard::map`, `OnceLock`), the HAL's MMIO test seam (`rza1l_hal::mmio::test`), QEMU ARM host tests, `armv7a-none-eabihf` firmware builds.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 4). This is the second of three Phase 4 plans. 4a (`2026-10-15-rza1-phase4a-ownership-foundation.md`) built the singletons, `bind_interrupts!`, typed DMA and `Board`. Phase 4c migrates SDHI, SSI+SCUX and USB, deletes `dmac::raw` and `gic::register`, and migrates DelugeFirmware#next.

## Global Constraints

- Layering (spec §2): `rza1l-hal` never names `deluge` in code or depends on a `deluge-*` crate. `wren/` and `examples/baremetal/` reach the platform only through the `deluge` facade. `usb/` and `drivers/` stay board-agnostic. `deluge-bsp` never depends on the simulator. `tools/check-layering.sh` enforces these rules and runs first in `tools/test.sh`.
- Phase 4 exit (spec): "no public `unsafe fn new()` constructors; enabling both `usb-log` and `usb-serial` fails to compile instead of panicking." 4a met the second half. This plan removes the `unsafe fn new()` constructors of `rspi`, `uart`, `ostm` and `gpio`. SDHI's and USB's are 4c's.
- During migration, "Old `unsafe` free functions move to `#[doc(hidden)] pub mod raw`" (spec). In 4b no driver needs a `raw` module: each one's channel-number functions become private in the same task that adds its owned type.
- No memory-layout change: `tools/link-layout.sh check` must pass after every task.
- Host tests: `cargo test --target armv7-unknown-linux-gnueabihf -p <crate> --lib <filter>`; the BSP needs `--features usb-host`.
- Firmware builds: use `cargo build-fw -p <pkg>` (the alias in `.cargo/config.toml`). zsh does not word-split a variable that holds a command, so write commands out in full.
- **Firmware build check**, run at the end of every task:
  ```bash
  for p in app-loader deluge-controller demo-firmware msc-firmware wp-probe vbus-probe sd-bench uac-host-firmware wren-firmware; do printf '%s: ' $p; cargo build-fw -p $p 2>&1 | tail -1; done
  tools/build-examples.sh > /tmp/claude-1000/p4b-examples.log 2>&1; echo "examples exit $?"
  ```
  Expected: nine `Finished` lines and `examples exit 0`. No new warnings: a build that newly warns about an unused import or an unused `unsafe` block is not done yet.
- DelugeFirmware#next pins deluge-sdk at `28bc21ce`. This plan doesn't touch it; 4c migrates it. But 4b must leave it a migration path: `uart::{try_read_pic, try_write_pic, try_write_midi, midi_write_space, try_read_midi}` and `Dma::steal` exist for its display, fault and MIDI code.
- Never push; never publish.

## Decisions settled before execution

1. **Plan split.** 4b covers the DMA escape hatches and new interrupt types, then RSPI, UART, MTU2, and OSTM with the time driver. 4c covers SDHI, SSI+SCUX, USB, deleting `dmac::raw`, and DelugeFirmware#next (user decision).
2. **Escape hatches are explicit and `unsafe`:** `Dma::steal::<C>()`, `dmac::stop_all()`, `Rspi::steal::<T>()` and `time_driver::stop()`. None of them touches a register or binds an interrupt except as its name says. There is no `UartTx::steal`. The PIC panic path and DelugeFirmware's fault path use the BSP's installed transmitter (`uart::try_write_pic`), which is safe and does nothing if the PIC link never came up. This narrows the survey's "a `steal` on every driver" to the drivers whose escape paths need one.
3. **The bus owns RSPI0 and the OLED's DMA channel.** `deluge_bsp::bus::init(shared_spi, oled_dma)` builds the `Rspi` and the OLED `Dma` together. The survey had `cv_gate::init(rspi0)` build the `Rspi`, but an OLED frame is an RSPI0 transmit, so its channel belongs with the bus. Moving bring-up out of `cv_gate::init` also fixes a latent bug. An SDK app that uses only the OLED never initialised RSPI0, and worked only because app-loader's `cv_gate::init` had already configured it before chain-loading.
4. **The OLED's DMA migrates in 4b.** The survey assigned it to neither half. 4c deletes `dmac::raw`, so it has to move somewhere, and it is RSPI0's.
5. **`Board` fields and the parts stash use role names**, like `BoardDma`: `shared_spi`, `midi_uart`, `pic_uart`, `gate_timer` and `encoder_poll_timer`, with the type aliases `SharedSpi`, `MidiScif`, `PicScif`, `GateTimer` and `EncoderPollTimer`. The SDK keeps unclaimed peripherals in a device-only `deluge::parts` (survey design), not in a changed `setup` signature.
6. **The UART has two handler types**, `ErrorInterruptHandler<T>` and `TxInterruptHandler<T>`. One generic type implementing `Handler<T::ErrorInterrupt>` and `Handler<T::TxInterrupt>` is rejected by coherence (E0119), because the compiler can't prove the two associated types differ.
7. **The BSP muxes UART pins before `Uart::new`.** Before, the order was SCIF init, then pin mux, then IRQs and DMA. Now it is pin mux, then everything else. A receive ring started before its RxD pin is muxed would sample an unconnected pin.
8. **Dead APIs are deleted, not migrated.** None of these has a caller in this repo or in DelugeFirmware: RSPI's interrupt-driven CV path (`register_irq`, `irq_spri`, `send32`, `enable_rx_irq`, `disable_rx_irq`, `reset_rx_buf`), `Rspi<CH, BITS>` with its `embedded-hal` `SpiBus` impls, `uart::ScifUart`, MTU2's free-running API (`start_free_running`, `count`, `stop`) and `ostm::Ostm<N>`.
9. **A prescaler the MTU2 channel lacks panics** in `OneShot::new`. It used to fall back silently to /1, which runs a 1 ms gate shot in 15.6 µs.
10. **`ostm::delay_ms` is safe and fails loudly.** It panics until the time driver has started OSTM0, and `try_delay_ms` returns `false` instead of spinning on a stopped counter.
11. **The DMAC's register access moves onto the MMIO seam** (Task 1), so the typed-DMA and UART tests can run on the host. Some `raw` functions already used the seam and others dereferenced raw pointers, which crash on the host.
12. **Left on `gic::register` for 4c:** USB0 in five firmwares, the encoder and trigger-clock GPIO interrupts (IRQ0–7), SDHI, and `dmac::raw`'s own tables. 4b adds no typed GPIO-IRQ interrupts.

## Review Focus

1. **An SDK app that uses only the OLED, booted without app-loader** (for example via J-Link `dev-upload`), must still get a working display. RSPI0 is now brought up by whichever capability needs it first, and locking the bus before `bus::init` panics instead of driving an unconfigured RSPI0. Pinned by `the_bus_comes_up_in_32_bit_mode_with_the_oled_dma_aimed_at_spdr` (Task 3).
2. **UART receive bytes arrive in order across the 256-byte ring's wrap**, and a read on an empty ring returns `None` instead of re-reading old bytes. A PIC or MIDI stream crosses the wrap every 256 bytes. Pinned by `received_bytes_come_out_in_order_across_the_ring_wrap` (Task 4).
3. **The PIC's 31 250 → 200 000 bps switch keeps both DMA request enables.** Losing RIE would silence reception after the handshake; losing TIE would stall every DMA write. Pinned by `a_uart_is_configured_bound_and_moves_both_directions_by_dma` (Task 4), which also checks that a DMA write sends exactly its bytes.
4. **A delay before the time driver starts OSTM0** (a panic before `Board::new` finishes, or an app that supplies its own time driver) must report it, not spin forever on a stopped counter. Pinned by `a_delay_before_ostm0_runs_reports_it_instead_of_spinning` and `delay_ms_before_ostm0_runs_panics` (Task 6).
5. **An MTU2 prescaler the channel doesn't have** must be refused at construction, not run 64× too fast. A one-shot must also arm and disarm with exactly the old register sequence, because MIDI gate timing depends on it. Pinned by `a_prescaler_the_channel_lacks_is_refused`, `arming_starts_a_shot_from_zero_and_unmasks_it` and `disarming_stops_the_shot_and_masks_it` (Task 5).

---

## File Structure

| Path | Responsibility |
|---|---|
| `crates/rza1l-hal/src/interrupt.rs` | Typelevel SGI0–15, OSTMI0/1, TGI0A–TGI4A, ERI0–4, TXI0–4 |
| `crates/rza1l-hal/src/gic.rs` | `disable` through the MMIO seam |
| `crates/rza1l-hal/src/dmac.rs`, `dmac/raw.rs` | Register access on the seam; `Dma::steal`, `Dma::init_with_link_descriptor`, `Dma::channel_start`, `stop_all` |
| `crates/rza1l-hal/src/lib.rs` | `uncached(p)`, the DMA-coherent alias that is the identity on the host |
| `crates/rza1l-hal/src/rspi.rs` | `Instance`, owned `Rspi<'d>`; channel functions private |
| `crates/rza1l-hal/src/uart.rs` | `Instance`/`RxDmaInstance`, two handler types, `Uart`/`UartRx`/`UartTx`; channel functions private |
| `crates/rza1l-hal/src/mtu2.rs` | `Instance`, `OneShot<'d>`; channel functions private |
| `crates/rza1l-hal/src/ostm.rs`, `time_driver.rs` | Seam access; safe delays; `time_driver::init(OSTM0, OSTM1, Irqs)`, `stop()` |
| `crates/rza1l-hal/src/gpio.rs` | `Pin::new` → `Pin::steal` |
| `crates/deluge-bsp/src/board.rs` | Role aliases and fields for RSPI0, SCIF0/1, MTU2 2/3; the time driver's binding |
| `crates/deluge-bsp/src/bus.rs` | `init(shared_spi, oled_dma)`; holds `Rspi` + OLED `Dma`; host-compiled and tested |
| `crates/deluge-bsp/src/{cv_gate,oled}.rs` | Use the bus's `Rspi`/`Dma`; no RSPI bring-up of their own |
| `crates/deluge-bsp/src/uart.rs` | `init_midi`/`init_pic` from singletons; installed halves; MIDI/PIC read/write functions |
| `crates/deluge-bsp/src/pic.rs` | Goes through `crate::uart` instead of HAL channel numbers |
| `crates/deluge-bsp/src/{midi_gate,encoder}.rs` | Take their MTU2 channel; `OneShot` in a `OnceLock` for the ISR |
| `crates/deluge-sdk/src/parts.rs` (new) | The unclaimed-peripherals stash: `with`, `take` |
| `crates/deluge-sdk/src/{lib.rs,plat/device.rs}` | Stash at boot; capabilities take their parts |
| `firmwares/*`, `hw-tests/*` | Pass `board.*` fields (demo-firmware: `deluge::parts::with`) |

---

### Task 1: Escape hatches, the 4b interrupt types, and the DMAC on the seam

**Files:**
- Modify: `crates/rza1l-hal/src/dmac.rs`, `crates/rza1l-hal/src/dmac/raw.rs`, `crates/rza1l-hal/src/interrupt.rs`, `crates/rza1l-hal/src/gic.rs:402-411`, `firmwares/app-loader/src/main.rs:187-201`

**Interfaces:**
- Consumes: 4a's `dmac::{Channel, Dma, InterruptHandler, raw}`, `interrupt::typelevel`, `mmio::{Reg32, test}`.
- Produces: `pub unsafe fn Dma::steal<C: Channel>() -> Dma<'static>`; `pub unsafe fn Dma::init_with_link_descriptor(&mut self, descriptor: *const u32, dmars: u32)`; `pub unsafe fn Dma::channel_start(&mut self)`; `pub unsafe fn dmac::stop_all()`; `interrupt::typelevel::{SGI0..SGI15, OSTMI0, OSTMI1, TGI0A, TGI1A, TGI2A, TGI3A, TGI4A, ERI0..ERI4, TXI0..TXI4}`; `gic::disable` on the seam.

- [ ] **Step 1: Branch**

```bash
git checkout -b refactor/rza1-ownership-4b
```

- [ ] **Step 2: Write the failing tests**

In `crates/rza1l-hal/src/dmac.rs`, `mod typed_tests`, change the peripherals import to `use crate::peripherals::{DMA_CH0, DMA_CH4, DMA_CH7, DMA_CH9, DMA_CH10, DMA_CH13, DMA_CH15};` and append:

```rust
    #[test]
    fn a_stolen_channel_touches_no_register() {
        test::reset();
        let dma = unsafe { Dma::steal::<DMA_CH10>() };
        assert_eq!(dma.number(), 10);
        assert!(test::log().is_empty());
    }

    #[test]
    fn stop_all_stops_every_channel_as_raw_stop_does() {
        test::reset();
        for ch in 0..16 {
            unsafe { raw::stop(ch) };
        }
        let one_by_one = test::writes();
        test::reset();
        unsafe { stop_all() };
        assert_eq!(test::writes(), one_by_one);
    }

    #[test]
    fn an_owned_channel_starts_a_link_descriptor_as_raw_does() {
        let desc: [u32; 8] = [0, 0, 0, 0, 0x8110_0265, 0, 0, 0];
        test::reset();
        unsafe {
            raw::init_with_link_descriptor(13, desc.as_ptr(), 0x62);
            raw::channel_start(13);
        }
        let raw_writes = test::writes();

        let mut dma = unsafe { Dma::steal::<DMA_CH13>() };
        test::reset();
        unsafe {
            dma.init_with_link_descriptor(desc.as_ptr(), 0x62);
            dma.channel_start();
        }
        assert_eq!(test::writes(), raw_writes);
    }

    #[test]
    fn register_mode_setup_goes_through_the_seam() {
        test::reset();
        unsafe { raw::init_register_mode(4, 0x0020_026C, 0xE800_C804, 0x121) };
        // Channel 4's block is at 0xE820_0100: DCTRL_0_7; CHCFG, CHITVL, CHEXT; N0DA; CHCTRL
        // (SWRST | CLRTC); N0DA again; DMARS2's low half.
        assert_eq!(
            test::writes(),
            [
                (0xE820_0300, 0),
                (0xE820_012C, 0x0020_026C),
                (0xE820_0130, 0),
                (0xE820_0134, 0),
                (0xE820_0104, 0xE800_C804),
                (0xE820_0128, (1 << 3) | (1 << 6)),
                (0xE820_0104, 0xE800_C804),
                (0xFCFE_1008, 0x121),
            ]
        );
    }
```

In `crates/rza1l-hal/src/interrupt.rs`, `mod tests`, append:

```rust
    #[test]
    fn the_4b_interrupt_ids_match_the_hardware_manual() {
        assert_eq!(typelevel::SGI0::ID, 0);
        assert_eq!(typelevel::SGI15::ID, 15);
        assert_eq!(typelevel::OSTMI0::ID, 134);
        assert_eq!(typelevel::OSTMI1::ID, 135);
        assert_eq!(
            [
                typelevel::TGI0A::ID,
                typelevel::TGI1A::ID,
                typelevel::TGI2A::ID,
                typelevel::TGI3A::ID,
                typelevel::TGI4A::ID
            ],
            crate::mtu2::TGIA_IRQ
        );
        // SCIFn: ERI = 222 + 4n, TXI = 224 + 4n (RZ/A1L hardware manual, table A.2).
        assert_eq!(
            [
                typelevel::ERI0::ID,
                typelevel::ERI1::ID,
                typelevel::ERI2::ID,
                typelevel::ERI3::ID,
                typelevel::ERI4::ID
            ],
            [222, 226, 230, 234, 238]
        );
        assert_eq!(
            [
                typelevel::TXI0::ID,
                typelevel::TXI1::ID,
                typelevel::TXI2::ID,
                typelevel::TXI3::ID,
                typelevel::TXI4::ID
            ],
            [224, 228, 232, 236, 240]
        );
    }

    #[test]
    fn disabling_an_interrupt_sets_its_clear_enable_bit() {
        test::reset();
        unsafe { crate::gic::disable(73) };
        // GICD_ICENABLER2 (0xE820_1180 + 2*4), bit 73 - 64 = 9.
        assert_eq!(test::writes(), [(0xE820_1188, 1 << 9)]);
    }
```

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib dmac interrupt 2>&1 | tail -15`
Expected: compile errors: no `steal`, `stop_all`, `init_with_link_descriptor` or `channel_start` on `Dma`/`dmac`, and no `typelevel::SGI0`.

- [ ] **Step 4: The DMAC's registers go through the seam**

In `crates/rza1l-hal/src/dmac.rs`, make the three register helpers return seam handles:

```rust
#[inline]
fn ch_reg(ch: u8, off: usize) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(ch_base(ch) + off)
}

#[inline]
fn dctrl_reg(ch: u8) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(if ch < 8 { DCTRL_0_7 } else { DCTRL_8_15 })
}

#[inline]
fn dmars_reg(ch: u8) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(DMARS_BASE + (ch as usize / 2) * 4)
}
```

then convert their call sites mechanically:

```bash
sed -i -E 's/\.write_volatile\(/.write(/g; s/\.read_volatile\(\)/.read()/g; s/((ch_reg|dctrl_reg|dmars_reg)\([^)]*\)) as usize/\1.0/g' \
  crates/rza1l-hal/src/dmac.rs crates/rza1l-hal/src/dmac/raw.rs
```

Every `write_volatile`/`read_volatile` in these two files is on a register returned by those helpers; `descriptor.add(4).read()` in `raw::init_with_link_descriptor` is a plain memory read and doesn't match. Run `cargo build --target armv7-unknown-linux-gnueabihf -p rza1l-hal 2>&1 | grep -E "^error" | head`. Expected: nothing. If a site still names a `*mut u32` method, convert it to the handle's `.write(v)`/`.read()`.

- [ ] **Step 5: `gic::disable` goes through the seam**

In `crates/rza1l-hal/src/gic.rs`, replace `disable`'s two-line body tail (`let addr = …; addr.write_volatile(…)`) with:

```rust
        crate::mmio::write32(GICD_ICENABLER0 + 4 * (id >> 5) as usize, 1u32 << (id & 31));
```

- [ ] **Step 6: The interrupt types**

In `crates/rza1l-hal/src/interrupt.rs`, extend the `interrupts!` invocation:

```rust
    interrupts! {
        // Software-generated interrupts: an executor run from an SGI handler raises its own.
        SGI0 = 0, SGI1 = 1, SGI2 = 2, SGI3 = 3, SGI4 = 4, SGI5 = 5, SGI6 = 6, SGI7 = 7,
        SGI8 = 8, SGI9 = 9, SGI10 = 10, SGI11 = 11, SGI12 = 12, SGI13 = 13, SGI14 = 14, SGI15 = 15,
        // DMAC channel n's completion interrupt is GIC ID 41 + n.
        DMAINT0 = 41, DMAINT1 = 42, DMAINT2 = 43, DMAINT3 = 44,
        DMAINT4 = 45, DMAINT5 = 46, DMAINT6 = 47, DMAINT7 = 48,
        DMAINT8 = 49, DMAINT9 = 50, DMAINT10 = 51, DMAINT11 = 52,
        DMAINT12 = 53, DMAINT13 = 54, DMAINT14 = 55, DMAINT15 = 56,
        USB0 = 73, USB1 = 74,
        // OSTM channel n's interrupt.
        OSTMI0 = 134, OSTMI1 = 135,
        // MTU2 channel n's compare-match A.
        TGI0A = 139, TGI1A = 146, TGI2A = 150, TGI3A = 154, TGI4A = 159,
        // SCIF channel n's receive error and transmit-FIFO-empty.
        ERI0 = 222, TXI0 = 224, ERI1 = 226, TXI1 = 228, ERI2 = 230, TXI2 = 232,
        ERI3 = 234, TXI3 = 236, ERI4 = 238, TXI4 = 240,
    }
```

- [ ] **Step 7: The DMA escape hatches and link-descriptor methods**

In `crates/rza1l-hal/src/dmac.rs`, add to `impl<'d> Dma<'d>`, after `new`:

```rust
    /// Channel `C` without its singleton, for code that can't reach the channel's owner: a fault
    /// handler draining a transfer, or a loader stopping DMA before it hands over the chip.
    /// Touches no register and binds no interrupt.
    ///
    /// # Safety
    /// The channel's owner must not run while the stolen handle is in use (interrupts masked, or
    /// the owner gone for good).
    pub unsafe fn steal<C: Channel>() -> Dma<'static> {
        Dma { ch: C::NUMBER, _channel: PhantomData }
    }
```

and, after `init_register_mode_rx`:

```rust
    /// See [`raw::init_with_link_descriptor`].
    ///
    /// # Safety
    /// `descriptor` is the bus address of a 32-byte-aligned, 8-word link descriptor that stays
    /// valid, with every descriptor it links to, for as long as the channel runs.
    pub unsafe fn init_with_link_descriptor(&mut self, descriptor: *const u32, dmars: u32) {
        unsafe { raw::init_with_link_descriptor(self.ch, descriptor, dmars) }
    }

    /// See [`raw::channel_start`].
    ///
    /// # Safety
    /// [`init_with_link_descriptor`](Self::init_with_link_descriptor) has run, and its
    /// descriptors' buffers are valid and unaliased while the channel runs.
    pub unsafe fn channel_start(&mut self) {
        unsafe { raw::channel_start(self.ch) }
    }
```

After the `impl<'d> Dma<'d>` block, add:

```rust
/// Stop every channel and release its request route: [`Dma::stop`] for channels 0–15.
///
/// For a loader handing the chip to another program, where a circular channel (a UART receive
/// ring) would keep writing into memory the next program owns.
///
/// # Safety
/// Every channel's transfer ends; no `Dma` may be used afterwards.
pub unsafe fn stop_all() {
    for ch in 0..16 {
        unsafe { raw::stop(ch) };
    }
}
```

In `firmwares/app-loader/src/main.rs`, `quiesce_for_handoff`, replace the `for ch in 0..16u8 { rza1l_hal::dmac::raw::stop(ch); }` loop with `rza1l_hal::dmac::stop_all();` (keep the comment above it).

- [ ] **Step 8: Run the tests**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: `test result: ok.`, with the six new tests among the passes.

- [ ] **Step 9: Build check and commit**

Run the **Firmware build check** (Global Constraints) and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/dmac.rs crates/rza1l-hal/src/dmac/raw.rs crates/rza1l-hal/src/interrupt.rs \
  crates/rza1l-hal/src/gic.rs firmwares/app-loader/src/main.rs
git commit -m "feat(rza1l-hal): Dma::steal and dmac::stop_all for fault and hand-off code; SGI, OSTM, MTU2 and SCIF interrupt types; the DMAC on the MMIO seam"
```

---

### Task 2: The Board hands out RSPI0, SCIF0/1 and MTU2 2/3; the SDK keeps unclaimed parts

**Files:**
- Modify: `crates/deluge-bsp/src/board.rs`
- Create: `crates/deluge-sdk/src/parts.rs`
- Modify: `crates/deluge-sdk/src/lib.rs` (module list near `:95-107`; `run` near `:623-635`)

**Interfaces:**
- Consumes: `rza1l_hal::peripherals::{RSPI0, SCIF0, SCIF1, MTU2_2, MTU2_3}`.
- Produces: `deluge_bsp::board::{SharedSpi, MidiScif, PicScif, GateTimer, EncoderPollTimer}` (type aliases); `Board` fields `shared_spi: Peri<'static, SharedSpi>`, `midi_uart: Peri<'static, MidiScif>`, `pic_uart: Peri<'static, PicScif>`, `gate_timer: Peri<'static, GateTimer>`, `encoder_poll_timer: Peri<'static, EncoderPollTimer>`; `deluge::parts::{Parts, with}` (device-only) with `Parts`'s public `Option<Peri<'static, _>>` fields `shared_spi`, `oled_dma`, `midi_uart`, `midi_rx_dma`, `pic_uart`, `pic_rx_dma`, `pic_tx_dma`, `gate_timer`, `encoder_poll_timer`; `pub(crate) fn parts::take<T>(what: &str, f: impl FnOnce(&mut Parts) -> Option<T>) -> T`.

This task is plumbing: nothing takes the new fields until Tasks 3–5, so its check is that every firmware still builds and boots the same way.

- [ ] **Step 1: Roles and fields in `Board`**

In `crates/deluge-bsp/src/board.rs`, after the DMA aliases, add:

```rust
/// RSPI0: the OLED and the CV DAC share it ([`crate::bus`]).
pub type SharedSpi = RSPI0;
/// SCIF0: DIN MIDI ([`crate::uart::init_midi`]).
pub type MidiScif = SCIF0;
/// SCIF1: the PIC32 link ([`crate::uart::init_pic`]).
pub type PicScif = SCIF1;
/// MTU2 channel 2: the MIDI/CV gate one-shot ([`crate::midi_gate`]).
pub type GateTimer = MTU2_2;
/// MTU2 channel 3: the encoder pin poll ([`crate::encoder`]).
pub type EncoderPollTimer = MTU2_3;
```

In `struct Board`, after `usb1`, add:

```rust
    /// RSPI0, shared by the OLED and the CV DAC.
    pub shared_spi: Peri<'static, SharedSpi>,
    /// SCIF0, DIN MIDI.
    pub midi_uart: Peri<'static, MidiScif>,
    /// SCIF1, the PIC32 link.
    pub pic_uart: Peri<'static, PicScif>,
    /// MTU2 channel 2, the MIDI/CV gate one-shot.
    pub gate_timer: Peri<'static, GateTimer>,
    /// MTU2 channel 3, the encoder pin poll.
    pub encoder_poll_timer: Peri<'static, EncoderPollTimer>,
```

and in `Board::new`'s `Self { … }`, after `usb1: p.USB1,`:

```rust
            shared_spi: p.RSPI0,
            midi_uart: p.SCIF0,
            pic_uart: p.SCIF1,
            gate_timer: p.MTU2_2,
            encoder_poll_timer: p.MTU2_3,
```

- [ ] **Step 2: The parts stash**

Create `crates/deluge-sdk/src/parts.rs`:

```rust
//! The board's peripherals that no SDK capability has claimed yet.
//!
//! `#[deluge::app]` brings the board up before `setup` runs and keeps here the peripherals its
//! capabilities drive. Each capability takes its own the first time it is used: [`Deluge::midi`]
//! takes SCIF0 and its receive DMA channel, and so on. An app that drives one of these through
//! `deluge-bsp` itself takes it first, with [`with`]. The capability that needed it then panics,
//! naming the peripheral, instead of driving hardware the app owns.
//!
//! [`Deluge::midi`]: crate::Deluge::midi

use core::cell::RefCell;

use deluge_bsp::board::{
    EncoderPollTimer, GateTimer, MidiRxDma, MidiScif, OledDma, PicRxDma, PicScif, PicTxDma,
    SharedSpi,
};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use rza1l_hal::Peri;

/// The unclaimed peripherals. A `None` field has been taken.
pub struct Parts {
    /// RSPI0, for `deluge_bsp::bus::init` (the OLED and CV).
    pub shared_spi: Option<Peri<'static, SharedSpi>>,
    /// The OLED's DMA channel, for `deluge_bsp::bus::init`.
    pub oled_dma: Option<Peri<'static, OledDma>>,
    /// SCIF0, for `deluge_bsp::uart::init_midi`.
    pub midi_uart: Option<Peri<'static, MidiScif>>,
    /// MIDI's receive DMA channel, for `deluge_bsp::uart::init_midi`.
    pub midi_rx_dma: Option<Peri<'static, MidiRxDma>>,
    /// SCIF1, for `deluge_bsp::uart::init_pic`.
    pub pic_uart: Option<Peri<'static, PicScif>>,
    /// The PIC link's receive DMA channel, for `deluge_bsp::uart::init_pic`.
    pub pic_rx_dma: Option<Peri<'static, PicRxDma>>,
    /// The PIC link's transmit DMA channel, for `deluge_bsp::uart::init_pic`.
    pub pic_tx_dma: Option<Peri<'static, PicTxDma>>,
    /// MTU2 channel 2, for `deluge_bsp::midi_gate::setup`.
    pub gate_timer: Option<Peri<'static, GateTimer>>,
    /// MTU2 channel 3, for `deluge_bsp::encoder::irq_init`.
    pub encoder_poll_timer: Option<Peri<'static, EncoderPollTimer>>,
}

static PARTS: Mutex<CriticalSectionRawMutex, RefCell<Option<Parts>>> =
    Mutex::new(RefCell::new(None));

/// Keep `parts` for the capabilities and the app. The runtime calls this once, before `setup`.
pub(crate) fn stash(parts: Parts) {
    PARTS.lock(|p| *p.borrow_mut() = Some(parts));
}

/// Run `f` on the unclaimed peripherals, in a critical section. Take one with `Option::take`:
///
/// ```ignore
/// let timer = deluge::parts::with(|p| p.gate_timer.take()).expect("MTU2 channel 2 is free");
/// deluge_bsp::midi_gate::setup(timer);
/// ```
///
/// # Panics
/// Before `#[deluge::app]` has brought the board up, or if `f` calls `with` again.
pub fn with<R>(f: impl FnOnce(&mut Parts) -> R) -> R {
    PARTS.lock(|p| {
        f(p.borrow_mut()
            .as_mut()
            .expect("deluge::parts::with ran before #[deluge::app] brought the board up"))
    })
}

/// Take the peripheral `f` picks, for a capability.
///
/// # Panics
/// Naming `what`, if the app took it through [`with`].
pub(crate) fn take<T>(what: &str, f: impl FnOnce(&mut Parts) -> Option<T>) -> T {
    with(f).unwrap_or_else(|| panic!("{what} was taken by the app through deluge::parts"))
}
```

In `crates/deluge-sdk/src/lib.rs`, next to the other module declarations (after `mod pads;`), add:

```rust
/// The board's peripherals no capability has claimed yet (device-only).
#[cfg(target_os = "none")]
pub mod parts;
```

- [ ] **Step 3: The runtime stashes them before `setup`**

In `run` (`crates/deluge-sdk/src/lib.rs`), directly after `crate::usb_serial::stash(usb0);` and before `setup();`, add:

```rust
            // The rest of the board waits for the capability that drives it, or for the app to
            // take it through `deluge::parts`.
            crate::parts::stash(crate::parts::Parts {
                shared_spi: Some(board.shared_spi),
                oled_dma: Some(board.dma.oled),
                midi_uart: Some(board.midi_uart),
                midi_rx_dma: Some(board.dma.midi_rx),
                pic_uart: Some(board.pic_uart),
                pic_rx_dma: Some(board.dma.pic_rx),
                pic_tx_dma: Some(board.dma.pic_tx),
                gate_timer: Some(board.gate_timer),
                encoder_poll_timer: Some(board.encoder_poll_timer),
            });
```

- [ ] **Step 4: Build check and commit**

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib board 2>&1 | grep "test result"`
Expected: `ok`.

Run the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/deluge-bsp/src/board.rs crates/deluge-sdk/src/parts.rs crates/deluge-sdk/src/lib.rs
git commit -m "feat(bsp,sdk): Board hands out RSPI0, SCIF0/1 and MTU2 2/3 by role; the SDK keeps unclaimed parts in deluge::parts"
```

---

### Task 3: RSPI is an owned driver, and the bus owns it with the OLED's DMA channel

**Files:**
- Modify: `crates/rza1l-hal/src/rspi.rs`
- Modify: `crates/deluge-bsp/src/lib.rs:20-21`, `crates/deluge-bsp/src/bus.rs` (rewrite), `crates/deluge-bsp/src/cv_gate.rs`, `crates/deluge-bsp/src/oled.rs`
- Modify: `crates/deluge-sdk/src/plat/device.rs` (`oled_init_panel`, `cv_gate_init`)
- Modify: `firmwares/{app-loader,controller-firmware,msc-firmware,demo-firmware}/src/main.rs`, `hw-tests/{wp-probe,vbus-probe,uac-host-firmware}/src/main.rs`

**Interfaces:**
- Consumes: Task 1's `Dma::steal`; Task 2's `SharedSpi`, `Board::shared_spi`, `parts::take`.
- Produces: `rza1l_hal::rspi::{Instance, Rspi}` with `Rspi::new<T: Instance>(Peri<'d, T>, bit_rate: u32) -> Rspi<'d>`, `unsafe Rspi::steal<T: Instance>() -> Rspi<'static>`, and `&mut self` methods `configure_8bit`, `configure_32bit`, `send8(u8)`, `send32_blocking(u32)`, `wait_end`. `deluge_bsp::bus::{init(Peri<'static, SharedSpi>, Peri<'static, OledDma>), is_initialised() -> bool}`; `Rspi0Guard` becomes `MappedMutexGuard<'static, CriticalSectionRawMutex, Rspi0>`; `pub(crate) Rspi0::oled_dma(&mut self) -> &mut Dma<'static>`. `cv_gate::init` requires `bus::init` first.

- [ ] **Step 1: Write the failing HAL tests**

In `crates/rza1l-hal/src/rspi.rs`'s `mod tests`, delete `rspi_irq_ids` and replace `init_programs_master_mode_config` with:

```rust
    #[test]
    fn rspi_channels_know_their_index() {
        assert_eq!(<crate::peripherals::RSPI0 as Instance>::INDEX, 0);
        assert_eq!(<crate::peripherals::RSPI4 as Instance>::INDEX, 4);
    }

    /// `Rspi::new` programs the master-mode SPI config through the seam: the key registers end
    /// up with the expected values for a 10 MHz setup.
    #[test]
    fn new_programs_master_mode_config() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        use crate::mmio;
        mmio::test::reset();
        let _rspi = Rspi::new(unsafe { crate::peripherals::RSPI0::steal() }, 10_000_000);
        let b = base(0);

        assert_eq!(mmio::test::peek8(b + OFF_SPBR), 3, "baud divider for ~10 MHz");
        assert_eq!(mmio::test::peek8(b + OFF_SPDCR), SPDCR_32BIT, "32-bit longword frames");
        assert_eq!(mmio::test::peek8(b + OFF_SPBFCR), SPBFCR_INIT_32BIT);
        assert_eq!(mmio::test::peek16(b + OFF_SPCMD0), SPCMD0_32BIT, "32-bit mode-0 cmd");
        let spcr = mmio::test::peek8(b + OFF_SPCR);
        assert_eq!(spcr & SPCR_MSTR, SPCR_MSTR, "master mode");
        assert_eq!(spcr & SPCR_SPTIE, SPCR_SPTIE, "TX interrupt enabled");
        assert_eq!(spcr & SPCR_SPE, SPCR_SPE, "SPI enabled (R_RSPI_Start)");
    }

    #[test]
    fn a_stolen_rspi_touches_no_register() {
        crate::mmio::test::reset();
        let _rspi = unsafe { Rspi::steal::<crate::peripherals::RSPI0>() };
        assert!(crate::mmio::test::log().is_empty());
    }

    #[test]
    fn switching_to_8_bit_frames_rewrites_length_command_and_buffer_control() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        let mut rspi = Rspi::new(unsafe { crate::peripherals::RSPI1::steal() }, 4_000_000);
        crate::mmio::test::reset();
        rspi.configure_8bit();
        let b = base(1);
        assert_eq!(
            crate::mmio::test::writes(),
            [
                (b + OFF_SPDCR, SPDCR_8BIT as u32),
                (b + OFF_SPCMD0, SPCMD0_8BIT as u32),
                (b + OFF_SPBFCR, SPBFCR_8BIT as u32),
            ]
        );
    }
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib rspi 2>&1 | tail -5`
Expected: compile errors: no `Instance` and no `Rspi::new` taking a peripheral.

- [ ] **Step 2: The owned `Rspi`**

In `crates/rza1l-hal/src/rspi.rs`:

1. Make these functions private by dropping `pub` (bodies unchanged): `init`, `send32_blocking`, `configure_8bit`, `configure_32bit`, `send8`, `wait_end`.
2. Delete `irq_spri`, `register_irq`, `disable_rx_irq`, `enable_rx_irq`, `send32`, `reset_rx_buf`, the `SPEI_BASE`, `SPCR_SPRIE` and `SPBFCR_CV_TRANSFER` constants, `use crate::gic;`, and everything from `// ── embedded-hal SpiBus impls` down to (not including) `// ── Address tests`.
3. In the module docs, drop the "GIC interrupt IDs" table and the `[`configure_8bit`]` link (write "8-bit frames for the OLED"), and add a usage paragraph: "Build one with [`Rspi::new`] from its peripheral singleton; it is ready in 32-bit frames."
4. Before `// ── Address tests`, add:

```rust
// ── Owned driver ──────────────────────────────────────────────────────────────

use core::marker::PhantomData;

use crate::Peri;

mod sealed {
    pub trait Sealed {}
}

/// An RSPI channel singleton (`peripherals::RSPI0`–`RSPI4`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–4.
    const INDEX: u8;
}

macro_rules! instances {
    ($($rspi:ident = $n:literal),* $(,)?) => {$(
        impl sealed::Sealed for crate::peripherals::$rspi {}
        impl Instance for crate::peripherals::$rspi {
            const INDEX: u8 = $n;
        }
    )*};
}

instances!(RSPI0 = 0, RSPI1 = 1, RSPI2 = 2, RSPI3 = 3, RSPI4 = 4);

/// An RSPI channel as an SPI master, SPI mode 0, in 32-bit or 8-bit frames.
pub struct Rspi<'d> {
    ch: u8,
    _rspi: PhantomData<&'d mut ()>,
}

impl<'d> Rspi<'d> {
    /// Bring RSPI `T` up as an SPI master at no more than `bit_rate` Hz, in 32-bit frames.
    /// Mirrors `R_RSPI_Create` + `R_RSPI_Start` from the C firmware (apart from the SPI mode; see
    /// `SPCMD0_*`).
    ///
    /// # Panics
    /// If [`crate::clocks::init`] has not been called.
    pub fn new<T: Instance>(_rspi: Peri<'d, T>, bit_rate: u32) -> Self {
        // SAFETY: RSPI T is owned, so nothing else drives it.
        unsafe { init(T::INDEX, bit_rate) };
        Self { ch: T::INDEX, _rspi: PhantomData }
    }

    /// RSPI `T` without its singleton, for code that can't reach the owner: a panic handler
    /// drawing to a display. Touches no register.
    ///
    /// # Safety
    /// [`Rspi::new`] has run for `T`, and its owner doesn't run while the stolen handle is in use
    /// (a panic handler with interrupts masked).
    pub unsafe fn steal<T: Instance>() -> Rspi<'static> {
        Rspi { ch: T::INDEX, _rspi: PhantomData }
    }

    /// Switch to 8-bit frames: SPDCR, SPCMD0, and SPBFCR (which also resets the receive FIFO).
    pub fn configure_8bit(&mut self) {
        // SAFETY: owned channel.
        unsafe { configure_8bit(self.ch) }
    }

    /// Switch to 32-bit frames.
    pub fn configure_32bit(&mut self) {
        // SAFETY: owned channel.
        unsafe { configure_32bit(self.ch) }
    }

    /// Queue one byte in 8-bit mode. Doesn't wait for it to shift out: call
    /// [`wait_end`](Self::wait_end) after the last.
    pub fn send8(&mut self, byte: u8) {
        // SAFETY: owned channel.
        unsafe { send8(self.ch, byte) }
    }

    /// Send one word in 32-bit mode and wait until it has shifted out.
    pub fn send32_blocking(&mut self, word: u32) {
        // SAFETY: owned channel.
        unsafe { send32_blocking(self.ch, word) }
    }

    /// Wait until the shift register is idle (TEND).
    pub fn wait_end(&mut self) {
        // SAFETY: owned channel.
        unsafe { wait_end(self.ch) }
    }
}
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib rspi 2>&1 | grep "test result"`
Expected: `ok`.

- [ ] **Step 3: Write the failing bus test**

In `crates/deluge-bsp/src/lib.rs`, delete the `#[cfg(target_os = "none")]` line above `pub mod bus;`. The bus becomes host-compiled so it can be tested. Its file-level `#![cfg(target_os = "none")]` goes in Step 4.

Append to `crates/deluge-bsp/src/bus.rs`:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use rza1l_hal::mmio::test;
    use rza1l_hal::peripherals::{DMA_CH4, RSPI0};

    #[test]
    fn the_bus_comes_up_in_32_bit_mode_with_the_oled_dma_aimed_at_spdr() {
        rza1l_hal::clocks::init(crate::system::CLOCKS);
        test::reset();

        assert!(!is_initialised());
        // Before init there is no bus: locking one must panic, not hand out an unconfigured RSPI0.
        assert!(std::panic::catch_unwind(|| try_lock_rspi0().is_some()).is_err());

        init(unsafe { RSPI0::steal() }, unsafe { DMA_CH4::steal() });
        assert!(is_initialised());
        let bus = try_lock_rspi0().expect("nobody holds the bus");
        assert_eq!(bus.mode, Mode::Bits32);
        // DMA channel 4 (registers at 0xE820_0100): N0DA is SPDR's low byte, CHCFG the OLED's.
        assert_eq!(test::peek32(0xE820_0104), crate::oled::RSPI0_SPDR_LL);
        assert_eq!(test::peek32(0xE820_012C), crate::oled::OLED_CHCFG);
    }
}
```

In `crates/deluge-bsp/src/oled.rs`, make `RSPI0_SPDR_LL`, `OLED_DMA_DMARS` and `OLED_CHCFG` `pub(crate) const`.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib bus 2>&1 | tail -5`
Expected: compile errors: no `init` or `is_initialised` in `bus`.

- [ ] **Step 4: The bus owns RSPI0 and the OLED DMA**

Replace everything in `crates/deluge-bsp/src/bus.rs` above the new `mod tests` with the following. The module doc is the old one minus `#![cfg(target_os = "none")]`, plus a paragraph about `init`.

```rust
//! Shared-bus arbitration for RSPI0.
//!
//! RSPI0 is physically shared between two consumers that need *mutually
//! incompatible* frame modes:
//!
//! * the **OLED** (`oled`) — 8-bit frames streamed via DMAC channel 4, and
//! * the **CV DAC** (`cv_gate`) — 32-bit blocking writes to a MAX5136.
//!
//! Driving either without coordinating corrupts the other's transfer, and
//! switching frame mode underneath an in-flight transfer is silently wrong.
//!
//! The bus is a single owned resource behind an async-aware mutex. Consumers
//! `lock_rspi0().await`, drive the bus through the returned guard, and release
//! it by dropping the guard. The guard may be held across `.await` (e.g. while
//! an OLED DMA transfer completes); other consumers wait. The guard also
//! *tracks the frame mode*, so `enter_8bit` / `enter_32bit` reconfigure only
//! when the mode actually changes.
//!
//! [`init`] brings RSPI0 up and takes the OLED's DMA channel with it, since an OLED frame is an
//! RSPI0 transmit. Until then there is no bus, and locking it panics.
//!
//! See the Advanced developer guide (`docs/advanced-guide.md`, §7 — *Dropping
//! down to the BSP & HAL*) for the RSPI0 arbitration design.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::{MappedMutexGuard, Mutex, MutexGuard};
use rza1l_hal::Peri;
use rza1l_hal::dmac::{self, Dma};
use rza1l_hal::rspi::Rspi;

use crate::board::{OledDma, SharedSpi};

/// RSPI0's SCK rate, shared by the CV DAC and the OLED.
///
/// The OLED sets the limit, not the DAC (good to 20 MHz). The OEL9M0087-W-E datasheet allows 10 MHz (a 100 ns
/// cycle), but the 2026 batch of these panels, the same model, glitches at 8.5 MHz and above; 4 MHz is safe.
const RATE_HZ: u32 = 4_000_000;

rza1l_hal::bind_interrupts!(struct Irqs {
    DMAINT4 => dmac::InterruptHandler<OledDma>;
});

/// RSPI0 frame mode, tracked by the bus owner so consumers don't reconfigure
/// the channel blindly on every transfer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Mode not known (a stolen bus): the next `enter_*` always reconfigures.
    Unknown,
    /// 8-bit frames (OLED).
    Bits8,
    /// 32-bit frames (CV DAC).
    Bits32,
}

/// Exclusive-access token for RSPI0, and with it the OLED's DMA channel.
///
/// The only way to obtain one is [`lock_rspi0`] / [`try_lock_rspi0`] (or, in a
/// dying context, [`steal_rspi0`]). Holding it guarantees no other consumer can
/// touch RSPI0.
pub struct Rspi0 {
    mode: Mode,
    rspi: Rspi<'static>,
    oled_dma: Dma<'static>,
}

impl Rspi0 {
    /// Ensure RSPI0 is in 8-bit frame mode (OLED). Reconfigures only if the
    /// channel is not already in 8-bit mode.
    #[inline]
    pub fn enter_8bit(&mut self) {
        if self.mode != Mode::Bits8 {
            self.rspi.configure_8bit();
            self.mode = Mode::Bits8;
        }
    }

    /// Re-apply the 8-bit transfer config (SPDCR/SPCMD0/SPBFCR including the
    /// RX-FIFO reset) **unconditionally**, regardless of the cached mode.
    ///
    /// The cached [`enter_8bit`](Self::enter_8bit) skips reconfiguration when the
    /// channel is already in 8-bit mode — fine for SPDCR/SPCMD0, which persist,
    /// but not for the RX FIFO: a full-duplex OLED DMA clocks 768 dummy bytes
    /// back into it every frame, and `SPBFCR.RXRST` must be re-asserted each time
    /// or the next transfer mis-paces (visible as a horizontal "stretch"). The C
    /// firmware (rza1) likewise rewrites SPBFCR every frame.
    /// Call once per OLED frame before arming the DMA.
    #[inline]
    pub fn reconfigure_8bit(&mut self) {
        self.rspi.configure_8bit();
        self.mode = Mode::Bits8;
    }

    /// Ensure RSPI0 is in 32-bit frame mode (CV DAC). Reconfigures only if the
    /// channel is not already in 32-bit mode.
    #[inline]
    pub fn enter_32bit(&mut self) {
        if self.mode != Mode::Bits32 {
            self.rspi.configure_32bit();
            self.mode = Mode::Bits32;
        }
    }

    /// Send one byte in 8-bit mode. Call [`enter_8bit`](Self::enter_8bit) first.
    #[inline]
    pub fn send8(&mut self, byte: u8) {
        self.rspi.send8(byte);
    }

    /// Blocking 32-bit send. Call [`enter_32bit`](Self::enter_32bit) first.
    #[inline]
    pub fn send32_blocking(&mut self, word: u32) {
        self.rspi.send32_blocking(word);
    }

    /// Poll until the shift register has drained (TEND=1). Call before
    /// de-asserting chip-select so the peripheral receives the full frame.
    #[inline]
    pub fn wait_end(&mut self) {
        self.rspi.wait_end();
    }

    /// The OLED's DMA channel, which transmits on RSPI0: held with the bus, so a frame's DMA
    /// can't run while a CV write drives RSPI0.
    pub(crate) fn oled_dma(&mut self) -> &mut Dma<'static> {
        &mut self.oled_dma
    }
}

/// The bus, once [`init`] has run, behind an async mutex.
///
/// `CriticalSectionRawMutex` (not an async-only raw mutex) is deliberate: it
/// lets [`try_lock_rspi0`] work outside the executor (boot).
static RSPI0: Mutex<CriticalSectionRawMutex, Option<Rspi0>> = Mutex::new(None);

/// Type of the guard returned by [`lock_rspi0`].
pub type Rspi0Guard = MappedMutexGuard<'static, CriticalSectionRawMutex, Rspi0>;

/// Bring RSPI0 up as the shared bus: an SPI master at 4 MHz in 32-bit frames, with the OLED's
/// DMA channel aimed at its transmit register.
///
/// Firmware calls it once with the [`crate::Board`]'s `shared_spi` and `dma.oled`, before
/// [`crate::cv_gate::init`] or [`crate::oled::init`]. The SDK calls it the first time an app
/// uses either.
pub fn init(spi: Peri<'static, SharedSpi>, oled_dma: Peri<'static, OledDma>) {
    let rspi = Rspi::new(spi, RATE_HZ);
    let mut oled_dma = Dma::new(oled_dma, Irqs);
    // SAFETY: every OLED transfer writes SPDR's low byte, the transmit register of the RSPI0
    // this bus owns.
    unsafe {
        oled_dma.init_register_mode(
            crate::oled::OLED_CHCFG,
            crate::oled::RSPI0_SPDR_LL,
            crate::oled::OLED_DMA_DMARS,
        )
    };
    let bus = Rspi0 { mode: Mode::Bits32, rspi, oled_dma };
    match RSPI0.try_lock() {
        // RSPI0 is owned once, so the slot is empty.
        Ok(mut slot) => *slot = Some(bus),
        Err(_) => unreachable!("RSPI0's bus is locked before it exists"),
    }
}

/// Whether [`init`] has run.
pub fn is_initialised() -> bool {
    // Only an initialised bus can be held: locking an empty one panics, which releases it.
    RSPI0.try_lock().map_or(true, |slot| slot.is_some())
}

fn live(guard: MutexGuard<'static, CriticalSectionRawMutex, Option<Rspi0>>) -> Rspi0Guard {
    MutexGuard::map(guard, |slot| {
        slot.as_mut()
            .expect("RSPI0 isn't up: call deluge_bsp::bus::init first")
    })
}

/// Acquire exclusive RSPI0 access, awaiting if another consumer holds it.
///
/// The guard may be held across `.await` (e.g. across an OLED DMA completion).
///
/// # Panics
/// If [`init`] hasn't run.
pub async fn lock_rspi0() -> Rspi0Guard {
    live(RSPI0.lock().await)
}

/// Try to acquire RSPI0 without awaiting; returns `None` if it is already held.
///
/// Intended for single-threaded startup (`cv_gate::init`) before the executor
/// runs, where contention is impossible and `.await` is unavailable.
///
/// # Panics
/// If [`init`] hasn't run.
pub fn try_lock_rspi0() -> Option<Rspi0Guard> {
    RSPI0.try_lock().ok().map(live)
}

/// Fabricate an RSPI0 token, **bypassing the mutex**.
///
/// For use only when the normal locking discipline cannot apply — i.e. a panic
/// handler that must draw to the OLED but cannot `.await` and may run with the
/// executor dead. Safe there because panic is single-threaded with interrupts
/// masked. Never call this from normal code.
///
/// # Safety
/// [`init`] has run, and no other RSPI0 consumer is or will be active for the lifetime of the
/// returned token (e.g. inside a panic handler with IRQs masked).
pub unsafe fn steal_rspi0() -> Rspi0 {
    // SAFETY: the caller's contract.
    unsafe {
        Rspi0 {
            mode: Mode::Unknown,
            rspi: Rspi::steal::<SharedSpi>(),
            oled_dma: Dma::steal::<OledDma>(),
        }
    }
}
```

- [ ] **Step 5: `cv_gate` and `oled` use the bus**

In `crates/deluge-bsp/src/cv_gate.rs`:
- `use rza1l_hal::{gpio, rspi};` → `use rza1l_hal::gpio;`. Delete the `SPI_CH` and `SPI_RATE_HZ` constants; their doc comment moved to `bus::RATE_HZ`.
- In the device `init`, change the doc's "**Must be called after**" list to "after [`crate::bus::init`] (RSPI0) and `rza1l_hal::time_driver::init` (which `delay_ms` needs)". Replace from `// ---- Initialise RSPI0 as 4 MHz SPI master` through the end of the zeroing loop with:

```rust
        // ---- RSPI0 is the bus's: hold it for the whole linearity sequence ------
        let mut bus = crate::bus::try_lock_rspi0()
            .expect("cv_gate::init runs at startup, before anything else locks RSPI0");
        bus.enter_32bit();

        // ---- MAX5136 linearity initialisation -----------------------------------
        // Step 1: LIN=1 (linear ramp mode)
        log::debug!("cv_gate: MAX5136 LIN=1");
        gpio::write(CS_PORT, CS_PIN, false);
        bus.send32_blocking(max5136::LINEARITY_ON_U32);
        gpio::write(CS_PORT, CS_PIN, true);

        // Step 2: wait ≥ 10 ms for internal capacitor to charge
        ostm::delay_ms(LIN_DELAY_MS);

        // Step 3: LIN=0 (return to normal mode)
        gpio::write(CS_PORT, CS_PIN, false);
        bus.send32_blocking(max5136::LINEARITY_OFF_U32);
        gpio::write(CS_PORT, CS_PIN, true);
        log::debug!("cv_gate: MAX5136 linearity init done");

        // ---- Zero all CV outputs ------------------------------------------------
        for ch in 0..NUM_CV_CHANNELS as u8 {
            cv_set_blocking(&mut bus, ch, 0);
        }
        log::debug!("cv_gate: CV outputs zeroed");
```

- Replace `cv_set_blocking` with a private, bus-taking version:

```rust
/// Write a voltage code to CV channel `ch` on a bus the caller already holds (in 32-bit mode).
#[cfg(target_os = "none")]
fn cv_set_blocking(bus: &mut crate::bus::Rspi0, ch: u8, value: u16) {
    let Some(word) = dac_word(ch, value) else {
        log::warn!("cv_gate: ignoring write to unwired CV channel {ch}");
        return;
    };
    // SAFETY: CS is cv_gate's GPIO; the bus is held.
    unsafe { gpio::write(CS_PORT, CS_PIN, false) };
    bus.send32_blocking(word);
    // SAFETY: as above.
    unsafe { gpio::write(CS_PORT, CS_PIN, true) };
}
```

- In the host test `init_and_gate_set_do_not_panic_on_host`, delete the `cv_set_blocking` loop and the `cv_set_blocking/` part of its doc comment.

In `crates/deluge-bsp/src/oled.rs`:
- In `init`, delete the whole `// ---- 1b. Set up DMAC channel 4 for OLED SPI TX` block (`bus::init` does it).
- In `send_frame`, replace `dmac::raw::start_transfer(OLED_DMA_CH, src_uncached, FRAME_BYTES as u32);` with `bus.oled_dma().start_transfer(src_uncached, FRAME_BYTES as u32);`, and `dmac::raw::wait_transfer_complete(OLED_DMA_CH).await;` with `bus.oled_dma().wait_transfer_complete().await;`.
- Delete `use rza1l_hal::dmac;`.
- `OLED_DMA_CH` is now read only by `board`'s test. Mark it `#[cfg(test)]` so device builds don't warn that it is dead.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib 2>&1 | grep -E "test result|FAILED|panicked at"`
Expected: `ok`. The one `panicked at` line is the deliberate panic in the bus test, `RSPI0 isn't up: call deluge_bsp::bus::init first`.

- [ ] **Step 6: Bring the bus up where RSPI0 was brought up**

In each of `firmwares/app-loader/src/main.rs`, `firmwares/controller-firmware/src/main.rs`, `firmwares/msc-firmware/src/main.rs`, `hw-tests/wp-probe/src/main.rs`, `hw-tests/vbus-probe/src/main.rs` and `hw-tests/uac-host-firmware/src/main.rs`:
- `let _board = deluge_bsp::Board::new(…)` → `let board = deluge_bsp::Board::new(…)`.
- Directly before the `cv_gate::init()` line (`unsafe { cv_gate::init() };` or `unsafe { deluge_bsp::cv_gate::init() };`) add `deluge_bsp::bus::init(board.shared_spi, board.dma.oled);`. Adjust the comment above it to say the bus brings RSPI0 up and `cv_gate::init` runs the DAC's linearity sequence on it.

In `firmwares/demo-firmware/src/main.rs` `setup()`, replace `unsafe { cv_gate::init() };` and the comment above it with:

```rust
    // RSPI0 is shared by the OLED (8-bit) and the CV DAC (32-bit), arbitrated by
    // deluge_bsp::bus. This firmware drives both itself, so it takes them from the SDK.
    let (spi, oled_dma) = deluge::parts::with(|p| (p.shared_spi.take(), p.oled_dma.take()));
    deluge_bsp::bus::init(spi.expect("RSPI0 is free"), oled_dma.expect("the OLED DMA channel is free"));
    unsafe { cv_gate::init() };
```

In `crates/deluge-sdk/src/plat/device.rs`, add above `oled_init_panel`:

```rust
/// Bring up RSPI0, which the OLED and the CV DAC share, the first time either needs it, unless
/// the app already did.
fn ensure_shared_spi() {
    if deluge_bsp::bus::is_initialised() {
        return;
    }
    let spi = crate::parts::take("RSPI0 (the OLED and CV bus)", |p| p.shared_spi.take());
    let dma = crate::parts::take("the OLED DMA channel", |p| p.oled_dma.take());
    deluge_bsp::bus::init(spi, dma);
}
```

`oled_init_panel` becomes `ensure_shared_spi(); oled::init().await;`. In `cv_gate_init`, call `ensure_shared_spi();` before the `unsafe { deluge_bsp::cv_gate::init() };` line, and change its SAFETY comment's first sentence to "runs once, after the bus is up."

- [ ] **Step 7: Suite, build check, commit**

Run: `tools/test.sh > /tmp/claude-1000/p4b-t3.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

Run the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/rspi.rs crates/deluge-bsp/src crates/deluge-sdk/src/plat/device.rs firmwares hw-tests
git commit -m "feat(rza1l-hal,bsp): Rspi is built from its singleton; the bus owns RSPI0 and the OLED's DMA, and brings RSPI0 up for whichever needs it first"
```

---

### Task 4: The UART is an owned driver with receive and transmit halves

**Files:**
- Modify: `crates/rza1l-hal/Cargo.toml` (host dev-dependencies), `crates/rza1l-hal/src/lib.rs`, `crates/rza1l-hal/src/uart.rs`
- Modify: `crates/deluge-bsp/src/uart.rs` (rewrite), `crates/deluge-bsp/src/pic.rs`
- Modify: `crates/deluge-sdk/src/plat/device.rs` (`pic_ensure_started`, `pic_pump`, `midi_init`)
- Modify: the `main.rs` and `tasks/pic.rs` of `firmwares/{app-loader,controller-firmware,msc-firmware,demo-firmware}` and `hw-tests/{wp-probe,vbus-probe,uac-host-firmware,sd-bench}` (app-loader's PIC reader is in `main.rs`)

**Interfaces:**
- Consumes: Task 1's `Dma::{init_with_link_descriptor, channel_start}`, `typelevel::{ERI0.., TXI0..}`; Task 2's `MidiScif`, `PicScif`, `Board::{midi_uart, pic_uart}`, `parts::take`.
- Produces: `rza1l_hal::uart::{Instance, RxDmaInstance, ErrorInterruptHandler<T>, TxInterruptHandler<T>, Uart, UartRx, UartTx}`:
  - `Uart::new<T: RxDmaInstance>(Peri<'d, T>, rx_dma: Dma<'d>, tx_dma: Option<Dma<'d>>, irq: impl Binding<T::ErrorInterrupt, ErrorInterruptHandler<T>> + Binding<T::TxInterrupt, TxInterruptHandler<T>> + Copy + 'd, baud: u32) -> Uart<'d>` and `Uart::split(self) -> (UartRx<'d>, UartTx<'d>)`.
  - `UartRx`: `async fn read_byte(&mut self) -> u8`, `fn try_read(&mut self) -> Option<u8>`.
  - `UartTx`: `async fn write(&self, &[u8])`, `fn try_write(&self, &[u8]) -> usize`, `fn tx_free(&self) -> usize`, `fn set_baud(&self, u32)`.
  - `embedded_io_async::Read` for `UartRx` and `Write` for `UartTx`.
  - `pub(crate) fn rza1l_hal::uncached<T>(*mut T) -> *mut T`.
- Produces (BSP): `uart::init_midi(Peri<'static, MidiScif>, Peri<'static, MidiRxDma>, baud: u32)`, `uart::init_pic(Peri<'static, PicScif>, Peri<'static, PicRxDma>, Peri<'static, PicTxDma>, baud: u32)`, and the existing `write_midi`, `read_midi_byte`, `try_read_midi`, `try_write_midi`, `midi_write_space`. New: `write_pic`, `read_pic_byte`, `try_read_pic`, `try_write_pic`, `set_pic_baud`. The re-export `pub use rza1l_hal::uart::{read_byte, write_bytes}` goes.

- [ ] **Step 1: The UART's registers go through the seam**

In `crates/rza1l-hal/src/uart.rs`, replace the four register helpers' bodies:

```rust
#[inline(always)]
unsafe fn rr16(addr: usize) -> u16 {
    unsafe { crate::mmio::read16(addr) }
}

#[inline(always)]
unsafe fn wr16(addr: usize, val: u16) {
    unsafe { crate::mmio::write16(addr, val) }
}

#[inline(always)]
unsafe fn rr8(addr: usize) -> u8 {
    unsafe { crate::mmio::read8(addr) }
}

#[inline(always)]
unsafe fn wr8(addr: usize, val: u8) {
    unsafe { crate::mmio::write8(addr, val) }
}
```

In `crates/rza1l-hal/src/lib.rs`, after `UNCACHED_MIRROR_OFFSET`, add:

```rust
/// `p`'s uncached alias: write DMA-shared memory through it so the DMAC sees the bytes without a
/// cache clean, and hand it to the DMAC as the bus address. On the host, where tests run, memory
/// has no alias, so it is `p`.
#[inline]
pub(crate) fn uncached<T>(p: *mut T) -> *mut T {
    #[cfg(target_os = "none")]
    return (p as usize + UNCACHED_MIRROR_OFFSET) as *mut T;
    #[cfg(not(target_os = "none"))]
    return p;
}
```

In `crates/rza1l-hal/Cargo.toml`, under `[target.'cfg(not(target_os = "none"))'.dev-dependencies]`, add `embassy-futures = { workspace = true }`.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib uart 2>&1 | grep "test result"`
Expected: `ok` (the existing address and baud tests).

- [ ] **Step 2: Write the failing UART tests**

Append to `crates/rza1l-hal/src/uart.rs`:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod typed_tests {
    use super::*;
    use crate::dmac::{self, Dma};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH10, DMA_CH12, DMA_CH13, SCIF0, SCIF1};

    crate::bind_interrupts!(struct Irqs {
        ERI0 => ErrorInterruptHandler<SCIF0>;
        TXI0 => TxInterruptHandler<SCIF0>;
        ERI1 => ErrorInterruptHandler<SCIF1>;
        TXI1 => TxInterruptHandler<SCIF1>;
        DMAINT10 => dmac::InterruptHandler<DMA_CH10>;
        DMAINT12 => dmac::InterruptHandler<DMA_CH12>;
        DMAINT13 => dmac::InterruptHandler<DMA_CH13>;
    });

    /// DMAC channel `ch`'s register at `off`.
    fn chreg(ch: u8, off: usize) -> usize {
        0xE820_0000 + ch as usize * 64 + (if ch >= 8 { 0x200 } else { 0 }) + off
    }

    #[test]
    fn a_uart_is_configured_bound_and_moves_both_directions_by_dma() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let rx_dma = Dma::new(unsafe { DMA_CH13::steal() }, Irqs);
        let tx_dma = Dma::new(unsafe { DMA_CH10::steal() }, Irqs);
        let (rx, tx) =
            Uart::new(unsafe { SCIF0::steal() }, rx_dma, Some(tx_dma), Irqs, 31_250).split();

        let b = base(0);
        assert_eq!(test::peek8(b + SCBRR), 131, "31 250 bps");
        assert_eq!(test::peek16(b + SCEMR), 0x0080, "double-speed baud generator");
        assert_eq!(test::peek16(b + SCSCR), TIE | RIE | RE | TE, "both requests go to the DMAC");
        assert!(crate::gic::handler(222).is_some(), "ERI0 bound");
        assert!(crate::gic::handler(224).is_some(), "TXI0 bound");
        // DMARS5 holds channel 10 (low half: SCIF0 transmit); DMARS6 channel 13 (high: receive).
        assert_eq!(test::peek32(0xFCFE_1014) & 0xFFFF, 0x61);
        assert_eq!(test::peek32(0xFCFE_1018) >> 16, 0x62);
        // The receive descriptor (NXLA): SCFRDR into the ring, linked back to itself.
        let desc = test::peek32(chreg(13, 0x38)) as *const u32;
        unsafe {
            assert_eq!(*desc.add(1), (b + SCFRDR) as u32);
            assert_eq!(*desc.add(2), rx.base);
            assert_eq!(*desc.add(7), desc as u32);
        }
        assert_eq!(rx.read, rx.base, "the ring starts empty");

        // A write is one DMA transfer from the staging buffer; the DMAC's TC flag (CHSTAT bit 6),
        // set here as the hardware would, completes it.
        test::poke32(chreg(10, 0x24), 1 << 6);
        embassy_futures::block_on(tx.write(b"hello"));
        assert_eq!(test::peek32(chreg(10, 0x08)), 5, "N0TB: the byte count");
        let staged = test::peek32(chreg(10, 0x00)) as *const u8;
        assert_eq!(unsafe { core::slice::from_raw_parts(staged, 5) }, b"hello");

        // A baud change keeps both DMA requests enabled.
        tx.set_baud(200_000);
        assert_eq!(test::peek8(b + SCBRR), scbrr(crate::clocks::TEST_CLOCKS.p1_hz(), 200_000));
        assert_eq!(test::peek16(b + SCSCR), TIE | RIE | RE | TE);
    }

    #[test]
    fn received_bytes_come_out_in_order_across_the_ring_wrap() {
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let rx_dma = Dma::new(unsafe { DMA_CH12::steal() }, Irqs);
        let (mut rx, _tx) = Uart::new(unsafe { SCIF1::steal() }, rx_dma, None, Irqs, 31_250).split();
        let ring = rx.base as *mut u8;
        let crda = chreg(12, 0x1C);

        // Nothing received: the DMAC's write position is still the ring's start.
        test::poke32(crda, rx.base);
        assert_eq!(rx.try_read(), None);

        // The DMAC fills the ring up to its last byte...
        for i in 0..DMA_RX_BUF_SIZE {
            unsafe { ring.add(i).write(i as u8) };
        }
        test::poke32(crda, rx.base + DMA_RX_BUF_SIZE as u32 - 1);
        for i in 0..DMA_RX_BUF_SIZE - 1 {
            assert_eq!(rx.try_read(), Some(i as u8));
        }
        assert_eq!(rx.try_read(), None);

        // ...then writes the last byte and wraps round to overwrite the first.
        unsafe { ring.write(0xAA) };
        test::poke32(crda, rx.base + 1);
        assert_eq!(rx.try_read(), Some(0xFF));
        assert_eq!(rx.try_read(), Some(0xAA));
        assert_eq!(rx.try_read(), None);
    }
}
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib uart 2>&1 | tail -5`
Expected: compile errors: no `Uart`, `ErrorInterruptHandler` or `TxInterruptHandler`.

- [ ] **Step 3: The owned UART**

In `crates/rza1l-hal/src/uart.rs`:

1. **Delete:** `use crate::UNCACHED_MIRROR_OFFSET;`; the statics `DMA_RX_DMACH`, `DMA_RX_READ`, `DMA_RX_BASE`, `DMA_RX_ACTIVE`, `DMA_TX_DMACH`, `DMA_TX_ACTIVE` and `DMA_TX_LOCK`; `register_txi_for`, `init_dma_rx`, `init_dma_tx`; the ten `eriN_handler`/`txiN_handler` functions, `type HandlerFn` and `ERI_HANDLERS`/`TXI_HANDLERS`; `read_byte`, `read_byte_dma`, `try_read_dma`, `write_bytes`, `try_write_fifo`, `tx_fifo_free`; and `ScifUart` with its three impls.
2. **Rename and make private:** `pub unsafe fn init(ch: usize, baud_rate: u32)` → `unsafe fn configure(ch: usize, baud_rate: u32)` (body unchanged; drop its sentence about `register_txi_for`). `pub unsafe fn set_baud(ch, baud_rate)` → `unsafe fn set_baud_regs(ch: usize, baud_rate: u32)`, with `if DMA_RX_ACTIVE[ch]` → `if RX_DMA_ACTIVE[ch].load(Ordering::Relaxed)` and `if DMA_TX_ACTIVE[ch]` → `if TX_DMA_ACTIVE[ch].load(Ordering::Relaxed)`.
3. **Comments on the kept statics:** above `DMA_RX_BUF`/`DMA_RX_DESC`: `// One receive ring per RxDmaInstance (SCIF0, SCIF1). SAFETY: ring n is touched only by the UartRx of the SCIF whose RING is n, and each SCIF is owned once.` Above `DMA_TX_BUF`: `// SAFETY: SCIF n's staging buffer, touched only by its UartTx under that UartTx's lock.` In the RXI `NOTE` comment, `[`init_dma_rx`]` → `the DMA ring ([`UartRx`])`.
4. **Imports:** add `use core::marker::PhantomData;`, `use core::sync::atomic::{AtomicBool, Ordering};`, `use crate::Peri;`, `use crate::dmac::Dma;`, `use crate::interrupt::{self, Binding, Handler, Interrupt};`.
5. **Module docs:** replace the `## Usage` example with:

```rust
//! ## Usage
//! ```ignore
//! rza1l_hal::bind_interrupts!(struct Irqs {
//!     ERI0 => uart::ErrorInterruptHandler<SCIF0>;
//!     TXI0 => uart::TxInterruptHandler<SCIF0>;
//!     DMAINT13 => dmac::InterruptHandler<DMA_CH13>;
//! });
//! // Mux the board's pins first, so the receive ring never samples an unconnected pin.
//! unsafe { gpio::set_pin_mux(6, 15, 5); gpio::set_pin_mux(6, 14, 5); }
//! let rx_dma = dmac::Dma::new(p.DMA_CH13, Irqs);
//! let (mut rx, tx) = uart::Uart::new(p.SCIF0, rx_dma, None, Irqs, 31_250).split();
//! tx.write(b"hello").await;
//! let b = rx.read_byte().await;
//! ```
```

and the sentence "Configure GPIO pin-mux … before (or after) calling [`init`]" → "Mux its pins with [`crate::gpio::set_pin_mux`] before [`Uart::new`]."

6. **Add**, after `wait_tdfe`:

```rust
// ---------------------------------------------------------------------------
// Owned UART
// ---------------------------------------------------------------------------

/// Whether SCIF n receives / transmits by DMA: [`set_baud_regs`] restores the request enables.
static RX_DMA_ACTIVE: [AtomicBool; NUM_CHANNELS] = [const { AtomicBool::new(false) }; NUM_CHANNELS];
static TX_DMA_ACTIVE: [AtomicBool; NUM_CHANNELS] = [const { AtomicBool::new(false) }; NUM_CHANNELS];

mod sealed {
    pub trait Sealed {}
}

/// A SCIF channel singleton (`peripherals::SCIF0`–`SCIF4`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–4.
    const INDEX: usize;
    /// Its receive-error interrupt, ERI<INDEX>.
    type ErrorInterrupt: Interrupt;
    /// Its transmit-FIFO-empty interrupt, TXI<INDEX>.
    type TxInterrupt: Interrupt;
}

/// A SCIF channel the HAL keeps a receive-DMA ring for: SCIF0 and SCIF1.
pub trait RxDmaInstance: Instance {
    /// Its ring.
    const RING: usize;
}

macro_rules! instances {
    ($($scif:ident = $n:literal, $eri:ident, $txi:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$scif {}
        impl Instance for crate::peripherals::$scif {
            const INDEX: usize = $n;
            type ErrorInterrupt = crate::interrupt::typelevel::$eri;
            type TxInterrupt = crate::interrupt::typelevel::$txi;
        }
    )*};
}

instances! {
    SCIF0 = 0, ERI0, TXI0;
    SCIF1 = 1, ERI1, TXI1;
    SCIF2 = 2, ERI2, TXI2;
    SCIF3 = 3, ERI3, TXI3;
    SCIF4 = 4, ERI4, TXI4;
}

impl RxDmaInstance for crate::peripherals::SCIF0 {
    const RING: usize = 0;
}
impl RxDmaInstance for crate::peripherals::SCIF1 {
    const RING: usize = 1;
}

/// DMARS resource selector for SCIF `ch`'s receive request (SCIF0 = 0x62, then + 4 per channel).
fn rx_dmars(ch: usize) -> u32 {
    0x62 + 4 * ch as u32
}

/// DMARS resource selector for SCIF `ch`'s transmit request (SCIF0 = 0x61, then + 4 per channel).
fn tx_dmars(ch: usize) -> u32 {
    0x61 + 4 * ch as u32
}

/// SCIF `T`'s receive-error interrupt: clears the error flags, without which one framing error
/// stops reception for good, and wakes the reader.
pub struct ErrorInterruptHandler<T: Instance> {
    _scif: PhantomData<T>,
}

impl<T: Instance> Handler<T::ErrorInterrupt> for ErrorInterruptHandler<T> {
    unsafe fn on_interrupt() {
        on_eri(T::INDEX);
    }
}

/// SCIF `T`'s transmit interrupt: wakes a writer waiting for FIFO space.
pub struct TxInterruptHandler<T: Instance> {
    _scif: PhantomData<T>,
}

impl<T: Instance> Handler<T::TxInterrupt> for TxInterruptHandler<T> {
    unsafe fn on_interrupt() {
        on_txi(T::INDEX);
    }
}

/// A SCIF UART, 8N1: it receives into a DMA ring and transmits by DMA or through the FIFO.
/// [`split`](Self::split) it into its halves.
pub struct Uart<'d> {
    rx: UartRx<'d>,
    tx: UartTx<'d>,
}

impl<'d> Uart<'d> {
    /// Bring SCIF `T` up at `baud`: receive into a ring on `rx_dma`; transmit by DMA on `tx_dma`,
    /// or through the FIFO and TXI without one.
    ///
    /// Mux its pins before calling this, so the ring never samples an unconnected pin.
    ///
    /// # Panics
    /// If [`crate::clocks::init`] has not been called.
    pub fn new<T: RxDmaInstance>(
        _scif: Peri<'d, T>,
        rx_dma: Dma<'d>,
        tx_dma: Option<Dma<'d>>,
        irq: impl Binding<T::ErrorInterrupt, ErrorInterruptHandler<T>>
            + Binding<T::TxInterrupt, TxInterruptHandler<T>>
            + Copy
            + 'd,
        baud: u32,
    ) -> Self {
        // SAFETY: SCIF T is owned.
        unsafe { configure(T::INDEX, baud) };
        interrupt::bind::<T::ErrorInterrupt, ErrorInterruptHandler<T>>(irq);
        interrupt::bind::<T::TxInterrupt, TxInterruptHandler<T>>(irq);
        // SAFETY: SCIF T's own interrupts. ERI is enabled although reception is by DMA: receive
        // errors still reach the GIC, and one left uncleared stops reception for good.
        unsafe {
            gic::set_priority(T::ErrorInterrupt::ID, UART_IRQ_PRIORITY);
            gic::set_priority(T::TxInterrupt::ID, UART_IRQ_PRIORITY);
            gic::enable(T::ErrorInterrupt::ID);
            gic::enable(T::TxInterrupt::ID);
        }
        let rx = UartRx::start::<T>(rx_dma);
        let tx = UartTx::new(T::INDEX, tx_dma);
        Self { rx, tx }
    }

    /// The receive and transmit halves.
    pub fn split(self) -> (UartRx<'d>, UartTx<'d>) {
        (self.rx, self.tx)
    }
}

/// A UART's receiver: a 256-byte ring the DMAC fills from the receive FIFO, with no interrupt.
pub struct UartRx<'d> {
    ch: usize,
    dma: Dma<'d>,
    /// The ring's start, and the CPU's next read (cached addresses, as CRDA reports them).
    base: u32,
    read: u32,
}

impl<'d> UartRx<'d> {
    /// Start the circular receive DMA for SCIF `T` on `dma`.
    fn start<T: RxDmaInstance>(mut dma: Dma<'d>) -> Self {
        let ch = T::INDEX;
        let b = base(ch);
        let dma_ch = dma.number();
        // SAFETY: ring T::RING is SCIF T's alone, and SCIF T is owned. The descriptor is written
        // through the uncached alias, the address the DMAC fetches it from, so no cache
        // maintenance is needed.
        unsafe {
            let buf_base = core::ptr::addr_of!(DMA_RX_BUF[T::RING].0[0]) as u32;
            // As the C firmware's link descriptor word [4]: 0x8110_0060 | DMA_AM_FOR_SCIF |
            // the channel's index within its group.
            let chcfg: u32 = 0x8110_0060 | DMA_AM_FOR_SCIF | (dma_ch & 7) as u32;
            let desc = crate::uncached(core::ptr::addr_of_mut!(DMA_RX_DESC[T::RING]) as *mut u32);
            let desc_addr = desc as u32;
            desc.add(0).write_volatile(0b1101); // header
            desc.add(1).write_volatile((b + SCFRDR) as u32); // source: SCFRDR
            desc.add(2).write_volatile(buf_base); // destination: the ring
            desc.add(3).write_volatile(DMA_RX_BUF_SIZE as u32); // byte count
            desc.add(4).write_volatile(chcfg);
            desc.add(5).write_volatile(0); // interval
            desc.add(6).write_volatile(0); // extension
            desc.add(7).write_volatile(desc_addr); // next: itself, so the ring loops
            dma.init_with_link_descriptor(desc_addr as *const u32, rx_dmars(ch));
            dma.channel_start();
            RX_DMA_ACTIVE[ch].store(true, Ordering::Relaxed);
            // TIE|RIE|TE|RE: route both requests; the GIC's RXI stays off, so receive requests go
            // only to the DMAC.
            wr16(b + SCSCR, TIE | RIE | RE | TE);
            log::debug!(
                "uart: ch{} DMA RX active (dma_ch={}, buf={:#010x})",
                ch,
                dma_ch,
                buf_base
            );
            Self { ch, dma, base: buf_base, read: buf_base }
        }
    }

    /// The next received byte, or `None` if the ring is empty.
    pub fn try_read(&mut self) -> Option<u8> {
        // CRDA: where the DMAC writes next.
        let write = self.dma.current_dst();
        if write == self.read {
            return None;
        }
        // SAFETY: `read` is inside this UART's ring. Read through the uncached alias: the DMAC
        // wrote physical RAM, and a cached line may be stale.
        let byte = unsafe { crate::uncached(self.read as usize as *mut u8).read_volatile() };
        let offset = (self.read - self.base + 1) & (DMA_RX_BUF_SIZE as u32 - 1);
        self.read = self.base + offset;
        Some(byte)
    }

    /// Wait for the next received byte. The ring raises no interrupt, so this polls it every
    /// 100 µs.
    pub async fn read_byte(&mut self) -> u8 {
        loop {
            if let Some(byte) = self.try_read() {
                return byte;
            }
            embassy_time::Timer::after_micros(100).await;
        }
    }
}

/// A UART's transmitter. Its methods take `&self`; whole writes don't interleave.
pub struct UartTx<'d> {
    ch: usize,
    /// The transmit DMA channel, if any. The lock also keeps two writers' bytes apart.
    dma: Mutex<CriticalSectionRawMutex, Option<Dma<'d>>>,
}

impl<'d> UartTx<'d> {
    fn new(ch: usize, mut dma: Option<Dma<'d>>) -> Self {
        if let Some(dma) = dma.as_mut() {
            let b = base(ch);
            let ftdr = b + FTDR;
            // As the C firmware's DMA_SCIF_TX_CONFIG: 8-bit, source increments, destination
            // fixed (FTDR), destination-side request; AM=2 so SCIF TDFE paces it.
            let chcfg = 0x0020_0068_u32 | DMA_AM_FOR_SCIF | (dma.number() as u32 & 7);
            // SAFETY: every transfer writes SCIF ch's transmit FIFO, which this UART owns.
            unsafe {
                dma.init_register_mode(chcfg, ftdr as u32, tx_dmars(ch));
                TX_DMA_ACTIVE[ch].store(true, Ordering::Relaxed);
                // TIE is both the TXI enable and the DMAC request; OR it in, keeping RE/RIE.
                let scscr = rr16(b + SCSCR);
                wr16(b + SCSCR, scscr | TIE | TE);
                // TXI's handler clears TIE, which would stall the DMAC: mask TXI at the GIC.
                gic::disable(TXI_BASE + (ch as u16) * 4);
            }
            log::info!("uart: ch{} DMA TX active (dma_ch={})", ch, dma.number());
        }
        Self { ch, dma: Mutex::new(dma) }
    }

    /// Send `buf`: by DMA, one transfer per 64 bytes, waking once per transfer; or without a DMA
    /// channel, by filling the FIFO and waking on TXI as it drains.
    pub async fn write(&self, buf: &[u8]) {
        let mut dma = self.dma.lock().await;
        let Some(dma) = dma.as_mut() else {
            let mut pos = 0;
            while pos < buf.len() {
                pos += self.try_write(&buf[pos..]);
                if pos < buf.len() {
                    // FIFO full: wait for TDFE (count ≤ trigger) through TXI.
                    wait_tdfe(self.ch).await;
                }
            }
            return;
        };
        let mut pos = 0;
        while pos < buf.len() {
            let chunk = (buf.len() - pos).min(DMA_TX_BUF_SIZE);
            // SAFETY: SCIF ch's staging buffer, used only under this lock, written through the
            // uncached alias so the DMAC reads fresh bytes; its address is the bus address.
            unsafe {
                let staging = crate::uncached(core::ptr::addr_of_mut!(DMA_TX_BUF[self.ch].0[0]));
                core::ptr::copy_nonoverlapping(buf.as_ptr().add(pos), staging, chunk);
                dma.start_transfer(staging as u32, chunk as u32);
            }
            dma.wait_transfer_complete().await;
            pos += chunk;
        }
    }

    /// Put as much of `buf` in the transmit FIFO as fits now, and return how much that was.
    /// Doesn't wait: for short, latency-sensitive messages such as a MIDI clock byte.
    pub fn try_write(&self, buf: &[u8]) -> usize {
        let free = self.tx_free();
        let n = buf.len().min(free);
        for &byte in &buf[..n] {
            // SAFETY: the owned SCIF's FIFO.
            unsafe { wr8(base(self.ch) + FTDR, byte) };
        }
        n
    }

    /// Free space in the transmit FIFO, in bytes: how many [`try_write`](Self::try_write) would
    /// take now.
    pub fn tx_free(&self) -> usize {
        // SAFETY: a read of the owned SCIF's FIFO count.
        let in_fifo = unsafe { (rr16(base(self.ch) + SCFDR) >> 8) & 0x1F } as usize;
        TX_FIFO_SIZE.saturating_sub(in_fifo)
    }

    /// Change the baud rate. TX and RX stop briefly: wait for the FIFO to drain first, or its
    /// bytes go out at the wrong rate. The DMA request enables are kept.
    pub fn set_baud(&self, baud: u32) {
        // SAFETY: the owned SCIF.
        unsafe { set_baud_regs(self.ch, baud) }
    }
}

// ── embedded-io-async impls ───────────────────────────────────────────────────

use core::convert::Infallible;

impl embedded_io_async::ErrorType for UartRx<'_> {
    type Error = Infallible;
}

impl embedded_io_async::Read for UartRx<'_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
        if buf.is_empty() {
            return Ok(0);
        }
        // At least one byte, then whatever else the ring already holds.
        buf[0] = self.read_byte().await;
        let mut n = 1;
        while n < buf.len() {
            match self.try_read() {
                Some(byte) => {
                    buf[n] = byte;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }
}

impl embedded_io_async::ErrorType for UartTx<'_> {
    type Error = Infallible;
}

impl embedded_io_async::Write for UartTx<'_> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Infallible> {
        UartTx::write(self, buf).await;
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), Infallible> {
        // A DMA write returns once its transfer is complete; a FIFO write may leave bytes queued.
        if self.dma.lock().await.is_none() {
            wait_tdfe(self.ch).await;
        }
        Ok(())
    }
}
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib uart 2>&1 | grep -E "test result|FAILED"`
Expected: `ok`, with both new tests passing.

- [ ] **Step 4: The BSP's UARTs**

Replace `crates/deluge-bsp/src/uart.rs` above its `mod tests` with:

```rust
//! Deluge board UART assignments.
//!
//! Maps the two SCIF channels used on the Deluge to their physical pins and
//! gives them meaningful names, keeping [`rza1l_hal::uart`] itself board-agnostic.
//!
//! | Channel | Signal  | TxD pin  | mux | RxD pin | mux |
//! |---------|---------|----------|-----|---------|-----|
//! | SCIF0   | MIDI DIN| P6_15    |  5  | P6_14   |  5  |
//! | SCIF1   | PIC32   | P3_15    |  5  | P1_9    |  3  |
//!
//! [`init_midi`] and [`init_pic`] take the SCIF and its DMA channels from the [`crate::Board`]
//! and keep the UART's halves; the other functions reach the UART they brought up.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::once_lock::OnceLock;
use rza1l_hal::Peri;
use rza1l_hal::dmac::{self, Dma};
use rza1l_hal::uart::{self, Uart, UartRx, UartTx};

use crate::board::{MidiRxDma, MidiScif, PicRxDma, PicScif, PicTxDma};

/// SCIF channel used for MIDI DIN (31250 bps).
pub const MIDI_CH: usize = 0;

/// SCIF channel used for the PIC32 co-processor.
pub const PIC_CH: usize = 1;

rza1l_hal::bind_interrupts!(struct MidiIrqs {
    ERI0 => uart::ErrorInterruptHandler<MidiScif>;
    TXI0 => uart::TxInterruptHandler<MidiScif>;
    DMAINT13 => dmac::InterruptHandler<MidiRxDma>;
});

rza1l_hal::bind_interrupts!(struct PicIrqs {
    ERI1 => uart::ErrorInterruptHandler<PicScif>;
    TXI1 => uart::TxInterruptHandler<PicScif>;
    DMAINT10 => dmac::InterruptHandler<PicTxDma>;
    DMAINT12 => dmac::InterruptHandler<PicRxDma>;
});

/// A UART's receiver, async-locked because a read holds it across `.await`.
type RxSlot = Mutex<CriticalSectionRawMutex, Option<UartRx<'static>>>;

static MIDI_RX: RxSlot = Mutex::new(None);
static MIDI_TX: OnceLock<UartTx<'static>> = OnceLock::new();
static PIC_RX: RxSlot = Mutex::new(None);
static PIC_TX: OnceLock<UartTx<'static>> = OnceLock::new();

/// Bring up the MIDI DIN UART (SCIF0): receive by DMA, transmit through the FIFO and TXI
/// (MIDI messages are short).
pub fn init_midi(scif: Peri<'static, MidiScif>, rx_dma: Peri<'static, MidiRxDma>, baud: u32) {
    log::debug!("bsp uart: MIDI SCIF{} @ {} bps", MIDI_CH, baud);
    // SAFETY: TxD0 and RxD0, which only SCIF0 drives. Muxed first, so the receive ring never
    // samples an unconnected pin.
    unsafe {
        rza1l_hal::gpio::set_pin_mux(6, 15, 5); // TxD0
        rza1l_hal::gpio::set_pin_mux(6, 14, 5); // RxD0
    }
    let rx_dma = Dma::new(rx_dma, MidiIrqs);
    let (rx, tx) = Uart::new(scif, rx_dma, None, MidiIrqs, baud).split();
    install(&MIDI_RX, &MIDI_TX, rx, tx);
}

/// Bring up the PIC32 UART (SCIF1): receive and transmit by DMA.
///
/// **Why DMA and not RXI:** on the RZ/A1 the SCIF "receive-FIFO-data-full"
/// request is wired to the DMAC, not the GIC — with RIE=1 and RDF=1 no GIC line
/// in the SCIF range 220–245 goes pending. The stock Renesas/Deluge firmware
/// notes the same: `SCSCR = 0x00F0; // Enable "interrupt" (which actually
/// triggers DMA)`. So RX must go through the DMAC.
pub fn init_pic(
    scif: Peri<'static, PicScif>,
    rx_dma: Peri<'static, PicRxDma>,
    tx_dma: Peri<'static, PicTxDma>,
    baud: u32,
) {
    log::debug!("bsp uart: PIC SCIF{} @ {} bps", PIC_CH, baud);
    // SAFETY: TxD1 and RxD1, which only SCIF1 drives; muxed first, as in `init_midi`.
    unsafe {
        rza1l_hal::gpio::set_pin_mux(3, 15, 5); // TxD1
        rza1l_hal::gpio::set_pin_mux(1, 9, 3); // RxD1
    }
    let rx_dma = Dma::new(rx_dma, PicIrqs);
    let tx_dma = Dma::new(tx_dma, PicIrqs);
    let (rx, tx) = Uart::new(scif, rx_dma, Some(tx_dma), PicIrqs, baud).split();
    install(&PIC_RX, &PIC_TX, rx, tx);
}

fn install(
    rx_slot: &'static RxSlot,
    tx_slot: &'static OnceLock<UartTx<'static>>,
    rx: UartRx<'static>,
    tx: UartTx<'static>,
) {
    // Each SCIF is owned once, so both slots are empty.
    match rx_slot.try_lock() {
        Ok(mut slot) => *slot = Some(rx),
        Err(_) => unreachable!("a UART's receiver is locked before the UART exists"),
    }
    if tx_slot.init(tx).is_err() {
        unreachable!("a UART's transmitter is installed twice");
    }
}

async fn read_byte(slot: &'static RxSlot, name: &str) -> u8 {
    let mut rx = slot.lock().await;
    match rx.as_mut() {
        Some(rx) => rx.read_byte().await,
        None => panic!("the {name} UART isn't initialised"),
    }
}

fn try_read(slot: &'static RxSlot) -> Option<u8> {
    slot.try_lock().ok()?.as_mut()?.try_read()
}

fn tx(slot: &'static OnceLock<UartTx<'static>>, name: &str) -> &'static UartTx<'static> {
    slot.try_get()
        .unwrap_or_else(|| panic!("the {name} UART isn't initialised"))
}

/// Write bytes to the MIDI DIN UART.
///
/// # Panics
/// Before [`init_midi`].
pub async fn write_midi(data: &[u8]) {
    tx(&MIDI_TX, "MIDI").write(data).await;
}

/// Read one byte from the MIDI DIN UART (its DMA ring).
///
/// # Panics
/// Before [`init_midi`].
pub async fn read_midi_byte() -> u8 {
    read_byte(&MIDI_RX, "MIDI").await
}

/// The next raw byte from the MIDI DIN ring, without waiting. `None` if the ring is empty, if
/// [`init_midi`] hasn't run, or while a [`read_midi_byte`] is waiting (it drains the ring).
pub fn try_read_midi() -> Option<u8> {
    try_read(&MIDI_RX)
}

/// Put as much of `data` in the MIDI DIN transmit FIFO as fits now; returns how much. Enough for
/// short messages such as clock pulses (`0xF8`). 0 before [`init_midi`].
pub fn try_write_midi(data: &[u8]) -> usize {
    MIDI_TX.try_get().map_or(0, |tx| tx.try_write(data))
}

/// Free space in the MIDI DIN TX FIFO, in bytes: how many [`try_write_midi`] would take now.
/// 0 before [`init_midi`].
pub fn midi_write_space() -> usize {
    MIDI_TX.try_get().map_or(0, |tx| tx.tx_free())
}

/// Write bytes to the PIC UART. [`crate::pic`] serialises whole commands around this.
///
/// # Panics
/// Before [`init_pic`].
pub async fn write_pic(data: &[u8]) {
    tx(&PIC_TX, "PIC").write(data).await;
}

/// Read one byte from the PIC UART (its DMA ring).
///
/// # Panics
/// Before [`init_pic`].
pub async fn read_pic_byte() -> u8 {
    read_byte(&PIC_RX, "PIC").await
}

/// The next raw byte from the PIC ring, without waiting; see [`try_read_midi`] for `None`.
pub fn try_read_pic() -> Option<u8> {
    try_read(&PIC_RX)
}

/// Put as much of `data` in the PIC transmit FIFO as fits now; returns how much. For fault and
/// panic paths that can't wait for the DMA writer. 0 before [`init_pic`].
pub fn try_write_pic(data: &[u8]) -> usize {
    PIC_TX.try_get().map_or(0, |tx| tx.try_write(data))
}

/// Change the PIC UART's baud rate (the PIC handshake switches 31 250 → 200 000 bps). Wait for
/// the last write to drain first.
///
/// # Panics
/// Before [`init_pic`].
pub fn set_pic_baud(baud: u32) {
    tx(&PIC_TX, "PIC").set_baud(baud);
}
```

In `crates/deluge-bsp/src/pic.rs`: delete `use rza1l_hal::uart;`. In the device `init`, replace `unsafe { uart::set_baud(UART_CH, BAUD_FAST) };` with `crate::uart::set_pic_baud(BAUD_FAST);`, and its comment's second sentence with "SCIF1 was brought up by `deluge_bsp::uart::init_pic`; this updates SCBRR." Device `tx`'s body becomes `let _guard = PIC_TX.lock().await; crate::uart::write_pic(bytes).await;`. Its doc's "do not call `uart::write_bytes(UART_CH, …)` directly" becomes "do not call `crate::uart::write_pic` directly". Device `read_byte`'s body becomes `crate::uart::read_pic_byte().await`, with the doc's "pops the DMA ring via `uart::read_byte`" changed to "via `crate::uart::read_pic_byte`". In `send_blocking`, replace `// SAFETY: …` and `let n = unsafe { uart::try_write_fifo(UART_CH, &bytes[sent..]) };` with `let n = crate::uart::try_write_pic(&bytes[sent..]);`. In the docs of `init` and of `UART_CH`, `deluge_bsp::uart::init_pic(31_250)` → `deluge_bsp::uart::init_pic`.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib 2>&1 | grep -E "test result|FAILED"`
Expected: `ok`.

- [ ] **Step 5: Callers**

PIC readers: replace `rza1l_hal::uart::read_byte(pic::UART_CH).await` with `pic::read_byte().await` in `tasks/pic.rs` of `hw-tests/{wp-probe,vbus-probe,uac-host-firmware,sd-bench}` and `firmwares/{msc-firmware,controller-firmware,demo-firmware}`. In `firmwares/app-loader/src/main.rs` (`pic_rx_task`), replace `rza1l_hal::uart::read_byte(deluge_bsp::uart::PIC_CH).await` with `pic::read_byte().await`.

Bring-up, firmwares holding a `board`:
- `hw-tests/{wp-probe,vbus-probe,uac-host-firmware}/src/main.rs`, `firmwares/{msc-firmware,app-loader}/src/main.rs`: `unsafe { bsp_uart::init_pic(31_250) };` (or `deluge_bsp::uart::init_pic`) → `deluge_bsp::uart::init_pic(board.pic_uart, board.dma.pic_rx, board.dma.pic_tx, 31_250);`.
- `hw-tests/sd-bench/src/main.rs`: the same, after renaming `let _board` → `let board`.
- `firmwares/controller-firmware/src/main.rs`: also `unsafe { bsp_uart::init_midi(31_250) };` → `bsp_uart::init_midi(board.midi_uart, board.dma.midi_rx, 31_250);`.

`firmwares/demo-firmware/src/main.rs` `setup()`: replace the two `unsafe { bsp_uart::init_… }` lines with:

```rust
    let (midi, midi_rx) = deluge::parts::with(|p| (p.midi_uart.take(), p.midi_rx_dma.take()));
    bsp_uart::init_midi(midi.expect("SCIF0 is free"), midi_rx.expect("the MIDI RX DMA channel is free"), 31_250);
    info!("UART: initialising PIC...");
    let (pic, pic_rx, pic_tx) =
        deluge::parts::with(|p| (p.pic_uart.take(), p.pic_rx_dma.take(), p.pic_tx_dma.take()));
    bsp_uart::init_pic(
        pic.expect("SCIF1 is free"),
        pic_rx.expect("the PIC RX DMA channel is free"),
        pic_tx.expect("the PIC TX DMA channel is free"),
        31_250,
    );
```

(keep the surrounding `info!` lines).

`crates/deluge-sdk/src/plat/device.rs`:
- `pic_ensure_started`: replace the `// SAFETY: …` comment and `unsafe { bsp_uart::init_pic(PIC_BAUD) };` with:

```rust
    let uart = crate::parts::take("SCIF1 (the PIC link)", |p| p.pic_uart.take());
    let rx = crate::parts::take("the PIC receive DMA channel", |p| p.pic_rx_dma.take());
    let tx = crate::parts::take("the PIC transmit DMA channel", |p| p.pic_tx_dma.take());
    bsp_uart::init_pic(uart, rx, tx, PIC_BAUD);
```

- `pic_pump`: `bsp_uart::read_byte(pic::UART_CH).await` → `pic::read_byte().await`.
- `midi_init`: its body becomes:

```rust
    let uart = crate::parts::take("SCIF0 (DIN MIDI)", |p| p.midi_uart.take());
    let rx = crate::parts::take("the MIDI receive DMA channel", |p| p.midi_rx_dma.take());
    deluge_bsp::uart::init_midi(uart, rx, MIDI_BAUD);
```

Remove any import the compiler now reports unused (for example `rza1l_hal::uart` in a firmware).

- [ ] **Step 6: Suite, build check, commit**

Run: `tools/test.sh > /tmp/claude-1000/p4b-t4.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

Run: `rg -n "uart::(read_byte|write_bytes|init|set_baud|register_txi_for|init_dma_rx|init_dma_tx|try_read_dma|try_write_fifo|tx_fifo_free|ScifUart)\b" -g '*.rs' crates firmwares hw-tests examples wren | grep -v "^crates/rza1l-hal/src/uart.rs"`
Expected: no output (the BSP's `init_midi`/`init_pic` don't match `init\b`).

Run the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal crates/deluge-bsp/src/uart.rs crates/deluge-bsp/src/pic.rs crates/deluge-sdk/src/plat/device.rs firmwares hw-tests
git commit -m "feat(rza1l-hal,bsp): Uart is built from its SCIF, DMA channels and binding, and splits into UartRx and UartTx; the BSP keeps the MIDI and PIC halves"
```

---

### Task 5: MTU2 one-shots are owned, and the gate and encoder timers take their channels

**Files:**
- Modify: `crates/rza1l-hal/src/mtu2.rs`
- Modify: `crates/deluge-bsp/src/midi_gate.rs`, `crates/deluge-bsp/src/encoder.rs`
- Modify: `crates/deluge-sdk/src/plat/device.rs` (`input_start_pump`), `firmwares/controller-firmware/src/main.rs`, `firmwares/demo-firmware/src/main.rs`, `firmwares/app-loader/src/main.rs`

**Interfaces:**
- Consumes: Task 1's `typelevel::{TGI0A..TGI4A}`, `gic::disable` on the seam; Task 2's `GateTimer`, `EncoderPollTimer`, `Board::{gate_timer, encoder_poll_timer}`.
- Produces: `rza1l_hal::mtu2::{Instance, OneShot}` with `OneShot::new<T: Instance, H: Handler<T::Interrupt>>(Peri<'d, T>, prescaler: u16, priority: u8, irq: impl Binding<T::Interrupt, H> + 'd) -> OneShot<'d>` and `&self` methods `arm(u16)`, `disarm()`, `clear_flag()`; `mtu2::TGIA_IRQ` is `pub(crate)`. `deluge_bsp::midi_gate::{setup(Peri<'static, GateTimer>), schedule_gate_off(u8, u16), cancel()}`, all safe. `deluge_bsp::encoder::irq_init(Peri<'static, EncoderPollTimer>)` (still `unsafe`: it configures the encoder GPIO interrupts).

- [ ] **Step 1: Write the failing tests**

In `crates/rza1l-hal/src/mtu2.rs`'s `mod tests`:
- In `tpsc_common_prescalers` and `tpsc_channel_specific_prescalers`, wrap every expected code in `Some(…)`, for example `assert_eq!(tpsc(ch, 1), Some(0b000));`.
- Replace `tpsc_invalid_combinations_fall_back_to_div1` with:

```rust
    #[test]
    fn tpsc_refuses_prescalers_the_channel_lacks() {
        // /256 not available on ch0/ch2; /1024 not on ch0/ch1; bogus values never.
        assert_eq!(tpsc(0, 256), None);
        assert_eq!(tpsc(2, 256), None);
        assert_eq!(tpsc(0, 1024), None);
        assert_eq!(tpsc(1, 1024), None);
        assert_eq!(tpsc(0, 3), None);
    }
```

- Delete `start_free_running_configures_tcr_and_sets_cst`, `stop_clears_only_this_channels_cst_bit` and `out_of_range_channel_is_a_noop` (their functions go in Step 3), and append:

```rust
    use crate::interrupt::{Handler, typelevel};
    use crate::peripherals::{MTU2_0, MTU2_2};

    struct Shot;
    impl Handler<typelevel::TGI2A> for Shot {
        unsafe fn on_interrupt() {}
    }
    struct Shot0;
    impl Handler<typelevel::TGI0A> for Shot0 {
        unsafe fn on_interrupt() {}
    }

    crate::bind_interrupts!(struct Irqs {
        TGI0A => Shot0;
        TGI2A => Shot;
    });

    #[test]
    fn a_one_shot_is_bound_stopped_and_prioritised_but_not_enabled() {
        mmio::test::reset();
        let _timer = OneShot::new(unsafe { MTU2_2::steal() }, 64, 5, Irqs);
        assert!(crate::gic::handler(150).is_some(), "TGI2A bound");
        assert_eq!(
            mmio::test::writes(),
            [
                (TRWER, 1),
                (TSTR, 0),                    // channel 2 stopped
                (TCR[2], 0b011 | (0b001 << 5)), // P0φ/64, counter cleared on a TGRA match
                (TIER[2], 0),
                (0xE820_1494, (5 << 3) << 16), // GICD_IPRIORITYR37, byte lane 150 % 4 = 2
            ]
        );
    }

    #[test]
    #[should_panic(expected = "MTU2 channel 0 has no /256 prescaler")]
    fn a_prescaler_the_channel_lacks_is_refused() {
        let _ = OneShot::new(unsafe { MTU2_0::steal() }, 256, 5, Irqs);
    }

    #[test]
    fn arming_starts_a_shot_from_zero_and_unmasks_it() {
        let timer = OneShot::new(unsafe { MTU2_2::steal() }, 64, 5, Irqs);
        mmio::test::reset();
        timer.arm(520);
        assert_eq!(
            mmio::test::writes(),
            [
                (TSTR, 0),
                (TGRA[2], 520),
                (TCNT[2], 0),
                (0xE820_1110, 1 << 22), // GICD_ISENABLER4, bit 150 - 128
                (TIER[2], 1),
                (TSTR, CST[2] as u32),
            ]
        );
    }

    #[test]
    fn disarming_stops_the_shot_and_masks_it() {
        let timer = OneShot::new(unsafe { MTU2_2::steal() }, 64, 5, Irqs);
        mmio::test::reset();
        mmio::test::poke8(TSTR, CST[2] | CST[3]);
        timer.disarm();
        assert_eq!(
            mmio::test::writes(),
            [
                (TIER[2], 0),
                (TSTR, CST[3] as u32), // channel 3 keeps running
                (0xE820_1190, 1 << 22), // GICD_ICENABLER4, bit 150 - 128
            ]
        );
    }
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib mtu2 2>&1 | tail -5`
Expected: compile errors: `tpsc` returns `u8`, and there is no `OneShot`.

- [ ] **Step 2: Implement**

In `crates/rza1l-hal/src/mtu2.rs`:

1. `tpsc` returns `Option<u8>`: each arm's value becomes `Some(…)`, and the fallbacks `_ => 0b000` (three of them) become `_ => None`. Its doc's last line becomes "Returns `None` for a prescaler the channel lacks."
2. `pub const TGIA_IRQ` → `pub(crate) const TGIA_IRQ`.
3. Delete `start_free_running`, `stop` and `count`. Make `enable_write`, `arm`, `clear_flag` and `disarm` private (drop `pub`). Replace `setup_one_shot` with this private version, which no longer registers a handler:

```rust
/// Stop channel `ch` and configure it for one-shot compare-match use: TCR = `tpsc` with
/// CCLR = TGRA (the counter clears on the match), TIER = 0, and TGInA's GIC priority. The GIC
/// source stays masked until [`arm`].
///
/// # Safety
/// Writes MTU2 and GIC registers; `ch` is owned.
unsafe fn setup_one_shot(ch: u8, tpsc: u8, priority: u8) {
    unsafe {
        wr8(TSTR, rd8(TSTR) & !CST[ch as usize]);
        wr8(TCR[ch as usize], tpsc | (1 << 5));
        wr8(TIER[ch as usize], 0);
        crate::gic::set_priority(TGIA_IRQ[ch as usize], priority);
    }
}
```

   In `arm`, `clear_flag` and `disarm`, delete the `if ch > 4 { return; }` guards: only an `Instance` reaches them now.
4. Replace the module docs' two usage examples with:

```rust
//! ## Usage (one-shot)
//! ```ignore
//! struct GateHandler;
//! impl Handler<typelevel::TGI2A> for GateHandler {
//!     unsafe fn on_interrupt() { /* timer.clear_flag(); act; timer.disarm(); */ }
//! }
//! rza1l_hal::bind_interrupts!(struct Irqs { TGI2A => GateHandler; });
//!
//! let timer = mtu2::OneShot::new(p.MTU2_2, 64, 5, Irqs); // P0φ/64, GIC priority 5
//! timer.arm(520); // fires after 520 ticks ≈ 1 ms
//! ```
```

   and in the TSTR section, "call [`enable_write`] once before touching them" → "[`OneShot::new`] opens it".
5. Append, before `#[cfg(all(test, …))] mod tests`:

```rust
// ---------------------------------------------------------------------------
// Owned one-shot
// ---------------------------------------------------------------------------

use core::marker::PhantomData;

use crate::Peri;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// An MTU2 channel singleton (`peripherals::MTU2_0`–`MTU2_4`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–4.
    const CH: u8;
    /// Its compare-match-A interrupt, TGI<CH>A.
    type Interrupt: Interrupt;
}

macro_rules! instances {
    ($($t:ident = $n:literal, $irq:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$t {}
        impl Instance for crate::peripherals::$t {
            const CH: u8 = $n;
            type Interrupt = crate::interrupt::typelevel::$irq;
        }
    )*};
}

instances! {
    MTU2_0 = 0, TGI0A;
    MTU2_1 = 1, TGI1A;
    MTU2_2 = 2, TGI2A;
    MTU2_3 = 3, TGI3A;
    MTU2_4 = 4, TGI4A;
}

/// An MTU2 channel as a one-shot timer: [`arm`](Self::arm) it for a number of ticks and its
/// compare-match handler runs once they have elapsed. The counter clears on the match, so a shot
/// left armed fires again every period, which makes a periodic tick too.
///
/// Its methods take `&self`, so an interrupt handler can reach it through a static.
pub struct OneShot<'d> {
    ch: u8,
    _timer: PhantomData<&'d mut ()>,
}

impl<'d> OneShot<'d> {
    /// Configure channel `T` at P0φ / `prescaler`, with `H` handling its compare match at GIC
    /// `priority` (0 most urgent, 31 least). Stopped until [`arm`](Self::arm).
    ///
    /// # Panics
    /// If channel `T` has no /`prescaler`: every channel has /1, /4, /16 and /64; channels 1, 3
    /// and 4 have /256; channels 2, 3 and 4 have /1024.
    pub fn new<T: Instance, H: Handler<T::Interrupt>>(
        _timer: Peri<'d, T>,
        prescaler: u16,
        priority: u8,
        irq: impl Binding<T::Interrupt, H> + 'd,
    ) -> Self {
        let code = tpsc(T::CH, prescaler).unwrap_or_else(|| {
            panic!("MTU2 channel {} has no /{} prescaler", T::CH, prescaler)
        });
        interrupt::bind(irq);
        // SAFETY: channel T is owned. TRWER opens channels 3 and 4's registers and is harmless
        // to write again.
        unsafe {
            enable_write();
            setup_one_shot(T::CH, code, priority);
        }
        Self { ch: T::CH, _timer: PhantomData }
    }

    /// Fire once `ticks` from now: the counter restarts from 0, a stale match flag is cleared, and
    /// the interrupt is unmasked. Re-arming before it fires replaces the pending shot.
    pub fn arm(&self, ticks: u16) {
        // SAFETY: the owned channel.
        unsafe { arm(self.ch, ticks) }
    }

    /// Stop the counter and mask the interrupt. Safe to call from the handler.
    pub fn disarm(&self) {
        // SAFETY: the owned channel.
        unsafe { disarm(self.ch) }
    }

    /// Clear the compare-match flag. The handler must do this before it returns, or the GIC
    /// raises the interrupt again at once.
    pub fn clear_flag(&self) {
        // SAFETY: the owned channel.
        unsafe { clear_flag(self.ch) }
    }
}
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib 2>&1 | grep -E "test result|FAILED"`
Expected: `ok` (the HAL's Task 1 test `the_4b_interrupt_ids_match_the_hardware_manual` still sees `TGIA_IRQ`).

- [ ] **Step 3: The gate timer takes MTU2 channel 2**

In `crates/deluge-bsp/src/midi_gate.rs`, replace the usage example in the module docs with:

```rust
//! ## Usage
//! ```rust,no_run
//! use deluge_bsp::midi_gate;
//!
//! // Once at startup, with the Board's MTU2 channel 2:
//! midi_gate::setup(board.gate_timer);
//!
//! // At render time, to de-assert gate 0 in 520 ticks (~1 ms):
//! midi_gate::schedule_gate_off(0, 520);
//!
//! // To cancel a pending shot (e.g. note extended):
//! midi_gate::cancel();
//! ```
```

Then replace everything from `use core::sync::atomic::{AtomicU8, Ordering};` down to (not including) the `TICK_HZ` doc comment with:

```rust
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::once_lock::OnceLock;
use rza1l_hal::Peri;
use rza1l_hal::interrupt::{Handler, typelevel};
use rza1l_hal::mtu2::OneShot;

use crate::board::GateTimer;

/// Prescaler: P0 / 64 ≈ 520 kHz.
const PRESCALER: u16 = 64;

/// GIC priority for TGI2A (must be lower-number / higher-priority than USB
/// at 9, to ensure gate events are not delayed behind USB servicing).
const PRIORITY: u8 = 5;

/// Sentinel value meaning "no pending action".
const NO_ACTION: u8 = 0xFF;

/// Gate channel to de-assert when the timer fires (0–3), or `NO_ACTION`.
/// Written by the scheduling side with `Release`; read by the ISR with `Acquire`.
static GATE_CH: AtomicU8 = AtomicU8::new(NO_ACTION);

/// The timer, once [`setup`] has run. The interrupt handler reaches it here.
static TIMER: OnceLock<OneShot<'static>> = OnceLock::new();

/// TGI2A: the shot fired.
struct ShotHandler;

impl Handler<typelevel::TGI2A> for ShotHandler {
    unsafe fn on_interrupt() {
        // Bound only by `setup`, which fills TIMER first.
        let Some(timer) = TIMER.try_get() else { return };
        // 1. Clear the compare-match flag first, so the GIC doesn't re-trigger.
        timer.clear_flag();
        // 2. Take the pending action (Acquire pairs with schedule_gate_off's Release); swapping
        //    NO_ACTION back makes a concurrent cancel or re-schedule race-free.
        let gate_ch = GATE_CH.swap(NO_ACTION, Ordering::Acquire);
        // 3. Do it.
        if gate_ch != NO_ACTION {
            // SAFETY: a gate GPIO, configured by cv_gate::init.
            unsafe { crate::cv_gate::gate_set(gate_ch, false) }; // de-assert: gate OFF
        }
        // 4. Disarm. The GIC mask takes effect after the assembly IRQ stub writes ICCEOIR.
        timer.disarm();
    }
}

rza1l_hal::bind_interrupts!(struct Irqs {
    TGI2A => ShotHandler;
});

/// Configure MTU2 channel 2 for one-shot gate timing. The timer is left stopped; it is started
/// by [`schedule_gate_off`].
pub fn setup(timer: Peri<'static, GateTimer>) {
    // Fill TIMER before the binding can fire: the handler needs it.
    if TIMER.init(OneShot::new(timer, PRESCALER, PRIORITY, Irqs)).is_err() {
        unreachable!("MTU2 channel 2 is owned once");
    }
}

fn timer() -> &'static OneShot<'static> {
    TIMER.try_get().expect("midi_gate::setup hasn't run")
}

/// Schedule a gate-off event `ticks` from now on gate channel `gate_ch`.
///
/// Arms MTU2 ch2 to fire after `ticks` P0/64 ticks (~1.92 µs each).  The
/// ISR will de-assert `gate_ch` when the timer expires.
///
/// - `gate_ch`: hardware gate output to turn off (0–3).
/// - `ticks`: number of ticks (1–65535).  Minimum useful value is a few ticks
///   to allow for ISR latency.
///
/// Calling this again before the timer fires cancels the previous target and
/// re-arms with the new parameters (the ISR will see the updated gate channel).
///
/// # Panics
/// Before [`setup`].
pub fn schedule_gate_off(gate_ch: u8, ticks: u16) {
    // Store the action before arming; pairs with the Acquire in the handler.
    GATE_CH.store(gate_ch, Ordering::Release);
    timer().arm(ticks);
}

/// Cancel a pending gate-off shot without firing.
///
/// If the timer has already fired this is a no-op (the handler already cleared
/// `GATE_CH`).  If the timer is still running it is stopped and disarmed before
/// the ISR can execute the gate action.
///
/// # Panics
/// Before [`setup`].
pub fn cancel() {
    // Disarm first so the ISR cannot begin executing between the store and the disarm.
    timer().disarm();
    GATE_CH.store(NO_ACTION, Ordering::Release);
}
```

Leave the module docs' "## Mechanism" section as it is: it still describes what the handler does. `OneShot::new` binds the handler before `TIMER.init` stores the timer. The handler can't fire in that gap, because the source stays masked until `arm`.

- [ ] **Step 4: The encoder poll takes MTU2 channel 3**

In `crates/deluge-bsp/src/encoder.rs`:
- Delete the `POLL_CH` constant and `poll_irq_handler`. In their place (device-only, like the code around them) add:

```rust
/// The poll timer, once [`irq_init`] has run. Its handler reaches it here.
#[cfg(target_os = "none")]
static POLL_TIMER: embassy_sync::once_lock::OnceLock<rza1l_hal::mtu2::OneShot<'static>> =
    embassy_sync::once_lock::OnceLock::new();

/// The poll's compare-match interrupt. The channel clears its counter on the match, so leaving it
/// armed makes it fire every period.
#[cfg(target_os = "none")]
struct PollHandler;

#[cfg(target_os = "none")]
impl rza1l_hal::interrupt::Handler<rza1l_hal::interrupt::typelevel::TGI3A> for PollHandler {
    unsafe fn on_interrupt() {
        if let Some(timer) = POLL_TIMER.try_get() {
            timer.clear_flag();
        }
        poll();
    }
}

#[cfg(target_os = "none")]
rza1l_hal::bind_interrupts!(struct PollIrqs {
    TGI3A => PollHandler;
});
```

- Device `irq_init` takes the channel: `pub unsafe fn irq_init(poll_timer: rza1l_hal::Peri<'static, crate::board::EncoderPollTimer>)`. Its doc's "starts the millisecond pin poll on MTU2 channel 3" stays. Replace the block from `rza1l_hal::mtu2::enable_write();` through `rza1l_hal::mtu2::arm(POLL_CH, POLL_TICKS);` with:

```rust
        // Store the timer before arming it: its handler clears the match through POLL_TIMER,
        // and an uncleared match re-raises the interrupt at once.
        let timer =
            rza1l_hal::mtu2::OneShot::new(poll_timer, POLL_PRESCALER, ENCODER_PRIORITY, PollIrqs);
        if POLL_TIMER.init(timer).is_err() {
            unreachable!("MTU2 channel 3 is owned once");
        }
        if let Some(timer) = POLL_TIMER.try_get() {
            timer.arm(POLL_TICKS);
        }
```

- The host `irq_init` takes the same parameter, unused: `pub unsafe fn irq_init(_poll_timer: rza1l_hal::Peri<'static, crate::board::EncoderPollTimer>) {}`. In `host_irq_init_is_a_safe_noop_and_encoders_are_quiescent`, the call becomes `unsafe { irq_init(rza1l_hal::peripherals::MTU2_3::steal()) };`.

- [ ] **Step 5: Callers**

- `crates/deluge-sdk/src/plat/device.rs` `input_start_pump`: `unsafe { encoder::irq_init() };` → `let poll_timer = crate::parts::take("MTU2 channel 3 (the encoder poll)", |p| p.encoder_poll_timer.take()); unsafe { encoder::irq_init(poll_timer) };`.
- `firmwares/controller-firmware/src/main.rs`: `unsafe { deluge_bsp::encoder::irq_init() };` → `unsafe { deluge_bsp::encoder::irq_init(board.encoder_poll_timer) };`.
- `firmwares/demo-firmware/src/main.rs` `setup()`: `unsafe { deluge_bsp::encoder::irq_init() };` → `let poll_timer = deluge::parts::with(|p| p.encoder_poll_timer.take()).expect("MTU2 channel 3 is free"); unsafe { deluge_bsp::encoder::irq_init(poll_timer) };`.
- `firmwares/app-loader/src/main.rs`: move the encoder bring-up out of `boot_task` into `main`. Add `unsafe { deluge_bsp::encoder::irq_init(board.encoder_poll_timer) };` directly after the `cv_gate::init()` line, where interrupts are still masked. Delete the `irq_init` line in `boot_task`, and trim its comment's last sentence ("encoder::irq_init() registers … before the global unmask.") to "main() has already registered every GIC source the boot tasks need."

- [ ] **Step 6: Suite, build check, commit**

Run: `tools/test.sh > /tmp/claude-1000/p4b-t5.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

Run: `rg -n "mtu2::(setup_one_shot|arm|disarm|clear_flag|enable_write|start_free_running)" -g '*.rs' crates firmwares hw-tests | grep -v "^crates/rza1l-hal/src/mtu2.rs"`
Expected: no output.

Run the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/mtu2.rs crates/deluge-bsp/src/midi_gate.rs crates/deluge-bsp/src/encoder.rs \
  crates/deluge-sdk/src/plat/device.rs firmwares
git commit -m "feat(rza1l-hal,bsp): MTU2 one-shots are built from their channel and handler binding and refuse a prescaler the channel lacks; the gate and encoder timers take theirs from the Board"
```

---

### Task 6: The time driver owns OSTM0 and OSTM1, and delays are safe

**Files:**
- Modify: `crates/rza1l-hal/src/ostm.rs`, `crates/rza1l-hal/src/time_driver.rs`
- Modify: `crates/deluge-bsp/src/board.rs` (`Board::new`), `crates/deluge-bsp/src/{cv_gate,oled,audio}.rs` (`ostm::delay_ms` calls), `firmwares/app-loader/src/main.rs` (`quiesce_for_handoff`)

**Interfaces:**
- Consumes: Task 1's `typelevel::{OSTMI0, OSTMI1}` and `gic::disable` on the seam.
- Produces: `rza1l_hal::time_driver::{OverflowHandler, AlarmHandler, init(Peri<'static, OSTM0>, Peri<'static, OSTM1>, irq: impl Binding<OSTMI0, OverflowHandler> + Binding<OSTMI1, AlarmHandler> + Copy + 'static), unsafe stop()}`. Safe `ostm::{delay_ms(u32), try_delay_ms(u32) -> bool, Delay}`. Every other `ostm` function becomes `pub(crate)` or private, and `Ostm<N>` is deleted.

- [ ] **Step 1: Write the failing tests**

In `crates/rza1l-hal/src/ostm.rs`, `mod tests`, widen the import to `use super::*;` and `use crate::clocks::TEST_CLOCKS;`, then append:

```rust
    #[test]
    fn the_time_drivers_ostm0_setup_is_free_running_with_a_wrap_interrupt() {
        crate::mmio::test::reset();
        unsafe { start_free_running_cmp(0, 0) };
        assert_eq!(
            crate::mmio::test::writes(),
            [
                (OSTM0_BASE + TT, 1),
                (OSTM0_BASE + CMP, 0),
                (OSTM0_BASE + CTL, CTL_FREE_RUN_INT as u32),
                (OSTM0_BASE + TS, 1),
            ]
        );
    }

    #[test]
    fn the_alarm_counts_down_once_from_its_delta() {
        crate::mmio::test::reset();
        unsafe { start_alarm(1, 33_064) };
        assert_eq!(
            crate::mmio::test::writes(),
            [
                (OSTM1_BASE + TT, 1),
                (OSTM1_BASE + CMP, 33_064),
                (OSTM1_BASE + CTL, CTL_INTERVAL as u32),
                (OSTM1_BASE + TS, 1),
            ]
        );
    }

    /// Before the time driver starts OSTM0 its counter stands still: a delay would spin forever.
    #[test]
    fn a_delay_before_ostm0_runs_reports_it_instead_of_spinning() {
        assert!(!try_delay_ms(100));
    }

    #[test]
    #[should_panic(expected = "OSTM0 isn't running")]
    fn delay_ms_before_ostm0_runs_panics() {
        delay_ms(1);
    }
```

(No HAL test starts the time driver, so `RUNNING` stays `false` throughout the test binary.)

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib ostm 2>&1 | tail -5`
Expected: the first two tests fail with a segfault (the OSTM registers aren't on the seam yet), or the new tests fail to compile (`try_delay_ms`/`delay_ms` are `unsafe`). Either confirms the starting point.

- [ ] **Step 2: OSTM on the seam, with safe delays**

In `crates/rza1l-hal/src/ostm.rs`:

1. Route every register access through the seam: `(X as *mut u8).write_volatile(v)` → `crate::mmio::write8(X, v)`, `(X as *const u8).read_volatile()` → `crate::mmio::read8(X)`, and the 32-bit `CMP`/`CNT` likewise with `write32`/`read32`. That covers `enable_clock` (STBCR5), `start_free_running` (TT, TE, CTL, TS), `count`, `stop`, `start_free_running_cmp` and `start_alarm`. For example, `start_alarm` becomes:

```rust
pub(crate) unsafe fn start_alarm(n: u8, delta_ticks: u32) {
    unsafe {
        let b = base(n);
        crate::mmio::write8(b + TT, 1); // stop in case already running
        crate::mmio::write32(b + CMP, delta_ticks);
        // Interval mode: the interrupt fires at the end of the count (CNT=0), not at the start.
        crate::mmio::write8(b + CTL, CTL_INTERVAL);
        crate::mmio::write8(b + TS, 1);
    }
}
```

2. Visibility: `enable_clock`, `start_free_running`, `count`, `stop`, `start_free_running_cmp` and `start_alarm` become `pub(crate)`; `delay_ticks` becomes private. Delete `Ostm<N>` and its impl (the `// Owned typed wrapper` section).
3. Add, after the `STBCR5_*` constants:

```rust
/// Whether OSTM0 is counting: the time driver sets it when it starts the channel.
static RUNNING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Record whether OSTM0 is counting. Only the time driver starts or stops it.
pub(crate) fn set_running(running: bool) {
    RUNNING.store(running, core::sync::atomic::Ordering::Release);
}

fn running() -> bool {
    RUNNING.load(core::sync::atomic::Ordering::Acquire)
}
```

4. Replace `delay_ms`, `try_delay_ms` and the `DelayNs` impl with:

```rust
/// Busy-wait for `ms` milliseconds on OSTM0's counter. Works with interrupts masked.
///
/// # Panics
/// If OSTM0 isn't running (the time driver starts it), or [`crate::clocks::init`] has not been
/// called.
pub fn delay_ms(ms: u32) {
    assert!(running(), "ostm::delay_ms: OSTM0 isn't running; the time driver starts it");
    // SAFETY: reads OSTM0's counter, which the time driver owns and never writes after start.
    unsafe { delay_ticks(ms_to_ticks(crate::clocks::get().p0_hz(), ms)) }
}

/// Like [`delay_ms`], but returns `false` without waiting if OSTM0 isn't running or no clock tree
/// is registered, instead of panicking. For code that must not panic, such as a panic handler.
pub fn try_delay_ms(ms: u32) -> bool {
    if !running() {
        return false;
    }
    match ms_ticks_if_clocked(crate::clocks::try_get(), ms) {
        Some(ticks) => {
            // SAFETY: as in `delay_ms`.
            unsafe { delay_ticks(ticks) };
            true
        }
        None => false,
    }
}
```

and

```rust
impl embedded_hal::delay::DelayNs for Delay {
    /// # Panics
    /// As [`delay_ms`].
    fn delay_ns(&mut self, ns: u32) {
        assert!(running(), "ostm::Delay: OSTM0 isn't running; the time driver starts it");
        let ticks = ns_to_ticks(crate::clocks::get().p0_hz(), ns);
        // SAFETY: as in `delay_ms`.
        unsafe { delay_ticks(ticks) };
    }
}
```

   `Delay`'s doc becomes "A `DelayNs` on OSTM0's counter, which the time driver runs." Its `new` doc drops the `start_free_running` sentence.
5. Module docs: replace the usage example with:

```rust
//! The time driver ([`crate::time_driver`]) owns both channels: OSTM0 free-runs as the clock,
//! OSTM1 is the alarm. Code that needs a busy-wait (with interrupts masked, or before the
//! executor runs) uses [`delay_ms`] or [`Delay`] on OSTM0's counter.
```

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib ostm 2>&1 | grep -E "test result|FAILED"`
Expected: `ok`.

- [ ] **Step 3: The time driver owns the channels**

In `crates/rza1l-hal/src/time_driver.rs`:
- Delete the `OSTM0_IRQ`/`OSTM1_IRQ` constants. Use `typelevel::OSTMI0::ID`/`OSTMI1::ID` (in `try_set_alarm`'s `gic::enable` and in the alarm ISR's `gic::disable`).
- Add imports `use crate::Peri;`, `use crate::interrupt::{self, Binding, Handler, Interrupt, typelevel::{OSTMI0, OSTMI1}};`, `use crate::peripherals::{OSTM0, OSTM1};`.
- Replace `fn ostm0_overflow_isr()` and `fn ostm1_alarm_isr()` (keep their doc comments, now on the types) with:

```rust
/// OSTM0's compare match, at each 32-bit wrap: reads the clock, and nothing else. The wrap itself
/// is detected in software by [`OstmDriver::raw_ostm_ticks`]; this only guarantees the "read at
/// least once per wrap period" requirement [`LAST_RAW`] documents, so its latency is not a
/// correctness concern.
pub struct OverflowHandler;

impl Handler<OSTMI0> for OverflowHandler {
    unsafe fn on_interrupt() {
        let _ = OstmDriver::raw_ostm_ticks();
    }
}

/// OSTM1's interval interrupt: the one-shot alarm expired.
pub struct AlarmHandler;

impl Handler<OSTMI1> for AlarmHandler {
    unsafe fn on_interrupt() {
        // Stop OSTM1 at once (interval mode would reload), and mask it until the next alarm.
        unsafe {
            ostm::stop(1);
            gic::disable(OSTMI1::ID);
        }
        let now = DRIVER.now();
        critical_section::with(|cs| {
            let mut queue = DRIVER.queue.borrow_ref_mut(cs);
            let mut next = queue.next_expiration(now);
            while !DRIVER.try_set_alarm(&cs, next) {
                next = queue.next_expiration(DRIVER.now());
            }
        });
    }
}
```

- Replace `pub unsafe fn init()` with:

```rust
/// Start the Embassy time driver on OSTM0 (free-running clock, interrupting at each wrap) and
/// OSTM1 (the alarm, dormant until a timer is scheduled).
///
/// # Panics
/// If [`crate::clocks::init`] has not been called.
pub fn init(
    _ostm0: Peri<'static, OSTM0>,
    _ostm1: Peri<'static, OSTM1>,
    irq: impl Binding<OSTMI0, OverflowHandler> + Binding<OSTMI1, AlarmHandler> + Copy + 'static,
) {
    RATIO.store(TickRatio::from_clocks(&crate::clocks::get()));
    // SAFETY: both channels are owned. CMP=0 makes OSTM0 interrupt as it wraps to 0.
    unsafe {
        ostm::enable_clock();
        ostm::start_free_running_cmp(0, 0);
    }
    ostm::set_running(true);
    interrupt::bind::<OSTMI0, OverflowHandler>(irq);
    interrupt::bind::<OSTMI1, AlarmHandler>(irq);
    // SAFETY: the two channels' own interrupts, below PMR so they reach the CPU. OSTM1 is
    // unmasked by `try_set_alarm` when an alarm is armed.
    unsafe {
        gic::set_priority(OSTMI0::ID, OSTM_IRQ_PRIORITY);
        gic::set_priority(OSTMI1::ID, OSTM_IRQ_PRIORITY);
        gic::enable(OSTMI0::ID);
    }
}

/// Stop the clock and the alarm: OSTM0 and OSTM1 halt and their interrupts are masked. For a
/// loader handing the chip to another program.
///
/// # Safety
/// Embassy time stops: no timer may be awaited afterwards, and `Instant::now` stands still.
pub unsafe fn stop() {
    ostm::set_running(false);
    unsafe {
        gic::disable(OSTMI0::ID);
        gic::disable(OSTMI1::ID);
        ostm::stop(0);
        ostm::stop(1);
    }
}
```

- Update the `OSTM0 is initialised before interrupts are enabled` safety comment in `raw_ostm_ticks` to "OSTM0 is owned by this driver, and reading its counter has no side effect." Also update `[`ostm0_overflow_isr`]` in `LAST_RAW`'s doc to `[`OverflowHandler`]`.

- [ ] **Step 4: `Board::new` hands the channels to the time driver; callers**

In `crates/deluge-bsp/src/board.rs`, add above `impl Board`:

```rust
#[cfg(target_os = "none")]
rza1l_hal::bind_interrupts!(struct TimeIrqs {
    OSTMI0 => rza1l_hal::time_driver::OverflowHandler;
    OSTMI1 => rza1l_hal::time_driver::AlarmHandler;
});
```

In `Board::new`, delete `rza1l_hal::ostm::enable_clock();` and `rza1l_hal::time_driver::init();` from the `unsafe` block, and after the block add `rza1l_hal::time_driver::init(p.OSTM0, p.OSTM1, TimeIrqs);`. Change the doc's "OSTM clock and the Embassy time driver" to "the Embassy time driver on OSTM0/OSTM1".

`ostm::delay_ms` is safe now. In `crates/deluge-bsp/src/{cv_gate,oled,audio}.rs`, wherever an `unsafe` block now holds only `ostm::delay_ms` calls, drop the block (the compiler's `unused_unsafe` warnings list them). In `cv_gate.rs`'s doc, `rza1l_hal::ostm::start_free_running(0)` (in two places) → `rza1l_hal::time_driver::init`. Do the same in `audio.rs:42`'s "**Call after:**" line.

In `firmwares/app-loader/src/main.rs` `quiesce_for_handoff`, replace `rza1l_hal::ostm::stop(0); rza1l_hal::ostm::stop(1);` with `rza1l_hal::time_driver::stop();` (keep the comment).

- [ ] **Step 5: Suite, build check, commit**

Run: `tools/test.sh > /tmp/claude-1000/p4b-t6.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

Run: `rg -n "ostm::(stop|start_|count|enable_clock)|Ostm<|time_driver::init\(\)" -g '*.rs' crates firmwares hw-tests examples wren | grep -v "^crates/rza1l-hal/src/\(ostm\|time_driver\).rs"`
Expected: no output.

Run the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/ostm.rs crates/rza1l-hal/src/time_driver.rs crates/deluge-bsp/src firmwares/app-loader/src/main.rs
git commit -m "feat(rza1l-hal): the time driver owns OSTM0 and OSTM1 and binds typed handlers; ostm::delay_ms is safe and refuses to spin on a stopped counter"
```

---

### Task 7: `Pin::steal`, docs, CHANGELOG, and the final check

**Files:**
- Modify: `crates/rza1l-hal/src/gpio.rs:213-222`, `crates/deluge-bsp/src/board.rs` (`BoardDma` doc), `CHANGELOG.md`, `crates/rza1l-hal/README.md`, `crates/deluge-bsp/README.md`, `crates/deluge-sdk/README.md`, `docs/advanced-guide.md`, `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md`

- [ ] **Step 1: `Pin::new` becomes `Pin::steal`**

In `crates/rza1l-hal/src/gpio.rs`, rename `pub unsafe fn new() -> Self` to `pub unsafe fn steal() -> Self`, and change its doc's first line to "A typed handle to a pin without configuring it, for a pin another part of the program set up." No caller uses `Pin::new` (verified when this plan was written).

Run: `rg -n "pub unsafe fn new\(" crates/rza1l-hal/src`
Expected: exactly `sdhi.rs`, `usb/driver.rs` and `usb/host.rs` (Phase 4c's).

Run: `rg -l "dmac::raw::" -g '*.rs' crates firmwares hw-tests | sort`
Expected: only `crates/deluge-bsp/src/{audio,audio_block,sd}.rs`, `crates/rza1l-hal/src/{sdhi,ssi,scux}.rs`, `crates/rza1l-hal/src/scux/{dvu_path,src_path,usb_tx_path}.rs` and `crates/rza1l-hal/src/dmac.rs` (Phase 4c's).

- [ ] **Step 2: Docs**

`crates/deluge-bsp/src/board.rs`: `BoardDma`'s doc paragraph ("Until Phase 4b each channel …") becomes: "The OLED, PIC and MIDI channels are taken by `bus::init` and `uart::init_*`. Until Phase 4c the SCUX, SSI and SD channels are still programmed by number, through `dmac::raw`, by their BSP drivers: don't build a `dmac::Dma` from one of those roles alongside its driver."

`CHANGELOG.md`, top of `### Changed`:

```markdown
- `rza1l-hal` drivers are built from their peripheral singletons. **Breaking:** `rspi::Rspi::new`,
  `uart::Uart::new` (split into `UartRx`/`UartTx`), `mtu2::OneShot::new` and
  `time_driver::init(OSTM0, OSTM1, Irqs)` replace the channel-number functions, which are now
  private; `Rspi<CH, BITS>`, `ScifUart`, `Ostm<N>`, the MTU2 free-running functions and the RSPI
  interrupt-driven send path are removed; `gpio::Pin::new` is `Pin::steal`. `ostm::delay_ms` is
  safe and panics until the time driver has started OSTM0. `OneShot::new` panics on a prescaler
  the channel lacks instead of running at /1. Escape hatches for fault and hand-off code:
  `dmac::Dma::steal`, `dmac::stop_all`, `rspi::Rspi::steal`, `time_driver::stop`.
- `deluge_bsp::Board` also hands out RSPI0, SCIF0/1 and MTU2 channels 2/3. **Breaking:**
  `bus::init(shared_spi, oled_dma)` brings RSPI0 up (it was `cv_gate::init`'s job, so an
  OLED-only app relied on the boot loader); `uart::init_midi`/`init_pic`, `midi_gate::setup` and
  `encoder::irq_init` take their peripherals; the BSP's `uart::{read_byte, write_bytes}`
  re-export is replaced by `read_pic_byte`, `write_pic`, `try_read_pic`, `try_write_pic` and
  `set_pic_baud`; `midi_gate::{schedule_gate_off, cancel}` are safe.
- The SDK runtime keeps the board's unclaimed peripherals in `deluge::parts` (device-only); a
  capability takes its own when first used, and an app that drives one itself takes it first
  with `deluge::parts::with`.
```

`crates/rza1l-hal/README.md`:
- Module table rows: `gic`: "(or, until Phase 4b, `gic::register(id, handler)`)" → "(or, until Phase 4c, `gic::register(id, handler)`)". `ostm`: "OS Timer: two 32-bit channels clocked from P0φ … Owned by the time driver; `delay_ms`/`Delay` busy-wait on OSTM0's counter." `time_driver`: append "`time_driver::init(p.OSTM0, p.OSTM1, Irqs)` with `OverflowHandler`/`AlarmHandler` bound." `mtu2`: "Multi-Function Timer Pulse Unit 2: five 16-bit channels. `OneShot::new(Peri<MTU2_n>, prescaler, priority, Irqs)` for compare-match one-shots and periodic ticks." `dmac`: replace "channel-number functions in `dmac::raw` until Phase 4b" with "`Dma::steal`/`stop_all` for fault and hand-off code; channel-number functions in `dmac::raw` until Phase 4c." `uart`: "SCIF async serial, five channels (SCIF0–4). `Uart::new(Peri<SCIFn>, rx_dma, tx_dma, Irqs, baud)` receives into a DMA ring (SCIF0/1) and transmits by DMA or through the FIFO; `split()` gives `UartRx`/`UartTx`." `rspi`: "SPI master (RSPI0–4): `Rspi::new(Peri<RSPIn>, bit_rate)`, 32-bit or 8-bit frames, blocking sends." `gpio`: "`Pin::steal()`" in place of the `Pin` construction mention.
- Replace the "Minimal bare-metal init sequence" block with:

```rust,ignore
rza1l_hal::bind_interrupts!(struct TimeIrqs {
    OSTMI0 => rza1l_hal::time_driver::OverflowHandler;
    OSTMI1 => rza1l_hal::time_driver::AlarmHandler;
});

fn main() -> ! {
    // The crystal's clock tree, the CPG, module clocks and the GIC; the singletons, once.
    let p = rza1l_hal::init(MY_CONFIG);
    unsafe {
        rza1l_hal::mmu::init_and_enable();
        rza1l_hal::cache::l1_enable();
        rza1l_hal::cache::l2_init();
    }
    // Embassy time driver: OSTM0 free-runs, OSTM1 is the alarm.
    rza1l_hal::time_driver::init(p.OSTM0, p.OSTM1, TimeIrqs);
    // Hand the rest of `p` to your drivers, e.g. `rspi::Rspi::new(p.RSPI0, 4_000_000)`.
    // …
}
```
- The paragraph "All peripheral init functions (`sdhi::init`, `ssi::init`, `uart::init`, …) are `unsafe` …" becomes: "Drivers take their peripheral singleton (`Peri<'d, T>`) and the interrupt binding they need, so `Rspi::new`, `Uart::new`, `OneShot::new` and `time_driver::init` are safe. SDHI, SSI, SCUX and USB still have `unsafe` init functions until Phase 4c."

`crates/deluge-bsp/README.md`:
- Module table: `uart` row → "SCIF0 MIDI DIN + SCIF1 PIC UART: `init_midi`/`init_pic` from the `Board`'s singletons; DMA receive rings, PIC transmit by DMA". `bus` row → "RSPI0 shared by the OLED and the CV DAC: `init(shared_spi, oled_dma)`, then `lock_rspi0`, `enter_8bit` / `enter_32bit`". `midi_gate` row: append "; `setup(board.gate_timer)`". `board` row → "…the Deluge's peripherals by role (USB0/1, RSPI0, SCIF0/1, MTU2 2/3, the 16 DMA channels)".
- The boot-sequence example becomes:

```rust
let board = deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG));
deluge_bsp::uart::init_pic(board.pic_uart, board.dma.pic_rx, board.dma.pic_tx, 31_250);
deluge_bsp::bus::init(board.shared_spi, board.dma.oled);
unsafe {
    deluge_bsp::audio::init();
    deluge_bsp::cv_gate::init();
}
```

`crates/deluge-sdk/README.md`: before `## Toolchain`, add:

```markdown
## Driving the board yourself

On the device, `#[deluge::app]` brings the board up before your `setup` runs and keeps the
peripherals its capabilities drive (RSPI0, the MIDI and PIC UARTs and their DMA channels, two MTU2
timers) in `deluge::parts`. Each capability takes its own when first used. To drive one through
`deluge-bsp` yourself, take it first, for example
`deluge::parts::with(|p| p.gate_timer.take())`. The matching capability then panics, naming the
peripheral, instead of fighting you for it.
```

`docs/advanced-guide.md`: in the RSPI0 section, after the paragraph ending "…needs the bus while the world is stopped.", add: "The bus exists once `bus::init(board.shared_spi, board.dma.oled)` has run; the SDK runs it the first time an app uses the OLED or CV. Before that, locking it panics." In the module table, the `bus` row becomes `RSPI0 ownership guard (`init`, `lock_rspi0`, `enter_8bit/32bit`, `steal_rspi0`)`. After the PIC subsection, add:

```markdown
### Taking a peripheral from the SDK

The runtime keeps the board's unclaimed peripherals in `deluge::parts` (device-only). An app that
drives one through the BSP itself takes it in `setup`:

```rust
let poll_timer = deluge::parts::with(|p| p.encoder_poll_timer.take()).expect("MTU2 channel 3 is free");
unsafe { deluge_bsp::encoder::irq_init(poll_timer) };
```

A capability that needs a peripheral the app took panics, naming it.
```

Spec, after the Phase 4 "**4a done …**" paragraph:

```markdown
**4b done (2026-10-15 plan):** RSPI (`Rspi`), the SCIF UART (`Uart` → `UartRx`/`UartTx`), MTU2
(`OneShot`) and the OSTM time driver are built from their singletons and typed bindings; their
channel-number functions are private, and `rspi`/`uart`/`ostm`/`gpio` have no public
`unsafe fn new()`. `Board` hands out RSPI0, SCIF0/1 and MTU2 2/3 by role, and the SDK keeps
unclaimed parts in `deluge::parts`. The BSP's bus owns RSPI0 with the OLED's DMA channel, which
also brings RSPI0 up for an OLED-only app (it had relied on the boot loader). Escape hatches:
`Dma::steal`, `dmac::stop_all`, `Rspi::steal`, `time_driver::stop`. Phase 4c migrates SDHI,
SSI+SCUX and USB, deletes `dmac::raw` and `gic::register`, and migrates DelugeFirmware#next.
```

- [ ] **Step 3: Final verification**

Run: `tools/test.sh > /tmp/claude-1000/p4b-final.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/p4b-final.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'`
Expected: `exit 0`, `failed 0`.

Run: `tools/link-layout.sh check && tools/check-manifests.sh; echo "exit $?"`
Expected: the layout matches; `exit 0`.

Run the **Firmware build check**.

Simulator unchanged:

```bash
W=/tmp/claude-1000/p4b-sim; rm -rf $W; mkdir -p $W
(cd examples/baremetal/oled_hello && timeout 300 cargo run -q --manifest-path ../../../tools/cargo-deluge/Cargo.toml \
   --target x86_64-unknown-linux-gnu -- deluge sim --headless --out $W/now > $W/now.log 2>&1; echo "exit $?")
cat $W/now/frame.state
```

Expected: `exit 0`; `cv 0 0 0 0` / `gate 0 0 0 0` / `synced 0`.

- [ ] **Step 4: Commit**

```bash
git add crates/rza1l-hal/src/gpio.rs crates/deluge-bsp/src/board.rs CHANGELOG.md crates/rza1l-hal/README.md \
  crates/deluge-bsp/README.md crates/deluge-sdk/README.md docs/advanced-guide.md \
  docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md
git commit -m "docs: Phase 4b — RSPI, UART, MTU2 and the time driver are built from their singletons; deluge::parts; Pin::steal"
```

- [ ] **Step 5: Hardware smoke test (the human partner's merge gate)**

List these in the final message; the executor can't run them. None of Phases 1–4a has been checked on hardware yet, so this list covers them too.

1. `blinky` boots and blinks: the CPG/STB/GIC bring-up (4a) and the time driver's typed handlers (OSTM0 wrap, OSTM1 alarms).
2. `oled_hello`, loaded with J-Link `dev-upload` (not through app-loader), draws its text: RSPI0 brought up by the OLED capability alone (Review Focus 1).
3. `deluge-controller`: pads, buttons and LEDs (PIC link, including the 31 250 → 200 000 bps switch), OLED frames, DIN MIDI in and out, CV and gate out, and all six encoders. Turn an encoder slowly enough to stop on a B transition, to exercise the MTU2 channel 3 poll.
4. `midi_cv`: MIDI notes drive CV and gate.
5. app-loader: the menu works with the encoders and buttons, and chain-loading an app works (`dmac::stop_all` and `time_driver::stop` quiesce the chip).
6. An app that panics shows `APP PANIC` on the OLED and strobes the SYNC LED (`steal_rspi0`, `try_write_pic`, `try_delay_ms`).
7. 4a's list: `usb_log` enumerates and logs, an app built with `usb-serial` enumerates as CDC-ACM, and `sd_demo` mounts the card.
