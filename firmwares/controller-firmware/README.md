# controller-firmware

Turns the [Synthstrom Deluge] into a **USB control surface** driven by a host
over CDC-ACM. The host owns all illumination and display; the Deluge forwards
input and renders host-supplied frames.

| | |
|---|---|
| Package | `deluge-controller` |
| Binary | `deluge-controller` |
| Target | `armv7a-none-eabihf` (RZ/A1L, Cortex-A9) |

## Runtime role

- Initialises the platform, heaps, USB, audio, and the task executor.
- Forwards BSP input streams (pads, buttons, encoders) to the host over CDC.
- Renders host-supplied frames to the OLED and host-supplied colours to the RGB
  pad matrix; applies MOD-knob master-volume control over USB audio.

## UI model

The host owns all illumination — pad LEDs, button LEDs, knob indicators, and the
OLED. Pad/button/encoder input is forwarded over CDC; the CDC task pushes host
RGB frames straight to the PIC and host framebuffers to the OLED task.

## Tasks

`src/tasks/` — `pic`, `encoder`, `jack_detect`, `midi`, `audio`, `cdc`, `usb`,
`usb_host`, `oled`, `sd`; firmware events are defined in
[`src/events.rs`](src/events.rs). See the module docs in
[`src/main.rs`](src/main.rs) for the BSP-vs-firmware task split.

## Building

Run from the workspace root:

| Command | Output |
|---------|--------|
| `cargo build-fw -p deluge-controller` | `target/armv7a-none-eabihf/debug/deluge-controller` (debug ELF) |
| `cargo build-controller-bin` | `target/armv7a-none-eabihf/release/deluge-controller.bin` (raw image — flash as the device firmware) |

## Running

A complete device firmware, RAM-linked at SRAM `0x20020000` — the same
second-stage window the app-loader uses. Either:

- **Run the ELF over a probe** — J-Link / `probe-rs` load and run it; see the
  [workspace README → Debugging](../../README.md#debugging).
- **Flash the `.bin` as the device firmware** — installed the same way as the
  app-loader, so the unit boots straight into it; see the
  [Device setup guide](../../docs/device-setup.md).

Either way, drive it from the host over CDC-ACM (the host owns illumination and
the display; the Deluge forwards input and renders host frames).

[Synthstrom Deluge]: https://synthstrom.com/product/deluge/
