# RZ/A1 HAL architecture: target layout and roadmap

**Status:** agreed in conversation on 2026-10-14, following an architecture audit of the repo.
**Plans:** each phase gets its own detailed plan in `docs/superpowers/plans/`, written once the
phase before it has landed. Phase 1's plan is
`docs/superpowers/plans/2026-10-14-rza1-phase1-board-facts-out-of-hal.md`.

## 1. Why

The repo already has the right three layers: `rza1l-hal` (chip), `deluge-bsp` (board) and the
`deluge` facade (`deluge-sdk`) with device, sim and Linux backends. But the HAL isn't shaped like an
Embassy HAL yet, and the layer boundaries leak:

| # | Problem | Evidence |
|---|---------|----------|
| 1 | No ownership model: drivers are `unsafe fn new()`, channels are `u8`, IRQs are bound at runtime with `fn()` pointers. Conflicts are runtime panics, not compile errors. | `dmac::start_transfer(ch: u8, …)`, `rspi::register_irq(ch, fn())`, `usb-log` vs `usb-serial` both owning USB0 |
| 2 | Deluge facts baked into the chip HAL | `ostm::OSTM_HZ`, `mtu2::MTU2_P0_HZ`, `rspi::P1_HZ` and `time_math`'s `21161/640` all assume the Deluge's 13.2256 MHz crystal; `gic.rs` hardcodes the Deluge's 64 MB SDRAM range; `startup.rs` calls `deluge_cpu_fault_report` |
| 3 | No PAC: every register is a hand-typed offset | `usb/regs.rs`, `scux/regs.rs`, per-module `const OFF_*`; `vendor/rza1.svd` (71 peripherals) is unused |
| 4 | Runtime lives in the HAL | `startup.rs`, linker scripts, `unlock-bootloader` |
| 5 | `deluge-bsp` mixes board wiring, generic USB classes, off-chip part drivers, chip-level SCUX paths and the simulator link | `usb/classes/*`, `usb/host/uac/*`, `scux_*_path.rs`, `sim.rs` |
| 6 | Facade leaks and duplicated board config | `deluge-sdk/src/lib.rs` does `pub use rza1l_hal;`; 24 `memory.x` copies, 22 byte-identical |

## 2. Target layout

```
rza1/                         ← later becomes the rza1-rs repo (Phase 6)
  rza1-pac      chiptool output from a patched rza1.svd (+ USB/SCUX YAML)      (Phase 5)
  rza1-rt       startup, vectors, linker scripts, fault hook                    (Phase 3)
  rza1l-hal     init(Config) -> Peripherals, clocks, GIC + bind_interrupts,     (Phases 1, 4)
                typed DMA, gpio/uart/rspi/ssi/scux/sdhi/spibsc/mtu2/ostm,
                mmu/cache, usb (device + host driver), time-driver-ostm feature
  rza1-alloc    today's deluge-alloc (SRAM→SDRAM spill isn't Deluge-specific)   (Phase 3)
usb/            board-agnostic embassy-usb / embassy-usb-host classes           (Phase 2)
deluge/ (stays under crates/ until Phase 6)
  deluge-bsp    pin map, ClockConfig, DMA/IRQ assignment, memory.x, PIC, jacks, OLED, CV DAC
  deluge-sdk    facade + plat/{device,sim,linux}; no HAL re-export
  deluge-protocol, deluge-image, deluge-sim*, cargo-deluge
libs, apps, wren, linux: unchanged
```

**Layering rules**, enforced by `tools/check-layering.sh` as each phase lands:

1. Nothing under `rza1/` (today `crates/rza1l-hal`) names `deluge` in code or depends on a
   `deluge-*` crate. (Phase 1)
2. `wren/` and `examples/baremetal/` reach the platform only through the `deluge` facade.
   (Phase 1, extending today's Wren-only rule)
3. `usb/` crates depend only on `embassy-usb*` traits, never on `rza1l-hal` or `deluge-*`. (Phase 2)
4. `deluge-bsp` doesn't depend on simulator crates. (Phase 2)

## 3. Decisions

- **Monorepo first, split later.** Embassy keeps every HAL in one repo. A HAL whose API still
  changes with each board need would turn every change into two PRs (the `wren-rs` cross-repo
  dependency already shows that cost). Phase 6 splits only once the exit criteria there are met.
- **Clocks are runtime config, Embassy-style.** The HAL takes a `ClockConfig { extal_hz }` at
  init and derives every peripheral clock from it. The BSP also uses the same `const fn`
  derivations in `const` contexts (`midi_gate::TICK_HZ`, `encoder::POLL_TICKS`).
- **The time driver stays exact.** P0φ is a half-integer on the Deluge (33,064,062.5 Hz), so the
  driver keeps a reduced ticks-per-µs rational, now computed from `ClockConfig`. A split
  quotient/remainder conversion keeps it from overflowing for any crystal (it overflowed after
  ~28 years at 21161/640, but a 13.33 MHz crystal's 13333333/400000 would overflow in ~16 days
  with the old single-multiply formula).
- **Keep the MMIO test seam** (`mmio.rs`) through every phase. In Phase 5 the generated PAC's
  register access is routed through it.
- **Chiptool, not svd2rust**, for the PAC: its `Copy` register handles with no ownership match
  the HAL's existing free-function style, and ownership comes from the Phase 4 singletons instead.
- **Keep the name `rza1l-hal`** until a second RZ/A1 variant (A1H/A1M) actually has a user.
  Renaming to `rza1-hal` with chip features is a Phase 6 decision.
- **Pure-logic BSP modules stay put** (`encoder_detent`, `encoder_quadrature`, `sample_fmt`,
  `midi_stream`) until something outside the BSP needs one.
- **Out of scope:** the `../flare` sibling path dependencies (owned by the three-repo topology
  spec, `2026-07-19-repo-topology-reorg-design.md`).

## 4. Phases

Each phase leaves every firmware building and every test passing (`tools/test.sh`,
`tools/build-examples.sh`, `cargo build-app-loader`, `cargo build-wren`, `cargo build-msc`,
`cargo build-fw -p deluge-controller`), and ends with a hardware smoke test on a real Deluge.

### Phase 1: board facts out of the HAL (detailed plan written)

- `rza1l_hal::clocks::{ClockConfig, init, get}`; `deluge_bsp::system::CLOCKS` holds the crystal.
- OSTM delays, RSPI baud and the MTU2 tick rate take their clocks from `clocks`;
  `OSTM_HZ`, `MTU2_P0_HZ` and `P1_HZ` are removed.
- The time driver's tick rational is computed from `ClockConfig` with overflow-free math.
- The GIC's code-address check covers the chip's CS2/CS3 SDRAM window, not the Deluge's 64 MB.
- The fault hook symbol becomes `rza1_cpu_fault_report`.
- `deluge-sdk` stops re-exporting `rza1l_hal`; layering rules 1 and 2 are enforced.

**Exit:** `grep -rniE 'deluge'` over HAL code (comments excluded) finds nothing, and
`check-layering.sh` enforces it.

### Phase 2: split `deluge-bsp`

- Move `usb/classes/{audio,midi,msc}.rs`, `usb/bot.rs`, `usb/midi_packets.rs` and
  `usb/host/{midi,uac/*}` into board-agnostic crates under `usb/`. Proposed names (final in the
  Phase 2 plan): `usb-uac2-class`, `usb-midi-class`, `usb-msc-class`, `usb-host-classes`.
  Before moving each file, the plan must list every `rza1l_hal`/`deluge_bsp` name it uses and
  replace each one with an `embassy-usb` trait or a constructor argument.
- Move `scux_dvu_path.rs`, `scux_src_path.rs` and `scux_usb_tx_path.rs` into
  `rza1l_hal::scux`, with the DMA channel numbers passed in from `deluge_bsp::system`.
- Move `sim.rs` and the `sim-link` feature out of `deluge-bsp` into `deluge-sdk/src/plat/sim`.
- Off-chip part drivers (SSD1309 OLED, MAX5136 CV DAC, SPI-NOR profile): extract each only if it
  separates cleanly from Deluge wiring (the OLED's chip select goes through the PIC, so it may
  not). First check crates.io for existing embedded-hal 1.0 drivers.

**Exit:** layering rules 3 and 4 are enforced; `deluge-bsp` holds only board wiring and drivers
tied to it.

**Done (2026-10-14 plan):** classes went into one `usb-classes` crate (features per class, unpublished);
the simulator moved behind `deluge_bsp::host_panel`; MAX5136 became `drivers/max5136`. SPI-NOR needed no
extraction — `rza1l_hal::spibsc` already holds the generic controller and `FlashMap`, and
`deluge_bsp::flash` is only the Deluge's part size and flash layout. The SSD1309 OLED stayed in
the BSP: its CS, DC and reset lines are PIC32 UART commands (`pic::CMD_*`), not GPIOs, so an
embedded-hal `SpiDevice` driver would still need Deluge-specific glue around every transfer.

### Phase 3: runtime and memory layout

- New `rza1-rt` crate: `startup.rs`, vectors, `rza1l.x`/`rza1l_rtt.x`, the fault hook and the
  `unlock-bootloader` feature. `mmu`, `cache` and `memmap` stay in the HAL, since the BSP calls
  them after startup.
- The time driver goes behind a `time-driver-ostm` feature, on by default.
- `deluge-bsp`'s `build.rs` provides `deluge_memory.x`/`deluge_memory_rtt.x` (MEMORY,
  `INTERNAL_RAM_END`, stack sizes as `PROVIDE`). Each binary's `memory.x` becomes
  `INCLUDE deluge_memory.x` plus any override (wren-firmware's 256 KB program stack,
  app-loader's layout). Verify the overrides with `arm-none-eabi-nm` on the built ELF.
- Rename `deluge-alloc` to `rza1-alloc`, publishing `deluge-alloc` once more as a re-export if
  it is already on crates.io.

**Exit:** 22 identical `memory.x` copies are gone; the HAL contains no `global_asm!` startup.

### Phase 4: Embassy ownership model

- Depend on `embassy-hal-internal`; define `Peripherals` with `peripherals!` (OSTM0/1, RSPI0–4,
  SCIF0–7, SSIF0–5, SDHI0/1, USB0/1, DMA_CH0–15, MTU2_0–4, SPIBSC0, SCUX, ADC; GPIO pins keep
  their existing `Pin<PORT, BIT, Mode>` const generics).
- `rza1l_hal::init(Config) -> Peripherals` absorbs `clocks::init`, `stb::init` and `gic::init`.
  `deluge_bsp::system::init_clocks` becomes `Board::new(rza1l_hal::init(BOARD_CONFIG))`.
- A `bind_interrupts!` macro over the existing GIC handler table: typed
  `InterruptHandler<T>` handlers registered when the driver is built.
- Typed DMA channels (`Peri<'d, DMA_CHn>`) first, since every DMA-using driver depends on them.
  Then migrate one driver per task: RSPI, SCIF/UART, SDHI, SSI+SCUX, USB, MTU2, OSTM/time driver.
  Old `unsafe` free functions move to `#[doc(hidden)] pub mod raw` during migration, then go.
- Decide whether the facade keeps `pub use deluge_bsp` once `Board` exists.

**Exit:** no public `unsafe fn new()` constructors; enabling both `usb-log` and `usb-serial`
fails to compile instead of panicking.

### Phase 5: PAC

- Generate `rza1-pac` with chiptool from `vendor/rza1.svd`, fixing the SVD with YAML transforms.
  Hand-write YAML for any block the SVD gets wrong (expect USB and SCUX).
- Post-process the generated register-access module so reads and writes go through `mmio`,
  keeping the host-side register-sequence tests.
- Before deleting each hand-written offset table, add a test that the PAC's addresses equal the
  old constants (the existing address tests are the cross-check).
- Migrate one driver per task.

**Exit:** no `const OFF_*`/`*_BASE` register tables left in `rza1l-hal`.

### Phase 6: split out the rza1-rs repo

Split when all of these hold:

- Phases 1–5 are done.
- No `rza1l-hal` change in the last ~2 months was forced by a Deluge-specific need.
- The `rza1/` crates have their own CI job that builds and tests them without `deluge-*` crates.

Then `git subtree split -P rza1` into the new repo, publish `rza1-pac`/`rza1-rt`/`rza1l-hal`, and
switch `deluge-bsp` to crates.io versions (with a `[patch]` for local development). Decide the
`rza1l-hal` → `rza1-hal` rename here.

## 5. Risks

- **Clock mis-derivation breaks timing everywhere.** Phase 1 pins every derived value against
  the old constants in tests and ends with a hardware check of OLED, CV, MIDI and timing.
- **Chiptool output vs the test seam.** If post-processing the generated module proves brittle,
  fall back to a small hand-written `Reg<T>` type in `rza1-pac::common` that calls `mmio`.
- **SVD quality.** Vendor SVDs for RZ/A1 are likely incomplete; budget time in Phase 5 for
  transforms, and keep hand-written blocks where the SVD is wrong.
- **USB class extraction** may uncover class code that relies on RUSB1 quirks (pipe budget, ISO
  hooks via `rza1l_hal::usb::pipe::register_iso_*_hook`). Those hooks need a trait or stay in a
  thin RZ/A1-specific adapter crate.
