# msc-firmware

Turns the [Synthstrom Deluge] into a **USB Mass Storage** SD-card reader/writer.
When connected, the device enumerates as a USB MSC (Bulk-Only Transport, SCSI
transparent) device exposing the inserted SD card as a raw block device, so the
host OS mounts the card's FAT volume directly.

| | |
|---|---|
| Package | `msc-firmware` |
| Binary | `msc-firmware` |
| Target | `armv7a-none-eabihf` (RZ/A1L, Cortex-A9) |

## Runtime role

- Initialises the platform, heaps, PIC/OLED transport, and the task executor.
- Brings USB0 up in **device mode** with a single MSC interface.
- Runs the Bulk-Only Transport / SCSI command loop, bridging USB bulk endpoints
  to [`deluge_bsp::sd`] block read/write.
- Renders a live throughput display (TX/RX MB/s and cumulative MB) to the OLED.

## Concurrency note

While acting as USB mass storage the **host owns the filesystem**. This firmware
performs raw block passthrough only — it never mounts or writes FAT itself —
avoiding cache-coherency corruption on the RZ/A1L.

## Tasks

`src/tasks/` — `usb`, `msc`, `pic`, `oled`, `blink`. See the module docs in
[`src/main.rs`](src/main.rs).

## Building

Run from the workspace root:

| Command | Output |
|---------|--------|
| `cargo build-msc` | `target/armv7a-none-eabihf/debug/msc-firmware` (debug ELF) |
| `cargo build-msc-bin` | `target/armv7a-none-eabihf/release/msc-firmware.bin` (raw flashing binary) |

See the [workspace README](../../README.md) for flashing and debugging.

[Synthstrom Deluge]: https://synthstrom.com/product/deluge/
