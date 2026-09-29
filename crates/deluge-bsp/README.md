# deluge-bsp

Board Support Package for the Synthstrom Deluge controller, targeting the
Renesas RZ/A1L (Arm Cortex-A9, 400 MHz, 3 MB on-chip SRAM + 64 MB SDRAM).

This crate sits on top of [`rza1l-hal`](../rza1l-hal) and provides
ready-to-use drivers and initialization routines for every peripheral on the
Deluge board.  It is `no_std` when compiled for the bare-metal target
(`armv7a-none-eabihf`), and can also be compiled for the host for unit
testing.

## Modules

| Module | Description |
|---|---|
| [`system`] | Clock gating (CPG / `StbConfig`), DMA channel map, single `init_clocks()` boot entry point |
| [`sdram`] | 64 MB SDRAM (Micron MT48LC16M16A2P-75) — pin-mux, BSC registers, JEDEC init |
| [`audio`] | SSI0 stereo codec interface; codec power sequencing and master-clock enable |
| [`scux_dvu_path`] | SCUX DVU path — CPU SRAM → DMA → SCUX → SSIF0 TX (2048-frame buffer, software volume / fade) |
| [`scux_src_path`] | SCUX async SRC path — asynchronous sample-rate conversion between two independent clock domains (e.g. 44.1 kHz ↔ 48 kHz) |
| [`cv_gate`] | MAX5136 quad 16-bit SPI CV DAC (2 channels, ~6552 counts/V) + 4 V-trig gate GPIOs |
| [`midi_gate`] | MTU2 one-shot timer for sub-millisecond precise gate-off scheduling (~1.92 µs resolution) |
| [`oled`] | SSD1309 128×48 OLED — `FrameBuffer`, pixel API, DMA frame send via RSPI0; CS/RST handshaked through the PIC co-processor |
| [`pic`] | PIC32 co-processor — 144-pad matrix, 36 buttons, 6 encoders, 36 LEDs, 7-segment display, gold knob indicators; dual-baud UART handshake (31 250 → 200 000 bps) |
| [`controls`] | Stable human-readable IDs for buttons, encoder shaft clicks, rotation indices, and gold-knob indicator bars |
| [`encoder`] | 6× rotary encoder IRQ accumulators (`ENCODER_DELTAS`) and detent extraction; wakes the firmware encoder task via `ENCODER_WAKER` |
| [`pads`] | Lock-free shared pad state — 144 pads packed into 5 × `AtomicU32`; `pad_get` / `pad_toggle` for ISR-safe access |
| [`uart`] | SCIF0 MIDI DIN (31 250 bps) + SCIF1 PIC UART; DMA-backed RX/TX |
| [`sd`] | SDHI SD v2 card — full JEDEC init, block read/write, SDHC/SDXC auto-detect, DMA bounce buffer |
| [`fat`] | `embedded-sdmmc` wrapper — `DelugeVolumeManager`, `DelugeBlockDevice`, FAT filesystem access |
| [`usb`] | USB class layer over the RUSB1 drivers in `rza1l-hal`: UAC2, USB-MIDI and MSC device classes, the MSC Bulk-Only Transport, and (feature `usb-host`) host-side class drivers |
| [`bus`] | RSPI0 ownership guard shared by the OLED and the CV DAC (`lock_rspi0`, `enter_8bit` / `enter_32bit`) |
| [`audio_block`] | Block-oriented audio tap over the SSI0 DMA rings, used by the SDK's `dlg.audio()` |
| [`jacks`] | Audio jack-detect inputs and the speaker-amp enable |
| [`trigger_clock`] | Analog trigger-clock input |
| [`rgb`] | RGB pad-LED surface for the 18 × 8 grid |
| [`battery`] | Battery-voltage sense via the ADC |
| [`flash`] | SPI-NOR chip profile and board flash map (feature `flash`) |
| [`sim`] | Desktop-simulator panel standing in for the peripherals on host builds (feature `sim-link`) |

## Peripheral sharing

RSPI0 is shared between the OLED DMA path (`oled`) and the CV DAC
(`cv_gate`). Both go through the async mutex in [`bus`]: a consumer
`lock_rspi0().await`s a guard, drives the bus through it, and releases it on
drop. The guard tracks the current frame mode, so switching between the OLED's
8-bit and the DAC's 32-bit frames only reconfigures the peripheral when needed.

## USB

The chip-level RUSB1 device and host drivers (`init_device_mode` /
`init_host_mode`, the `embassy-usb-driver` implementations) live in
[`rza1l_hal::usb`](../rza1l-hal/README.md); this crate adds the board's USB
classes on top.

## Feature flags

| Feature | Effect |
|---|---|
| `fat` | The `embedded-sdmmc` FAT stack (`fat` module and the SD block-device adapters) |
| `flash` | The `flash` module (only the app-loader writes flash) |
| `audio-irq` | Clock `audio_block` from a per-block RX-DMA interrupt instead of a poll loop |
| `embedded-graphics` | `DrawTarget` impl for `oled::FrameBuffer` |
| `usb-host` | USB host-side class drivers (`embassy-usb-host`) |
| `sim-link` | Route PIC commands and OLED frames to the desktop simulator (host builds only) |

Modules that need the bare-metal target (Embassy tasks, DMA, MTU2) are gated on
`#[cfg(target_os = "none")]`; the pure-logic modules also build on the host for
unit testing.

## Usage

```toml
[dependencies]
deluge-bsp = { path = "../deluge-bsp" }
```

A typical boot sequence (`init_clocks` also brings up the SDRAM controller):

```rust
unsafe {
    deluge_bsp::system::init_clocks();
    deluge_bsp::audio::init();
    deluge_bsp::uart::init_pic(31_250);
    deluge_bsp::cv_gate::init();
}
```

Apps built on the `deluge` SDK get all of this from `#[deluge::app]` and don't
call it directly.
