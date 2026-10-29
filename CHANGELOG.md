# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `rza1-hal` DMA buffers and cancel-safe SD (phase 3 of the pre-0.1 API pass):
  - `dma_buf::{Uncached, UncachedMut}` wrap a byte slice that lies wholly in a normal
    non-cacheable section of the HAL's MMU table (the `+0x4000_0000` mirrors;
    `memmap::is_uncached_range`), so a DMA driver can't be handed cached memory. No per-transfer
    cache maintenance.
  - `Sdhi::read_blocks_dma`/`write_blocks_dma` take those types, and `read_blocks_sw`/
    `write_blocks_sw` take slices; the block count is the buffer's length. None is `unsafe` now.
    A length that isn't whole 512-byte blocks is the new `SdhiError::BufferLength`; `SdhiError`
    is `#[non_exhaustive]`.
  - A dropped SD command or transfer halts its DMA channel, stops the transfer (SD_STOP.STP) and
    resets the controller to identification mode before the drop returns, and
    `Sdhi::take_aborted` reports it. `deluge-bsp`'s SD layer then returns `NotInitialized` until
    `sd::init` re-identifies the card.
  - `Dma` itself has no `Drop`: the SSI, SCUX and UART rings keep running after their handle is
    gone, so the stop-on-drop lives on each transfer (as embassy-stm32's `Transfer`).
- `rza1-hal` GPIO is owned, embassy-style (phase 2 of the pre-0.1 API pass):
  - Every RZ/A1L pin is a `Peripherals` singleton (`P0_0` … `P9_5`; P8/P9 only on the 208/233-pin
    packages). `gpio::{Input, Output, Flex}` take one; they implement the embedded-hal 1.0
    digital traits, and dropping one changes no register.
  - An output's level goes through PSRn and toggles through PNOTn; mode, direction and the rest
    are read-modify-written in a critical section (PMCSR1/PMSR1 have no bits for P1_8–P1_15), so
    a write never undoes another pin's. An output's level is written before it becomes an output.
    Only `OutputCapable` pins can be an `Output` (not P0_x or P1_8–P1_15).
  - Pin-function traits (`uart::{TxPin, RxPin}`, `rspi::{SckPin, MosiPin, MisoPin, SslPin}`,
    `sdhi::{ClkPin, CmdPin, D0Pin..D3Pin, CdPin, WpPin}`) are generated from the manual's port
    tables by `rza1/tools/gen-pins.py` (`--check` in `rza1/tools/test.sh`). `Uart::new`,
    `Rspi::new_txonly`/`Rspi::new` and `Sdhi::new` take their pins and set the functions
    themselves. `Flex::set_as_alternate` covers functions without a driver yet.
  - `gpio::read_port(Port)` is safe (reading changes nothing). The raw `set_as_output`,
    `set_as_input`, `enable_input_buffer`, `read_pin`, `write` and `set_pin_mux`, and the
    const-generic `Pin<PORT, BIT, MODE>`, are gone.
- `deluge-bsp`: `Board::pins` (`BoardPins`) hands the pins out by role; `uart::init_*`,
  `bus::init`, `sd::init_hw`, `audio::init*`, `jacks::init`, `cv_gate::init`,
  `encoder::irq_init` and `trigger_clock::irq_init` take them. `cv_gate::init`,
  `cv_gate::gate_set`, `jacks::init` and `jacks::set_speaker_enable` are safe now. The facade's
  `deluge::parts` carries the pin groups.
- **`rza1l-hal` is now `rza1-hal`**, with chip features `rza1l` and `rza1h`: enable exactly one.
  `rza1h` compiles and adds RSPI3/RSPI4 and the 587-entry interrupt table; it is not
  hardware-validated, and SDHI is `rza1l`-only (the A1H PAC has no SDHI block).
- `rza1-hal` features: `log` and `defmt` (optional, at most one) replace the hard `log`
  dependency, and the HAL no longer forces `release_max_level_info` downstream; `unstable-pac`
  gates `rza1_hal::pac`; `rt` gates the `gic_dispatch` export; `critical-section-impl` (default)
  provides the critical-section implementation; `unstable-driver-steal` gates the now
  `#[doc(hidden)]` `Dma::steal`, `Rspi::steal` and `UartRx::steal`.
- `rza1_hal::Config` is typed and `#[non_exhaustive]`: `Config::new(clocks)` with `const`
  builders for `CpgConfig` (FRQCR), `ModuleClocks` (named STBCR2–12 module clocks, reset state by
  default), and the DMAC and time-driver interrupt priorities. It has no `Default`: the RZ/A1 has
  no internal oscillator, so there is no safe crystal to assume. `stb::StbConfig` is gone.
- Interrupts: `interrupt::Priority` (`P0`–`P30`), and `Interrupt` gains `enable`, `disable`,
  `is_enabled`, `is_pending`, `pend` (SGIs through GICD_SGIR), `unpend`, `get_priority` and
  `set_priority`. The GIC now starts every interrupt at `Priority::DEFAULT` (P16) instead of 31,
  so an enabled interrupt fires even if nobody set its priority. `gic::enable`, `gic::disable`
  and `gic::set_priority` are crate-private.
- Drivers no longer set interrupt priorities (UART, SDHI, USB, MTU2; `mtu2::OneShot::new` loses
  its `priority` parameter); DMAC channels take `Config::dma_interrupt_priority`. `deluge-bsp`
  sets the Deluge's priorities explicitly in `Board::new`, to the values the drivers used.
- `rza1_hal::init` starts the OSTM time driver itself (`time-driver-ostm`), at
  `Config::time_interrupt_priority`; `time_driver::init` and its handler types are gone, and
  OSTM0/OSTM1 and their interrupts are not in `Peripherals` while the driver owns them.
- DelugeFirmware#next does not build against this until its migration (phase 9 of the
  pre-0.1 API pass).

### Removed

- The Wren scripting subsystem (`wren-sys`, `deluge-wren-core`, `wren-firmware`, the web
  editor/simulator and debugger) moved to its own repo, deluge-wren; it consumes this SDK
  through the `deluge` facade.

### Added

- `hw-tests/overbridge-device-firmware`: runtime switching between USB device faces. One task
  owns USB0 and presents either the Overbridge class or a UAC2 device, disconnecting
  (`rza1l_hal::usb::disconnect`) and re-initialising device mode between them; the
  `face-switch` feature alternates every 30 s. It is the pattern for a firmware that offers both.
- `rza1l-hal` USB host: `Rusb1HostDriver::enable_rx_dma` gives the driver a DMAC channel. The
  first interrupt-on-bulk IN pipe opened afterwards (an Overbridge audio stream) is drained by
  DMA through D1FIFO instead of byte-wise CPU reads. Without it, transfers use the CPU as before.
  `deluge_bsp::board::Usb0RxDma` names the channel for it (DMA_CH1).
- `deluge-bsp` `usb::host`: `HostAudioMode` (Auto / Uac / Overbridge / Off), set at runtime with
  `set_host_audio_mode`; changing it re-binds an attached device. `host_audio()` returns
  whichever audio driver is bound, and `host_audio_connection()` counts binds.
- Elektron Overbridge 2 device class (`usb-classes` feature `device-overbridge`,
  `overbridge::device`). It has the Digitakt's interface layout and name requests; the board
  supplies the identity and channel counts. The RZ/A1L device driver carries interrupt
  endpoints over 64 bytes on bulk pipes.
- Elektron Overbridge 2 host support (`usb-classes` feature `host-overbridge`; `deluge-bsp`
  feature `usb-host-overbridge`). A Digitakt, Digitone, Syntakt, Analog Four/Rytm/Heat MKII,
  Digitakt II or Digitone II in Overbridge mode streams every track to the Deluge as 48 kHz
  stems, read through `deluge_bsp::usb::host::overbridge()`, with a stream back to the
  device. Elektron devices are offered to Overbridge before UAC. On the RZ/A1L the stream back
  is one 64-byte block (header + 4 frames, `OverbridgeHost::limit_out_packet`) per block
  received, so audio to the device runs short (4 of every 7 frames); the device's 88-byte
  block doesn't fit an interrupt pipe, and split in two it locked a Digitakt up.
  `OverbridgeHost::split` gives the receive and send halves to run concurrently.
- `usb-classes` `AudioClass`: up to 48 capture channels. Above 1024 bytes the capture endpoint
  is sized per high-speed microframe (7 frames), so `AudioClass<8>` (168-byte packets) builds,
  for example for an Elektron Outbox 8. `AudioClass::CAPTURE_MAX_PACKET` gives the size.
- `rza1l-hal` USB host: an interrupt IN endpoint over 64 bytes (such as Overbridge's 368-byte
  audio blocks) opens on a bulk pipe (MXPS 512 on pipes 3-5, or its own size up to 1024 on
  pipes 1-2) instead of failing for lack of a large interrupt pipe. An interrupt OUT endpoint
  over 64 bytes opens on an interrupt pipe (6-9) at MXPS 64: a high-speed bulk OUT pipe always
  starts with PING (TRM §28.4.7), which a Digitakt's interrupt endpoint never answers, and an
  isochronous pipe can only send DATA0 (TRM §28.4.3). Its senders must keep each packet to 64
  bytes.
- `deluge_bsp::cpu_load`: a rough CPU-load meter for hardware tests (an executor spin count,
  logged once a second against the idlest second seen). It keeps the executor from sleeping,
  so it is for the bench only.
- Initial public release of the Deluge SDK: write apps for the Synthstrom
  Deluge in async Rust via the `#[deluge::app]` attribute and the `Deluge`
  capability handle.
- `cargo deluge` host subcommand (`new` / `build` / `run` / `deploy` / `log` /
  `debug` / `trace`).
- The on-device app-loader (second-stage bootloader) with USB dev-mode upload
  and SD-card `/APPS/` loading.
- Example apps under `examples/` covering OLED, input, pads, LEDs, audio, CV/gate,
  MIDI, clock I/O, SD card, and USB logging.
- SD (RZ/A1L): High-Speed mode support — cards that accept the CMD6 switch now
  run at 33.3 MHz SD_CLK (P1/2) instead of 16.7 MHz, ~2× sequential throughput;
  automatic fallback to 16.7 MHz for cards without CMD6.
- `deluge::oled::{FrameBuffer, draw_str}` for apps that render into their own
  frame buffer, and the `usb-serial` feature's `Deluge::usb_serial` — take USB0
  as a CDC-ACM port under your own VID/PID, no `unsafe`.
- `deluge_bsp::usb::classes::midi` packet mode: `use_packets()` plus
  `try_recv_packet_from_host` / `try_send_packet_to_host` / `tx_packet_free` /
  `tx_packet_pending` pass USB-MIDI 1.0 event packets through unchanged — SysEx
  and cable numbers included — for consumers that speak packets.

### Changed

- `rza1l-hal` USB host: every bulk OUT endpoint on a port shares one pipe, re-pointed
  per transfer with each endpoint's data toggle kept. A hub now carries 4 USB-MIDI devices that
  send and receive (it was 2). The shared pipe prefers the bulk-only pipes 3–5, keeping 1–2
  free for isochronous audio where it can.
- `deluge-bsp`: `usb-host` is split into `usb-host-core` (supervisor, hubs, MIDI),
  `usb-host-uac` and `usb-host-overbridge`. `usb-host` still means `usb-host-uac` for this
  release; it will become `usb-host-core`, so name `usb-host-uac` if you want UAC.
- The RZ/A1 chip crates (`rza1-pac`, `rza1-rt`, `rza1l-hal`, `rza1-alloc`) moved from `crates/` to
  `rza1/`, their own Cargo workspace with its own tooling (`rza1/tools/test.sh`) and CI job. A
  path dependency on them changes from `crates/<crate>` to `rza1/<crate>`; git dependencies are
  unaffected.
- Phase 5 is complete: every `rza1l-hal` module takes its registers from `rza1-pac`
  (`tools/check-register-tables.sh` checks the whole crate). DMAC, SSI, SCUX, SDHI, SPIBSC and USB
  moved in 5b; SCUX, SPIBSC and USB used raw volatile pointers and now go through the MMIO seam, so
  their register sequences are host-testable. The PAC gains DMAC channel blocks, SCUX sub-unit
  blocks, USB arrays and FIFO byte views, and the RZ/A1L's SDHI (a block from its manual's register
  list; the vendor SVD lacks it). **Breaking:** `usb::regs::{Rusb1Regs, rd, wr, rmw, rd32, wr32,
  pipectr_ptr, devadd_ptr}` and `usb::{USB0_BASE, USB1_BASE, base}` are gone: use
  `rza1l_hal::pac::USB200` / `USB201` (or `usb::regs::usb(port)` and `usb::regs::pipectr`).
  `usb::fifo::FifoPort`'s fields are PAC register handles (plus a `lanes` byte-view array), and its
  `cfifo`/`d0fifo`/`d1fifo` constructors and `fifo_for_pipe` take a `rza1l_hal::pac::usb::Usb` and
  are safe; the `usb::pipe::pipe_*` functions take that `Usb` instead of `*mut Rusb1Regs`.
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
  RZ/A1L's 538, not the RZ/A1H's 587. The docs now give the Deluge's chip as the RZ/A1L R7S721020
  (they gave an RZ/A1H part number).
- Phase 4 is complete: every `rza1l-hal` driver is built from its peripheral singleton and a typed
  interrupt binding. **Breaking:** `sdhi::Sdhi::new(Peri<SDHIn>, tx, rx, Irqs, sd_option)` replaces
  the port-numbered SDHI functions; `ssi::init*` take `Peri<SSIF0>` and typed DMA channels
  (`SsiConfig` is gone); the SCUX FIFO and path inits take `dmac::Dma`; `usb::init_device_mode` /
  `init_host_mode` / `disconnect` take the port singleton and a `DeviceInterruptHandler`,
  `HostInterruptHandler` or `DualRoleInterruptHandler` binding; `dmac::raw` and `gic::register` are
  no longer public (bind handlers with `bind_interrupts!`; drivers hold typed `Dma` handles). New:
  `Dma::new_without_interrupt` for polled rings, `dmac::BlockDma` + `BlockInterruptHandler` for
  per-block rings, typelevel `IRQ0`–`IRQ7` and SDHI interrupts. `usb::Rusb1Driver<'d>` and
  `Rusb1HostDriver<'d>` borrow their port, so a firmware type is `Rusb1Driver<'static>`.
- `deluge_bsp::Board` also hands out SDHI1 (`sd_card`) and SSIF0 (`audio_ssi`). **Breaking:**
  `sd::init_hw(card, tx, rx)` brings the SD slot up once (`sd::init` now runs only the card
  protocol); `audio::init(ssi, tx, rx)` and `audio::init_with_scux(ssi, rx, ffd0)` take their
  parts; `system::SSI_CONFIG` is `system::SSICR`. `deluge::parts` gains the SD, audio and (when the
  SDK doesn't own it) USB0 peripherals.
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
- `rza1l-hal` has Embassy-style ownership. `rza1l_hal::init(Config) -> Peripherals` takes the
  peripheral singletons once and runs the chip bring-up (CPG, module clocks, GIC);
  `rza1l_hal::bind_interrupts!` binds typed interrupt handlers; `dmac::Dma::new(Peri<DMA_CHn>, Irqs)`
  owns a DMA channel. **Breaking:** the channel-number DMA functions moved to the transitional
  `rza1l_hal::dmac::raw`, and `deluge_bsp::system::init_clocks()` is replaced by
  `deluge_bsp::Board::new(rza1l_hal::init(deluge_bsp::system::BOARD_CONFIG))`, which returns
  the peripherals by role.
- **Breaking:** enabling both `usb-log` and `usb-serial` is a compile error (both drive USB0)
  instead of a panic at `Deluge::usb_serial()`.
- **Breaking:** the `deluge` facade no longer re-exports `deluge_bsp`; depend on `deluge-bsp`
  directly to use it.
- The runtime moved out of `rza1l-hal` into the new `rza1-rt` crate: the reset vector and
  bootloader metadata, the reset, fault and IRQ entry handlers, and the linker script, now
  `link.x`. **Breaking:** binaries link with `-Tlink.x` (`cargo:rustc-link-arg-bins=-Tlink.x`
  in `build.rs`) instead of `-Trza1l.x`/`-Trza1l_rtt.x`; the `rtt` and `app-fault-reporter`
  features moved from `rza1l-hal` to `rza1-rt` and are reached as `deluge-bsp/rtt` and
  `deluge-bsp/app-fault-reporter`.
- `deluge-bsp` provides the Deluge's memory layout: its `build.rs` puts `memory.x` (the RTT form
  with the `rtt` feature) on the linker search path. **Breaking:** delete your app's `memory.x`
  and `memory_rtt.x` and reduce its `build.rs` to the `-Tlink.x` line (`cargo deluge new` now
  scaffolds that). The stack sizes are `PROVIDE`d: override one with
  `cargo:rustc-link-arg-bins=--defsym=PROGRAM_STACK_SIZE=<bytes>`, printed *before* the
  `-Tlink.x` line (lld applies `--defsym` in command-line order). To bring your own layout
  instead, turn off `deluge-bsp`'s default `memory-x` feature and put your `memory.x` on the
  search path. If you keep the BSP's layout and your app's build directory is on the linker
  search path (a `cc`-built C/C++ library puts it there), run `cargo clean -p <app>` once so an
  old `memory.x` your previous `build.rs` copied there can't shadow it.
- The boot and fault blink use the status LED the board's `memory.x` names
  (`RZA1_STATUS_LED_PORT`/`_PIN`, P6_7 on the Deluge) instead of a hard-coded pin.
  **Breaking:** a custom `memory.x` must define both (`RZA1_STATUS_LED_PORT = 6;`
  `RZA1_STATUS_LED_PIN = 7;` for the Deluge), or the link fails.
- `rza1l_hal::time_driver` is behind the `time-driver-ostm` feature, on by default.
- `deluge-alloc` is renamed `rza1-alloc`. Apps using `deluge::deluge_alloc` are unaffected.
- `deluge-bsp` holds only Deluge wiring now:
  - USB device classes (UAC2, USB-MIDI, MSC), the Bulk-Only Transport engine, the USB-MIDI
    packet queues and the USB host MIDI/UAC drivers moved to the new board-agnostic
    `usb-classes` crate (one feature per class). **Breaking:** import them from `usb_classes::…`.
    The SD-backed `SdBlock` is now `deluge_bsp::usb::sd_block::SdBlock`. `bot::run`/`run_until`
    are generic over any `embassy_usb_driver` endpoints. `Uac::try_register` takes the sample
    rate to negotiate, and `usb_classes::host::uac::shared` is public. Firmware that uses a
    class now depends on `usb-classes` itself, with that class's feature (`device-audio`,
    `device-midi`, `device-msc`, `host-midi`, `host-uac`). `SdBlock` needs the new
    `deluge-bsp/usb-msc` feature.
  - The MAX5136 CV DAC protocol is the new `max5136` embedded-hal driver crate.
    `deluge_bsp::cv_gate::cv_set` (and so `deluge::Cv::set`) now ignores, with a warning, a
    channel the Deluge doesn't wire (anything but 0–1) instead of sending the DAC a wrong command.
  - The SCUX DVU/SRC path recipes are in `rza1l_hal::scux::{dvu_path, src_path, usb_tx_path}`,
    taking their DMA channels; `deluge_bsp::scux_*_path` keep their old API on the Deluge's
    channels.
  - The desktop-simulator backend moved into `deluge-sdk`; the BSP exposes
    `deluge_bsp::host_panel` for it. **Breaking:** the `deluge-bsp/sim-link` feature and
    `deluge_bsp::sim` are gone. The PIC command constants the simulator uses
    (`deluge_bsp::pic::CMD_*`) are now public.
    Installing a panel needs the new `deluge-bsp/host-panel` feature, which `deluge-sdk`'s `sim`
    feature turns on.
  - The `rza1l_hal::scux` path pointer accessors (`tx_current_ptr`, `rx_current_ptr`) panic
    if called before their path's `init`, instead of reading a non-existent DMA channel.
- `rza1l-hal` is now board-agnostic. The board registers its crystal with
  `rza1l_hal::clocks::init(ClockConfig::new(extal_hz))` (the Deluge's is
  `deluge_bsp::system::CLOCKS`, which `rza1l_hal::init(BOARD_CONFIG)` registers). OSTM delays, the RSPI and SCIF baud
  dividers and the Embassy time driver derive their rates from it. **Breaking:** `ostm::OSTM_HZ`
  and `mtu2::MTU2_P0_HZ` are removed; use `rza1l_hal::clocks::get().p0_hz()` or
  `deluge_bsp::system::CLOCKS.p0_hz()`.
  `ClockConfig::new` rejects crystals outside the RZ/A1L's 10–13.33 MHz EXTAL range, its
  frequency is read with `extal_hz()`, and a second `clocks::init` with a different crystal
  panics.
- `rza1l_hal::gic::set_sdram_code_window` lets a board narrow the IRQ dispatcher's
  handler-address check to the SDRAM it actually fits (default: all of CS2/CS3). The Deluge
  sets its 64 MB on CS3 (`deluge_bsp::sdram::WINDOW`).
- **Breaking:** the `app-fault-reporter` hook is now `rza1_cpu_fault_report` (was
  `deluge_cpu_fault_report`).
- **Breaking:** `deluge` no longer re-exports `rza1l_hal`; depend on `rza1l-hal` directly.
- The Embassy time driver's µs conversion no longer overflows on long uptimes for crystals other
  than the Deluge's.
- Repository layout: the Wren subsystem now lives under `wren/`, the Linux
  backend crates under `linux/`, the app-loader under `firmwares/`, hardware
  probes under `hw-tests/`, the desktop simulator under `crates/`, and examples
  under `examples/baremetal/` and `examples/linux/`. Package names are unchanged.

### Fixed

- `rza1l-hal` USB host: a high-speed device's interrupt endpoint is polled at the period its
  bInterval asks for (2^(bInterval-1) microframes). It was polled more often: a hub's status
  endpoint at bInterval 12 every 1 ms instead of 16 ms.

[Unreleased]: https://github.com/FirestormAudio/deluge-sdk
