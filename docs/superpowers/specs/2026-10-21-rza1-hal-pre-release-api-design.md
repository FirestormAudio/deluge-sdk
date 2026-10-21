# rza1-hal pre-0.1 API pass: design

**Status:** agreed in conversation on 2026-10-21. It follows two read-only audits of the
same day: an embassy-idiom audit of `rza1l-hal`/`rza1-rt`/`rza1-pac`, and a release-readiness
audit.
**Builds on:** `2026-10-14-rza1-hal-architecture-design.md` (Phases 1–5 done; Phase 6 is the
split and publish). This pass slots in **before** Phase 6.
**Plans:** one per phase in `docs/superpowers/plans/`, each written once the previous phase
lands.

## 1. Why

Phase 6 publishes `rza1-pac`, `rza1-rt` and the HAL to crates.io. Once 0.1 is out, every API
fix below becomes a semver break. The ownership and interrupt core is already embassy-shaped:
- `peripherals!` with `init(Config) -> Peripherals`;
- `Peri<'d, T>` on every driver;
- `bind_interrupts!`;
- sealed `Instance` traits;
- an `embassy_usb_driver::Driver` impl;
- `time_driver_impl!`.

The drivers built on that core aren't embassy-shaped yet. The audit found:

| # | Problem | Evidence |
|---|---|---|
| 1 | Async SD DMA isn't cancel-safe: dropping the future leaves DMA and the SD command writing into the caller's buffer | `sdhi.rs:1659-1680` takes `*mut u8`; `dmac/raw.rs:320` has no drop guard; `Dma` has no `Drop` (`dmac.rs:143`) |
| 2 | Audio and USB-iso aren't drivers. They are module-level `init` functions, `static mut` buffers and ISR hooks hard-wired to SSIF0; 31 `static mut` in the HAL | `usb/pipe.rs:73-130` `register_iso_{in,out}_hook`; `ssi.rs:173-291`; `scux/{src,dvu,usb_tx}_path.rs` |
| 3 | Deluge-specific code in the chip crates | `scux::usb_tx_path`; `unlock-bootloader` (`spibsc.rs:624-658`); `rza1-rt` hard-codes `.BootLoad_ValidProgramTest.` (`startup.rs:55`), bootloader words and a status-LED blink; board facts in docs (`ssi.rs:1-30`, `rspi.rs:6-72`, `dmac/raw.rs:152,280`, `bsc.rs:4`, `usb/mod.rs:14`) |
| 4 | GPIO has no ownership, and its writes race | pins not in `peripherals!` (`lib.rs:36-47`); `Pin::steal()` (`gpio.rs:196`); free `unsafe fn set_pin_mux`; read-modify-write without a critical section (`gpio.rs:78,138`) |
| 5 | `Config` and constructors take raw register values | `lib.rs:50` `frqcr: u16` + 11 STBCR bytes, no `Default`; `Sdhi::new(.., sd_option: u16)`, `ssi::init(.., ssicr: u32)` |

The audit also found smaller gaps that §4 covers:
- public driver `steal()`;
- no Blocking/Async typestate;
- missing embedded-hal impls;
- UART errors swallowed (`Error = Infallible`);
- hard `log` dependency;
- forced critical-section impl;
- ungated PAC;
- no IRQ pend/priority;
- the time driver needing a manual `init`;
- `#![allow(dead_code)]` crate-wide;
- ~25 public raw `unsafe fn` in `scux`;
- no `#[entry]` in `rza1-rt`.

## 2. Goals, non-goals, anti-goals

**Goals**
- **Audience:** a general-purpose embassy HAL for **other RZ/A1 developers**, with the Deluge as one
  user among many. Every driver is owned, configurable and chip-neutral. Someone who knows
  embassy-stm32 or embassy-rp should find nothing surprising.
- **Rename:** `rza1l-hal` → **`rza1-hal`** with `rza1l` / `rza1h` chip features that select the PAC chip.
- **No Deluge-specific code** in `rza1/` (code or docs). The Deluge pieces move to `deluge-bsp`
  (or the app-loader for bootloader-only bits).
- **Everything public is safe to drop at any point.** Nothing reachable without ownership.

**Non-goals**
- New peripherals or features beyond what exists today.
- Performance work, beyond not regressing the bench-checked paths.
- The repo split, publishing, and switching `deluge-*` to crates.io versions. Those are Phase 6.
- Hardware-validating A1H. `rza1h` must compile and expose the extra instances (more RSPI
  channels, the larger IRQ table), but only A1L hardware exists.

**Anti-goals**: these would count as failure even if everything compiles:
- **`main` stops building, or link layout drifts silently.** Every phase keeps
  `tools/link-layout.sh check` and `tools/test.sh` green.
- **A safe API that loses the timing the hooks provided.** The USB iso path regressing
  (underruns, or the double-buffer priming of TRM §28.4 BRDY cond. 4 breaking) is a failure.
- **"Generic" drivers that still assume Deluge wiring under the hood**, such as a fixed SSIF0, a fixed
  crystal or a fixed DMA channel.

## 3. Constraints

- **Lockstep downstream migration (user decision):** each phase updates `deluge-bsp`,
  `deluge-sdk`, `usb-classes`, firmwares, hw-tests and examples in the same phase.
- **DelugeFirmware#next gets one bump at the end of the pass (user decision).** It builds against
  this checkout through its gitignored `local-sdk.toml`, and it uses HAL internals this pass
  removes, so it may stay broken while the pass is in progress. A final phase migrates it once,
  against the finished API. Its C ABI (`src/bsp/rust/src/audio/device.rs`, which fills the ring
  by slot offset) must stay expressible; see decision 7.
- **Hardware gating (user decision):** phases that touch timing-sensitive paths each get a bench check on the
  pi-top J-Trace bench before merging. Those paths are SD DMA, SSI/SCUX audio, USB iso and GPIO; see
  memory notes for the bench. Everything else relies on host tests and the QEMU bucket.
- **Layering rules** from the 2026-10-14 spec still hold. `tools/check-layering.sh` must keep
  firing, updated for the new crate name.
- Keep the `mmio.rs` test seam.
- The deploy target is 32-bit (`usize == u32`). Review buffer and offset arithmetic by hand,
  because x86-64 host tests can't catch the overflow.
- Never push or publish during this pass.

## 4. Decisions

### Decided by the user

1. **Rename to `rza1-hal`** with `rza1l`/`rza1h` features. Exactly one must be enabled
   (`compile_error!` otherwise).
2. **Audio ring buffers are lent by the caller as `&'d mut [u32]`**, the embassy-stm32
   `RingBufferedSai` / `ReadableRingBuffer` shape. The board crate places the buffer, for example in
   uncached SRAM. The HAL holds no audio statics. Dropping the driver stops DMA before the
   borrow ends.
3. **USB iso uses an owned stream object.** The RUSB1 driver hands out an RZ/A1-specific
   `IsoStream<'d>` (name is the builder's call) that owns the pipe and a caller-lent ring. The
   USB ISR fills or drains that ring directly, so the "prime both BRDY planes" timing stays in the
   HAL. It is an extension next to the standard `embassy_usb_driver::Driver` impl, not a
   replacement. The hooks are deleted.
4. **`rza1-rt` gets cortex-m-rt-style macros:**
   - `#[entry]` (type-checked `fn() -> !`), an IRQ/exception attribute and a fault-handler hook.
   - The image signature or boot header becomes a user-provided linker section.
   - `deluge-bsp`, or the app-loader, supplies the `.BootLoad_ValidProgramTest.` header, the
     bootloader words and the status-LED fault blink.
5. **Bench-test the risky phases only** (see §3).
6. **Lockstep downstream migration**, with DelugeFirmware bumped once at the end (see §3).
7. **Audio drivers offer both a copy API and a positional window.**
   - The copy API is embassy-style `read`/`write`, with overrun and underrun errors.
   - The positional window gives the hardware DMA position and a writable (or readable) window of
     the ring ahead of it, for callers that write N frames ahead of the hardware. Today
     `deluge-bsp/src/audio_block.rs` (`tx_current_ptr`, `TX_WRITE_AHEAD_FRAMES`), the firmwares'
     USB audio path and DelugeFirmware's C ABI all work this way.
   - The window has no embassy counterpart. It must keep today's latency, and it must be
     reachable from interrupt context.
8. **The USB-iso → audio hand-off stays in interrupt context.** An `IsoStream` can be bound to
   an interrupt-context consumer. The canonical one is an audio driver's positional window plus
   a sample-format conversion. On BRDY, the ISR moves the packet straight from the FIFO into the
   audio ring, as `iso_out_to_ssi` / `iso_in_from_ssi` do today, with no task hop. Sample
   conversion lives where the ISR can reach it: a consumer trait the board crate implements, or
   a HAL-provided converter. **Phases 5 and 6 are therefore coupled:** the window API has to fit
   the ISR binding.
9. **DMA buffers are a checked uncached-buffer type.** The HAL knows each chip's uncached
   mirror ranges, so a buffer type (name is the builder's call) checks at construction that its
   memory is DMA-coherent, or builds itself from the mirror. SD, audio and iso DMA accept only
   that type. There is no per-transfer cache maintenance, so SD throughput is unaffected. This
   replaces today's `unsafe` "uncached memory (OCRAM mirror)" contracts (`sdhi.rs:933,1016,1658`).
   The board crate decides where buffers live; the HAL only checks them.

### My calls, following embassy conventions; the user can override

- **`IsoStream`'s consumers are `deluge-bsp` and the firmwares, not `usb-classes`.**
  `usb-classes` never used the hooks and stays chip-neutral, with no HAL dependency (layering
  rule 3).
- **USB host iso is in scope.** `usb/host.rs`'s HAL-owned static iso IN rings (2 pipes × 8 ×
  1024 B) become caller-lent, using the same buffer type and stream shape as device iso. Phase 6
  covers host and device together, and the Digitakt bench run is its acceptance check.
- **`rza1h` is minimal.** The existing drivers expose A1H's extra instances (RSPI3/4, the
  larger IRQ table), and the crate compiles with `rza1h`. Pin singletons, pin-function trait
  tables and A1H-only blocks (extra ports, extra SCIF/SSIF) are A1L-only for now. A1H pin
  tables can be added later without breaking anything.
- **SD DMA** takes `&mut [u8]` / `&[u8]` (or a block-aligned wrapper). An
  `embassy_hal_internal::drop::OnDrop` guard aborts the DMA and the SD command and waits for
  them to stop. `Dma` gets a `Drop` that stops its channel. The local `OnDrop` copy in
  `usb/host.rs:197` is replaced by the embassy one.
- **SSI** becomes an owned driver over any SSI channel: `Peri<'d, SSIx>`, its DMA channel(s) and
  pins, plus a `#[non_exhaustive] Config` with `Default`.
- **SCUX** becomes composable pieces (SRC, DVU, routing), each owned. The Deluge's USB-TX path is
  assembled from them in `deluge-bsp`.
- **GPIO** follows embassy:
  - pin singletons (`P1_0` …) go into `peripherals!`;
  - `Input` / `Output` / `Flex<'d>` are built from `Peri<'d, impl Pin>`;
  - per-peripheral pin traits (`pin_trait!`-style, e.g. `TxPin<T>`, `SckPin<T>`) let each
    driver set the pin function itself;
  - writes go through the PAC's masked set/clear register (`psr(n)`), so they're atomic;
  - `Pin::steal()` and the free `set_pin_mux` are no longer public.
- **`init` `Config`** gets typed clock and standby settings, `Default`, and `#[non_exhaustive]`.
  Every driver `Config` gets the same treatment, so no constructor takes raw register words.
- **Modes:** drivers that support both get `Blocking`/`Async` typestate with `new_blocking`/`new`.
  `Rspi` gains async. `Uart` gains blocking and stops polling RX every 100 µs, where the hardware
  allows it.
- **Traits:**
  - embedded-hal 1.0 `SpiBus`, plus the async version, on RSPI;
  - embedded-hal 1.0 `Wait` on GPIO, if IRQ0–7 pin interrupts are wired;
  - blocking `embedded-io` on UART;
  - embedded-hal-nb serial where it's natural.
- **Errors:**
  - typed `uart::Error { Framing, Noise, Overrun, Parity }` implementing `embedded_io::Error`;
  - every error enum is `#[non_exhaustive]`, documented, and derives
    `Debug, Clone, Copy, PartialEq, Eq`;
  - `defmt::Format` behind the `defmt` feature.
- **Features:**
  - optional `log` and `defmt` through an embassy-style `fmt.rs` shim, so the HAL no longer
    forces `release_max_level_info` on downstream crates;
  - `unstable-pac` gates `pub use rza1_pac as pac`;
  - `rt` enables the PAC/runtime pieces;
  - `critical-section-impl` is on by default;
  - `unlock-bootloader` is removed.
- **Time driver:** started inside `init()` when `time-driver-ostm` is on. That feature stays on
  by default (the HAL has only one timer option today). OSTM0/1 drop out of `Peripherals` while
  it's enabled.
- **Interrupts:** an `InterruptExt` with `enable/disable/pend/set_priority`, as embassy's
  `interrupt_mod!` provides. Drivers no longer hard-code priorities. Pending an interrupt is what
  makes an `InterruptExecutor` on an SGI possible.
- **No public `steal()` on drivers** (`Dma`, `Rspi`, `UartRx`). Only `Peripherals::steal` and
  singleton `steal` remain, as in embassy. If the app-loader handoff needs a driver-level
  `steal`, it goes behind a hidden or unstable feature.
- **Docs hygiene:**
  - `#![deny(missing_docs)]` on all four crates;
  - remove `#![allow(dead_code)]`;
  - public raw `unsafe fn` channel APIs become `pub(crate)` or methods on drivers;
  - `cargo doc` is warning-free;
  - `#![no_std]` unconditionally, with `#[cfg(test)] extern crate std`.

## 5. Shape of the work

The plans own the details. My proposed order puts foundations first and the risky paths after
them:

1. **Rename and foundations.** `rza1-hal` with chip features, `fmt.rs`/feature gates,
   `unstable-pac`, `InterruptExt`, the time driver inside `init`, `Config` with `Default`, no
   public `steal`.
2. **GPIO ownership, atomic writes and pin traits.** Bench check: OLED, CV, MIDI, pads.
3. **The uncached-buffer type, plus DMA and SD cancel safety.** Bench check: SD read and write
   (throughput no worse than the High-Speed baseline), plus a cancel-mid-transfer probe.
4. **UART and RSPI modes, traits and typed errors.**
5. **SSI and SCUX owned drivers with lent rings, the copy API and the positional window; the USB-TX
   path moves to `deluge-bsp`.** Design the window together with phase 6's ISR binding (decision 8).
   Bench check: audio passthru, USB speaker → SCUX path.
6. **The USB iso stream (device and host) replaces the hooks and the host's static rings.** Bench
   check: UAC2 device in and out, Digitakt host streaming, the Overbridge soak.
7. **`rza1-rt` macros, boot header and fault hook moved out.** Bench check: cold boot through the
   app-loader, fault blink.
8. **Docs and lint sweep.** `missing_docs`, `cargo doc` clean, README per crate, CHANGELOG entry.
9. **DelugeFirmware#next migration.** A follow-up DF branch against the finished API, built
   through `local-sdk.toml`, with the C ABI's ring-by-offset fill on the positional window.

Each phase is done when:
- `tools/test.sh`, `rza1/tools/test.sh`, `tools/check-layering.sh` and
  `tools/link-layout.sh check` are green;
- every firmware, hw-test and example builds;
- the bench check passed, where the phase has one.

## 6. Left to the builder

- Type and module names (`IsoStream`, the SCUX block split, pin-trait macro shape).
- Whether phases 4 and 5 swap, and how each phase splits into tasks.
- How `deluge-bsp` exposes the moved pieces. Its public API may change freely, but the
  `deluge` facade's public API shouldn't change unless a plan says why.
- Exact bench-probe design (e.g. how to cancel an SD transfer mid-flight on hardware).
