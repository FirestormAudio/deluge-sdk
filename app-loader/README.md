# app-loader

Second-stage bootloader / **app loader** for the [Synthstrom Deluge] (RZ/A1L).
The Deluge first-stage bootloader loads this image from SPI flash into SRAM at
`0x20020000`; the app loader then picks and launches an application firmware
image from the SD card or on-board flash, and provides USB firmware-update and
data-transfer modes.

| | |
|---|---|
| Package | `app-loader` |
| Binary | `app-loader` |
| Target | `armv7a-none-eabihf` (RZ/A1L, Cortex-A9) |
| Loaded at | SRAM `0x20020000` (by the first-stage bootloader) |

## Boot sequence

1. Initialise the platform (MMU, caches, SDRAM, GIC, OSTM).
2. Mount the SD card's first FAT volume.
3. Enumerate ELF application images from `/APPS/` on the card.
4. Present a GRUB-style boot menu on the OLED with encoder-wheel selection and a
   `BOOT_COUNTDOWN_SECS` (5 s) auto-boot of the default entry; if only a single
   image exists it auto-launches.
5. Stream the selected ELF, load its `PT_LOAD` segments to their physical
   addresses, flush all caches, and branch to `e_entry`.

The menu also exposes two synthetic entries:

- **`UPDATE FW`** — enter USB UF2 firmware-update mode.
- **`DATA TRANSFER`** — expose the raw SD card over USB Mass Storage.

## Modules

`src/` —

| Module | Role |
|--------|------|
| [`elf`](src/elf.rs) | Minimal streaming ELF32-LE loader for ARM firmware images |
| [`file_browser`](src/file_browser.rs) | Enumerates loadable ELF images from `/APPS` on the SD card |
| [`flashboot`](src/flashboot.rs) | Launches a firmware image stored in SPI flash (in addition to SD `/APPS/`) |
| [`launcher`](src/launcher.rs) | Cache flush + branch-to-application handoff |
| [`ui`](src/ui.rs) | OLED + encoder file-selector / boot menu (128×48) |
| [`ghostfat`](src/ghostfat.rs) | Synthesized FAT16 volume backing the UF2 update drive |
| [`uf2`](src/uf2.rs) | UF2 block parsing + SPI-flash programming |
| [`usbmsc`](src/usbmsc.rs) | USB MSC modes — UF2 update (ghostfat) and raw-SD data transfer |

## Building

Run from the workspace root (aliases in `.cargo/config.toml` supply the required
`-Zbuild-std=core` flags):

| Command | Output |
|---------|--------|
| `cargo build-app-loader` | `target/armv7a-none-eabihf/debug/app-loader` (debug ELF) |
| `cargo build-app-loader-bin` | `target/armv7a-none-eabihf/release/app-loader.bin` (raw flashing binary) |

The on-card ELF / USB UF2 toolchain is in [`tools/uf2/`](../tools/uf2/) and
[`tools/elf2uf2/`](../tools/elf2uf2/). See the [workspace README](../README.md)
for flashing and debugging.

[Synthstrom Deluge]: https://synthstrom.com/product/deluge/
