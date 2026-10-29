# deluge-bsp

Board Support Package for the Synthstrom Deluge controller, targeting the
Renesas RZ/A1L (Arm Cortex-A9, 400 MHz, 3 MB on-chip SRAM + 64 MB SDRAM).

This crate sits on top of [`rza1-hal`](../rza1-hal) and provides
ready-to-use drivers and initialization routines for every peripheral on the
Deluge board.  It is `no_std` when compiled for the bare-metal target
(`armv7a-none-eabihf`), and can also be compiled for the host for unit
testing.

## Modules

| Module | Description |
|---|---|
| [`system`] | `BOARD_CONFIG`, the chip configuration `rza1_hal::init` takes (crystal, `CPG`, `MODULE_CLOCKS`, DMAC and time-driver priorities), and the DMA channel map |
| [`sdram`] | 64 MB SDRAM (Micron MT48LC16M16A2P-75) — pin-mux, BSC registers, JEDEC init |
| [`audio`] | SSI0 stereo codec interface, its rings ([`audio::with_tx`], [`audio::with_rx`]); codec power sequencing and master-clock enable |
| [`scux_dvu_path`] | Controls for the SCUX DVU path ([`audio::init_with_scux`]): volume, fades, the SRC's engine rate |
| [`scux_src_path`] | SCUX channel 1: async SRC between two rings (FFD1 → SRC1 → FFU1) |
| [`scux_usb_tx_path`] | SCUX channel 2: async SRC between two rings (FFD2 → SRC2 → FFU2), built here from the HAL's SCUX pieces |
| [`cv_gate`] | MAX5136 quad 16-bit SPI CV DAC (2 channels, ~6552 counts/V; frames from the [`max5136`](../../drivers/max5136) crate) + 4 V-trig gate GPIOs |
| [`midi_gate`] | MTU2 one-shot timer for sub-millisecond precise gate-off scheduling (~1.92 µs resolution); `setup(board.gate_timer)` |
| [`oled`] | SSD1309 128×48 OLED — `FrameBuffer`, pixel API, DMA frame send via RSPI0; CS/RST handshaked through the PIC co-processor |
| [`pic`] | PIC32 co-processor — 144-pad matrix, 36 buttons, 6 encoders, 36 LEDs, 7-segment display, gold knob indicators; dual-baud UART handshake (31 250 → 200 000 bps) |
| [`controls`] | Stable human-readable IDs for buttons, encoder shaft clicks, rotation indices, and gold-knob indicator bars |
| [`encoder`] | 6× rotary encoder IRQ accumulators (`ENCODER_DELTAS`) and detent extraction; wakes the firmware encoder task via `ENCODER_WAKER` |
| [`pads`] | Lock-free shared pad state — 144 pads packed into 5 × `AtomicU32`; `pad_get` / `pad_toggle` for ISR-safe access |
| [`uart`] | SCIF0 MIDI DIN + SCIF1 PIC UART: `init_midi`/`init_pic` from the `Board`'s singletons; DMA receive rings, PIC transmit by DMA |
| [`sd`] | SDHI SD v2 card — full JEDEC init, block read/write, SDHC/SDXC auto-detect, DMA bounce buffer |
| [`fat`] | `embedded-sdmmc` wrapper — `DelugeVolumeManager`, `DelugeBlockDevice`, FAT filesystem access |
| [`usb`] | The Deluge's USB identities (`ids`), (feature `usb-msc`) the SD card as a Mass Storage medium (`sd_block`), and (feature `usb-host`) the host-role supervisor; the classes themselves are in the [`usb-classes`](../../usb/usb-classes) crate |
| [`bus`] | RSPI0 shared by the OLED and the CV DAC: `init(shared_spi, oled_dma)`, then `lock_rspi0`, `enter_8bit` / `enter_32bit` |
| [`audio_block`] | Block-oriented audio tap over the SSI0 DMA rings, used by the SDK's `dlg.audio()` |
| [`jacks`] | Audio jack-detect inputs and the speaker-amp enable |
| [`trigger_clock`] | Analog trigger-clock input |
| [`rgb`] | RGB pad-LED surface for the 18 × 8 grid |
| [`battery`] | Battery-voltage sense via the ADC |
| [`board`] | `Board::new(rza1_hal::init(system::BOARD_CONFIG))`: the rest of the bring-up, then the Deluge's peripherals by role (USB0/1, RSPI0, SCIF0/1, MTU2 2/3, SDHI1 `sd_card`, SSIF0 `audio_ssi`, the 16 DMA channels) |
| [`flash`] | SPI-NOR chip profile and board flash map (feature `flash`) |
| [`host_panel`] | Host builds: the hook a simulated front panel plugs into (`deluge-sdk`'s simulator backend installs one) |

## Peripheral sharing

RSPI0 is shared between the OLED DMA path (`oled`) and the CV DAC
(`cv_gate`). Both go through the async mutex in [`bus`]: a consumer
`lock_rspi0().await`s a guard, drives the bus through it, and releases it on
drop. The guard tracks the current frame mode, so switching between the OLED's
8-bit and the DAC's 32-bit frames only reconfigures the peripheral when needed.

## USB

The chip-level RUSB1 device and host drivers (`init_device_mode` /
`init_host_mode`, the `embassy-usb-driver` implementations) live in
[`rza1_hal::usb`](../rza1-hal/README.md), and the board-agnostic classes (UAC2,
USB-MIDI, MSC + Bulk-Only Transport, host MIDI/UAC) in
[`usb-classes`](../../usb/usb-classes). This crate adds only the Deluge's part:
its USB identities, the SD-backed `SdBlock`, and the host-role supervisor.

## Feature flags

| Feature | Effect |
|---|---|
| `fat` | The `embedded-sdmmc` FAT stack (`fat` module and the SD block-device adapters) |
| `flash` | The `flash` module (only the app-loader writes flash) |
| `audio-irq` | Clock `audio_block` from the RX ring's per-block interrupt (`wait_block`) instead of a poll loop, with a shorter write-ahead |
| `embedded-graphics` | `DrawTarget` impl for `oled::FrameBuffer` |
| `usb-host` | The USB host-role supervisor (turns on `usb-classes`' `host-midi` and `host-uac`) |
| `usb-msc` | `usb::sd_block`, the SD card as a USB Mass Storage medium (turns on `usb-classes`' `device-msc`) |
| `host-panel` | Host builds: `host_panel::install`, so a simulator can stand in for the front panel (`deluge-sdk`'s `sim` turns it on) |
| `memory-x` (default) | Put the Deluge's `memory.x` on the linker search path; turn off to supply your own |
| `rtt` | SEGGER RTT: the RTT memory layout and fault dumps to RTT channel 0 (turns on `rza1-rt/rtt`) |
| `fault-blink` (default) | The fault handler: a CPU fault blinks the SYNC LED (P6_7), 1 to 5 pulses by kind; turn off to define your own `rza1_cpu_fault_report` (an SDK app turns `deluge`'s defaults off instead) |

The crate's `build.rs` puts the Deluge's memory layout (`memory.x`, or `memory_rtt.x` with `rtt`) on the linker search path for `rza1-rt`'s `link.x`.

Modules that need the bare-metal target (Embassy tasks, DMA, MTU2) are gated on
`#[cfg(target_os = "none")]`; the pure-logic modules also build on the host for
unit testing.

## Usage

```toml
[dependencies]
deluge-bsp = { path = "../deluge-bsp" }
```

A typical boot sequence (`Board::new` also brings up the MMU, caches and the SDRAM controller):

```rust
let board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));
deluge_bsp::uart::init_pic(board.pic_uart, board.pins.pic, board.dma.pic_rx, board.dma.pic_tx, 31_250);
deluge_bsp::bus::init(board.shared_spi, board.pins.spi, board.dma.oled);
deluge_bsp::sd::init_hw(board.sd_card, board.pins.sd, board.dma.sd_tx, board.dma.sd_rx);
deluge_bsp::audio::init(board.audio_ssi, board.pins.audio, board.dma.ssi_tx, board.dma.ssi_rx);
deluge_bsp::cv_gate::init(board.pins.cv_gate);
```

Apps built on the `deluge` SDK get all of this from `#[deluge::app]` and don't
call it directly.
