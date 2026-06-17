# demo-firmware

Top-level demo firmware for the [Synthstrom Deluge] — the reference image that
exercises the full board: Embassy executor, USB device/host stack, audio, and
the complete input → event → render pipeline.

| | |
|---|---|
| Package | `demo-firmware` |
| Binary | `demo-firmware` |
| Target | `armv7a-none-eabihf` (RZ/A1L, Cortex-A9) |

## Runtime role

- Initialises the platform, heaps, USB, audio, and the task executor.
- Runs the Deluge USB **device** or **host** stack depending on `USB0_HOST_MODE`.
- Translates BSP input streams (pads, buttons, encoders) into firmware events
  and UI behaviour.
- Applies product policy: OLED rendering, RGB rendering, analyser display,
  heartbeat, and MOD-knob volume control.

## Tasks

`src/tasks/` — `pic`, `encoder`, `jack_detect`, `midi`, `audio`, `usb`,
`usb_host`, `oled`, `rgb`, `analysis`, `blink`, `sd`. The BSP owns hardware
capture and shared state; this crate layers product behaviour on top (see the
module docs in [`src/main.rs`](src/main.rs)).

## Building

Run from the workspace root (aliases in `.cargo/config.toml` supply the required
`-Zbuild-std=core` flags):

| Command | Output |
|---------|--------|
| `cargo build-fw` | `target/armv7a-none-eabihf/debug/demo-firmware` (debug ELF, RTT enabled) |
| `cargo build-fw-rel` | `target/armv7a-none-eabihf/release/demo-firmware` (release ELF, RTT disabled) |
| `cargo build-fw-bin` | `target/armv7a-none-eabihf/release/demo-firmware.bin` (raw image — flash as the device firmware) |

## Running

A complete device firmware, RAM-linked at SRAM `0x20020000` — the same
second-stage window the app-loader uses. Either:

- **Run the ELF over a probe** — J-Link / `probe-rs` load and run it; see the
  [workspace README → Debugging](../../README.md#debugging).
- **Flash the `.bin` as the device firmware** — installed the same way as the
  app-loader, so the unit boots straight into it; see the
  [Device setup guide](../../docs/device-setup.md).

[Synthstrom Deluge]: https://synthstrom.com/product/deluge/
