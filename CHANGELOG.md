# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, minor releases may contain
breaking changes.

## [Unreleased]

### Fixed

- `usb_classes::midi`: the MIDI 2.0 alternate setting works on Linux. The Group Terminal Block
  descriptors now answer the standard `GET_DESCRIPTOR` (0x26 << 8 | alternate) the USB MIDI 2.0
  spec defines, instead of a class request no host sends, so Linux no longer gives up with
  "Failed to parse GTB"; and the block declares MIDI 2.0 (`bMIDIProtocol` 0x11), not MIDI 1.0
  with timestamps (0x02).
- The black encoders (the four with detents) start counting from the detent's resting state
  instead of the pins as read at boot. A boot read that caught contact bounce put every click
  out of step until the next reboot, and the first click after each reversal did nothing. The
  gold encoders still start at the pins as read.

### Added

- `rza1-rt`'s `zero-init-below-image` feature (passed through by `deluge-bsp`) places `.bss`
  and the MMU translation table below the vector table instead of after the image, for an app
  whose heap starts at the image's end and reclaims its boot-only tail. The default layout is
  unchanged, and `link.x` now refuses a `__sram_heap_base` that lies below the zero-init data.
- `usb_classes::midi` and `usb_classes::midi_packets` can run more than one USB-MIDI device:
  `MidiState` and `PacketQueues` hold a device's queues and flags (one `static` each per device),
  with `build_with`, the `run_*_with` endpoint tasks and `MidiState::end_session` for a session
  dropped without a bus reset. The free functions, `build` and `run_*` keep working, on
  `DEFAULT_STATE`.
- `ClockOut::start`, `set_period`, `stop` and `is_running`. `set_period` and `set_pulse_width`
  take effect at the next period boundary, so a tempo change never gives a short or doubled
  pulse. Periods are clamped to 1 ms–2 s and the pulse width to under the period; the first
  clamp of each is logged at `warn`.

### Changed

- In bare-metal builds `ClockOut`'s edges come from the MTU2 channel 4 timer, so its timing no
  longer depends on the executor; native Linux and the simulator use a software loop.
  Dropping a `ClockOut` stops the clock and leaves the gate low, and `pulse()` while the clock
  runs returns at once.
- `Deluge::clock_out(ch)` panics up front for a gate channel of 4 or more; the Deluge has
  gates 0–3.
- The `clock_jacks` example free-runs a 24 ppqn clock and switches its tempo on PLAY. Its clock
  in no longer echoes to the clock out: it only toggles the SYNC LED and shows its BPM.

## [0.1.0] - 2026-11-05

The first developer preview of the Deluge SDK. The API is not stable yet: until 1.0, a minor
release may contain breaking changes.

The RZ/A1 chip crates and `deluge-embedded-fatfs` are released as 0.2.0, and
`deluge-block-device-adapters` as 0.3.0: earlier versions briefly published under those names
had a different API and were yanked.

### Added

- **The SDK** (`deluge-sdk`, imported as `deluge`): write apps for the OLED Deluge in async Rust.
  `#[deluge::app]` brings up heaps, clocks, interrupts, the Embassy executor and a panic handler;
  the `Deluge` capability handle gives each peripheral to one owner: OLED, pads, buttons,
  encoders, LEDs, stereo audio, CV/gate, clock in/out, DIN MIDI, jack detection, the SD card
  (async FAT, streaming files with seek) and the SYNC LED. Optional features route `log` to RTT
  or a USB serial port, register the SRAM heap, drive audio from the DMA interrupt, and select
  the simulator or Linux backend.
- **`cargo deluge`**: `new`, `build`, `run` (upload over USB and launch from RAM), `deploy` (copy
  to the SD card's `/APPS/`), `log`, `sim`, `linux`, `debug` and `trace`.
- **The app-loader**: a second-stage bootloader with a boot menu, USB dev-mode upload, SD
  `/APPS/` loading, storing an app to flash, and a recovery boot. Optional bootloader write
  protection (`flash-protect`, off by default) locks the bootloader region at app hand-off.
- **Board support** (`deluge-bsp`): the PIC front panel, SSD1309 OLED, codec and SSI audio, the
  MAX5136 CV DAC (`max5136`), SD card (with High-Speed mode where the card supports it), SPI
  flash identification and write protection, the SRAM and SDRAM heaps, USB device classes, and
  USB host with hubs.
- **USB** (`deluge-usb-classes`, imported as `usb_classes`): UAC2 audio, USB-MIDI (including raw packet mode), mass storage
  (Bulk-Only Transport) device classes; host-side USB-MIDI and UAC2 audio; and
  Elektron Overbridge 2 host and device classes.
- **RZ/A1 chip crates** (`rza1-pac`, `rza1-rt`, `rza1-rt-macros`, `rza1-hal`): a peripheral
  access crate generated from the vendor SVD for the RZ/A1L and RZ/A1H, a runtime with
  `#[entry]`, `#[irq]` and `#[fault]`, and an Embassy HAL including the SD card protocol. They
  form their own workspace under `rza1/` with no dependency on any `deluge-*` crate.
- **Async FAT** (`deluge-embedded-fatfs`, `deluge-block-device-adapters`): forks of MabezDev's
  `embedded-fatfs` and `block-device-adapters` with upstream fixes applied.
- **DSP and UI libraries**: `deluge-fixedpoint` and `arm-dsp` (`MIT OR Apache-2.0`);
  `deluge-ui-toolkit`, `deluge-grid-toolkit` and `deluge-fonts` (`GPL-3.0-or-later`, in the
  repository but not published to crates.io yet).
- **A desktop simulator** (`deluge-simulator`, run by `cargo deluge sim`) with an on-screen front
  panel, audio, MIDI and SD bridges, and headless golden-frame testing, plus a browser front panel
  (`tools/deluge-web-sim`). `deluge-panel-protocol` is the front-panel wire protocol shared by
  the simulator, the controller firmware and an external brain.
- **A native Linux backend** for apps that run on the Deluge's Linux userland: `libdeluge-sys`
  and `libdeluge`, bindings to its hardware library.
- **A from-scratch boot chain, in development**: `stage0`, an execute-in-place first stage that
  validates and enters a CRC-checked boot slot, and `mkslot`, the host tool that builds slot
  images. For development units only.
- **Examples** under `examples/` covering every capability, and hardware validation firmwares
  under `hw-tests/`.

[Unreleased]: https://github.com/FirestormAudio/deluge-sdk/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/FirestormAudio/deluge-sdk/releases/tag/v0.1.0
