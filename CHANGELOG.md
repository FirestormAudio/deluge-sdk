# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, minor releases may contain
breaking changes.

## [Unreleased]

The first developer preview of the Deluge SDK.

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
  MAX5136 CV DAC, SD card (with High-Speed mode where the card supports it), SPI flash
  identification and write protection, USB device classes, and USB host with hubs.
- **USB** (`usb-classes`): UAC2 audio, USB-MIDI (including raw packet mode), mass storage
  (Bulk-Only Transport) device classes; host-side USB-MIDI and UAC2 audio; and
  Elektron Overbridge 2 host and device classes.
- **RZ/A1 chip crates** (`rza1-pac`, `rza1-rt`, `rza1-rt-macros`, `rza1-hal`, `rza1-alloc`): a
  peripheral access crate generated from the vendor SVD for the RZ/A1L and RZ/A1H, a runtime with
  `#[entry]`, `#[irq]` and `#[fault]`, an Embassy HAL, and heap allocators. They form their own
  workspace under `rza1/` with no dependency on any `deluge-*` crate.
- **DSP and UI libraries**: `deluge-fixedpoint` and `arm-dsp` (`MIT OR Apache-2.0`);
  `deluge-ui-toolkit`, `deluge-grid-toolkit` and `deluge-fonts` (`GPL-3.0-or-later`).
- **A desktop simulator** (`cargo deluge sim`) with an on-screen front panel, audio, MIDI and SD
  bridges, and headless golden-frame testing, plus a browser front panel
  (`tools/deluge-web-sim`).
- **A native Linux backend** (`linux/`) for apps that run on the Deluge's Linux userland.
- **A from-scratch boot chain, in development**: `stage0`, an execute-in-place first stage that
  validates and enters a CRC-checked boot slot, and `mkslot`, the host tool that builds slot
  images. For development units only.
- **Examples** under `examples/` covering every capability, and hardware validation firmwares
  under `hw-tests/`.

[Unreleased]: https://github.com/FirestormAudio/deluge-sdk
