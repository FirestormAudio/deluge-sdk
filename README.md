# deluge-embassy

Demo Rust firmware for the [Synthstrom Deluge], built on
[Embassy] and targeting the onboard Renesas RZ/A1L (ARM Cortex-A9,
R7S721001).

---

## Repository layout

Firmware binaries live under [`firmwares/`](firmwares/); the rest are shared
libraries and the on-board app loader.

| Crate | Description |
|-------|-------------|
| [`firmwares/demo-firmware`](firmwares/demo-firmware/) | Top-level demo firmware — Embassy executor, USB stack, audio, task orchestration |
| [`firmwares/controller-firmware`](firmwares/controller-firmware/) | `deluge-controller` firmware — USB host/CDC controller build |
| [`firmwares/msc-firmware`](firmwares/msc-firmware/) | USB Mass Storage Class firmware build |
| [`app-loader`](app-loader/) | Second-stage bootloader / app loader — OLED file selector, SD-card ELF + USB UF2 flashing |
| [`crates/rza1l-hal`](crates/rza1l-hal/) | Register-level HAL for the RZ/A1L SoC (MMU, caches, GIC, timers, DMA, RSPI, SSI, SCUX, SDHI, …) |
| [`crates/deluge-bsp`](crates/deluge-bsp/) | Board support package — SDRAM, audio codec, OLED, PIC co-processor, CV/gate, MIDI, SD card, USB |
| [`crates/deluge-fft`](crates/deluge-fft/) | `no_std` FFT library with RZ/A1L-tuned radix-4/8 paths and a real-FFT spectrum analyser |
| [`crates/deluge`](crates/deluge/) | Maker-friendly app SDK facade — the `Deluge` handle + `#[deluge::app]` |
| [`crates/deluge-macros`](crates/deluge-macros/) | The `#[deluge::app]` attribute macro |
| [`crates/fixedpoint`](crates/fixedpoint/) | Type-safe fixed-point arithmetic (Q31/Q16/…) for DSP |
| [`crates/armv7-dsp-intrinsics`](crates/armv7-dsp-intrinsics/) | ARMv7 DSP intrinsics (SMMUL/SSAT/QADD/VCVT) backing `fixedpoint` |
| [`crates/deluge-ui`](crates/deluge-ui/) | OLED UI toolkit — Deluge fonts, text, graphics, icons, immediate-mode menus (**GPL-3.0**) |
| [`crates/deluge-fonts`](crates/deluge-fonts/) | Deluge OLED font assets (**GPL-3.0**) |

---

## Licensing

The SDK and core libraries are **`MIT OR Apache-2.0`**. The OLED UI toolkit
(`crates/deluge-ui`, package `deluge-ui-toolkit`) and its fonts
(`crates/deluge-fonts`, package `embedded-fonts-deluge`) are **`GPL-3.0-or-later`**
(see [`LICENSE-GPL`](LICENSE-GPL)). They are standalone crates: the permissive
`deluge` facade does **not** depend on them, so an app stays MIT/Apache unless it
opts into the toolkit, in which case that app becomes GPL.

Apps that use the toolkit (menus/text) need a global allocator: enable the
`deluge` crate's `alloc` feature (registers the on-chip SRAM heap) and build with
`-Zbuild-std=core,alloc` (the `cargo build-fw-alloc` alias). See
[`examples/oled_menu`](examples/oled_menu/).

---

## Target

| Property      | Value |
|---------------|-------|
| CPU           | ARM Cortex-A9, ARMv7-A, 400 MHz |
| SoC           | Renesas RZ/A1L (R7S721001) |
| RAM           | 3 MB on-chip SRAM + 64 MB SDRAM |
| Rust target   | `armv7a-none-eabihf` |
| Rust channel  | nightly (see [`rust-toolchain.toml`](rust-toolchain.toml)) |
| Async runtime | [Embassy] |

---

## Prerequisites

- Rust nightly with the `armv7a-none-eabihf` target and `llvm-tools-preview`
  (managed automatically by `rust-toolchain.toml`):
  ```sh
  rustup show   # installs the toolchain on first run
  ```
- [`cargo-binutils`] for the `build-fw-bin` alias:
  ```sh
  cargo install cargo-binutils
  ```
- A J-Link probe and `JLinkGDBServerCLExe` on `$PATH` for flashing/debugging.

---

## Building

`.cargo/config.toml` sets `target = "armv7a-none-eabihf"` and defines these
aliases — always use them instead of bare `cargo build` so the required
`-Zbuild-std=core` nightly flags are passed correctly:

| Command | Output | Notes |
|---------|--------|-------|
| `cargo build-fw` | `target/armv7a-none-eabihf/debug/demo-firmware` | Debug ELF, RTT enabled |
| `cargo build-fw-rel` | `target/armv7a-none-eabihf/release/demo-firmware` | Release ELF, RTT disabled |
| `cargo build-fw-bin` | `target/armv7a-none-eabihf/release/demo-firmware.bin` | Raw binary for flashing |

---

## Testing

The default `armv7a-none-eabihf` target is bare-metal (no `std`), so the test
harness can't build there. Tests instead run on two host-side targets:

- **QEMU ARM** (`armv7-unknown-linux-gnueabihf`, under `qemu-arm`) — crates that
  use ARM inline asm or ARM/NEON intrinsics (`rza1l-hal`, `deluge-bsp`,
  `fixedpoint`, `armv7-dsp-intrinsics`, `deluge-fft`).
- **Host** (`x86_64-unknown-linux-gnu`) — pure-logic / std crates and ones whose
  dev-deps don't cross-compile to ARM (`deluge-ui-toolkit`, host tools).

Run everything with one command:

```sh
./tools/test.sh
```

Prerequisites (one-time):

```sh
rustup target add armv7-unknown-linux-gnueabihf x86_64-unknown-linux-gnu
# Debian/Ubuntu:
sudo apt-get install qemu-user gcc-arm-linux-gnueabihf
# Arch:
sudo pacman -S qemu-user-static arm-linux-gnueabihf-gcc
```

The `qemu-arm` runner and ARM cross-linker are wired up in `.cargo/config.toml`.
CI runs the same script (`.github/workflows/test.yml`).

---

## Debugging

Two probe backends are supported: J-Link (stable, recommended) and a custom
`probe-rs` fork with Cortex-A9 trace support.

### J-Link

Open the project in VS Code with the [Cortex-Debug] extension installed, then
use one of the launch configurations in [`.vscode/launch.json`](.vscode/launch.json):

- **Rust firmware (debug)** — loads the debug ELF via J-Link and attaches RTT.
- **Rust firmware (release)** — loads the release ELF.

Both configurations use [`rza1_debug.JLinkScript`](rza1_debug.JLinkScript) to
halt the CPU and configure it for direct SRAM execution without going through
the ROM bootloader. Requires `JLinkGDBServerCLExe` on `$PATH`.

RTT log output is written to `rtt.log` in the workspace root and streamed to
the Cortex-Debug terminal.

### probe-rs (`trace-a9` fork)

A fork of probe-rs with Cortex-A9 PTM/ETF trace support for the RZ/A1L is
available at [`stellar-aria/probe-rs`][probe-rs-fork], `trace-a9` branch.
A vendored copy is kept in [`tools/probe-rs/`](tools/probe-rs/).

Build and install from the fork:

```sh
cd ~/GitHub/probe-rs          # or tools/probe-rs/
cargo install --path probe-rs-tools --locked
```

**Flash and run** (streams RTT to the terminal):

```sh
probe-rs run --chip R7S721020 target/armv7a-none-eabihf/debug/demo-firmware
```

**Capture PTM instruction trace** from the on-chip ETF buffer:

```sh
# Dump raw bytes for offline decoding (ptm2human / Trace Compass):
probe-rs read-trace --chip R7S721020 --duration-ms 2000 --output trace.bin

# Decode inline and print packets:
probe-rs read-trace --chip R7S721020 --duration-ms 2000 --decode \
    --elf target/armv7a-none-eabihf/debug/demo-firmware

# Compact execution-flow view (ISync + branch targets with symbols):
probe-rs read-trace --chip R7S721020 --duration-ms 2000 --flow \
    --elf target/armv7a-none-eabihf/debug/demo-firmware

# JSON Lines output for programmatic processing:
probe-rs read-trace --chip R7S721020 --duration-ms 2000 --decode \
    --elf target/armv7a-none-eabihf/debug/demo-firmware \
    --output-format json 2>&1 | jq 'select(.type == "Branch")'
```

---

## CI

The [CI workflow](.github/workflows/ci.yml) builds the release firmware binary
on every push to `main` and on pull requests. The resulting
`firmware-release` artifact contains `demo-firmware.bin`.

[Synthstrom Deluge]: https://synthstrom.com/product/deluge/
[Embassy]: https://embassy.dev
[`cargo-binutils`]: https://github.com/rust-embedded/cargo-binutils
[Cortex-Debug]: https://marketplace.visualstudio.com/items?itemName=marus25.cortex-debug
[probe-rs-fork]: https://github.com/stellar-aria/probe-rs/tree/trace-a9
