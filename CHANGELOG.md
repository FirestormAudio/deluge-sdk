# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, minor releases may contain
breaking changes.

## [Unreleased]

### Fixed

- `deluge-flash-protect`: a write that sets SR1 on its own re-sends SR2 with the QE bit that was
  read, so a part that clears SR2 on a one-byte `01h` keeps its QE; the read-back now checks QE
  too, so a write that loses it isn't reported as written.

## [0.2.0] - 2026-11-09

The app-loader is rebuilt on deluge-sdk, and can run from the boot slots behind stage-0 as well as
from the stock bootloader.

### Added

- The app-loader can run from a boot slot behind stage-0 as well as from the stock bootloader: one
  binary for both. In a slot it confirms the slot it booted from, and offers `UPDATE LOADER` for a
  `/BOOT/LOADER.SLT` on the card, written into the other slot. Boot-region protection (provision
  on first boot, lock-down before an app) is behind the `flash-protect` feature, off by default.
- stage-0 boots slot B too: it picks the newest valid slot, tries a pending one, and rolls back
  to the other when the watchdog ended its last boot.
- `firmwares/loader-shim`: the slot image's shim, position-independent, so one `LOADER.SLT` fits
  either slot. `tools/loader-image.sh` builds `LOADER.SLT` and a stage-0 install image, and checks
  both.
- `deluge-loader`: the app-loader's logic, host-tested: the boot and protection sequence, the
  menu and auto-boot decision, app staging and self-update.
- `rza1_hal::wdt`: the watchdog (its clock select, up to 125 ms), and `spibsc::set_fast_read`
  (the SPI flash's 0Bh fast read at a given bit rate). `deluge_bsp::flash::set_stock_read_mode`
  puts the flash's reads back on the stock bootloader's timing.
- `deluge_bsp::flash::protect::reload`, `lift_volatile` and `restore_volatile`.
- `deluge_image::app`: a header for a flat image a loader copies into RAM and
  runs (`encode_app_header`, `validate_app`), and `mkslot app`, which flattens an ELF's
  `PT_LOAD` segments under it.
- `deluge_image::slot::decode_boot_info` reads back the word `boot_info` writes (refusing
  anything else), and `plan_update` picks a self-update's slot (the one not running) and sequence.
- `deluge::Sd::list` and `Sd::open_in`: list a root-level directory, and open a file in one
  (both names single, no paths), on the device and in the simulator.

### Changed

- **Breaking:** the Deluge's boot-slot layout is `deluge_image::slot::DELUGE`.
- `deluge::Deluge::sd` can be called again once the `Sd` it returned is dropped, and after a
  mount that failed: one `Sd` at a time instead of one per boot.

## [0.1.1] - 2026-11-08

### Fixed

- The app-loader boots from the stock Deluge bootloader again. In 0.1.0 it was linked at
  `0x20020000`, where the stock bootloader runs while it copies the image into SRAM: the copy
  overwrote its own loop about 11 KB in, and the half-copied app-loader died with a data abort
  at every power-on. It now builds with deluge-bsp's `zero-init-below-image`, which starts the
  image above the bootloader (about `0x2004A000`, as before 0.1.0), and its link fails if
  `_start` is ever below `0x20040000`.

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

- `deluge-image::slot`: the boot-slot layout (`SlotLayout`; the Deluge's is a constant) and the variant
  codes a stage-0 accepts are parameters (`parse_with`, `validate_with`, `encode_header_with`); the
  Deluge's are unchanged. `select` picks the
  slot to boot from both slots and the watchdog flag: the newest valid, a pending one tried, rolled
  back to the other if the watchdog killed its last boot, or tried again when it is the only one.
  `mkslot` takes `--variant-code`, `--slot-len`, `--slot-b`, the slot offsets and `--corrupt-body-b`.
- `deluge-flash-protect`: the status-register protection core (`recipe`, `plan`, `apply`, `SrBus`)
  moved out of `deluge-bsp` into a chip-neutral crate, which `deluge_bsp::flash::protect`
  re-exports. It adds the AT25QF641B (8 MB, bottom 1 MB), `protected_range_for` any capacity, and
  `lift_volatile`/`restore_volatile`: open a protected region for one program through the
  volatile status register, leaving the non-volatile protection alone (a failed write is undone
  by a software reset).
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

- `deluge_bsp::flash::protect`'s `WriteMethod`, `SrBus`, `Goal`, `plan` and the rest come from
  the new `deluge-flash-protect` crate. `WriteMethod` is now that crate's type, not
  `rza1_hal::spibsc::WriteMethod`: an outside `SrBus` implementation converts with
  `protect::hal_method` when it calls the HAL.
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

[Unreleased]: https://github.com/FirestormAudio/deluge-sdk/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/FirestormAudio/deluge-sdk/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/FirestormAudio/deluge-sdk/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/FirestormAudio/deluge-sdk/releases/tag/v0.1.0
