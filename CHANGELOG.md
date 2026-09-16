# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

### Changed

- Repository layout: the Wren subsystem now lives under `wren/`, the Linux
  backend crates under `linux/`, the app-loader under `firmwares/`, hardware
  probes under `hw-tests/`, the desktop simulator under `crates/`, and examples
  under `examples/baremetal/` and `examples/linux/`. Package names are unchanged.

[Unreleased]: https://github.com/FirestormAudio/deluge-sdk
