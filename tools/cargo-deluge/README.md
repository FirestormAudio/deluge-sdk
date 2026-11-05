# cargo-deluge

The `cargo deluge` host subcommand: builds, scaffolds, uploads and runs apps written against the
[`deluge`](../../crates/deluge-sdk) SDK, so an app author never has to pass `-Zbuild-std`, linker
flags or the `armv7a-none-eabihf` target triple by hand. Apps are built as ELF files, which the
[app-loader](../../docs/app-loader.md) loads and runs from RAM. The tool is plain `std` Rust; its
only dependency is `serialport`, for the USB upload path. It has no Cargo features.

## Install

From a clone of the repository:

```sh
rustup show                              # installs the pinned nightly toolchain and target
cargo install --path tools/cargo-deluge
```

Cargo then runs the installed `cargo-deluge` binary for `cargo deluge <command>`.
`cargo deluge help` prints every command with its options.

## Commands

| Command | Does |
|---------|------|
| `cargo deluge new <name>` | Scaffold a new app crate (sources, `.cargo/config.toml`, VS Code tasks and launch configuration) |
| `cargo deluge build [--release]` | Build the current app to an ELF |
| `cargo deluge run [--release] [--port <p>] [--log]` | Build, upload the ELF over USB and launch it from RAM (needs dev mode on the app-loader) |
| `cargo deluge upload <path> [--port <p>] [--log]` | Upload an already-built ELF or appliance image without rebuilding |
| `cargo deluge deploy [--release] [--dest <mount>]` | Build, then copy the ELF to a mounted SD card's `/APPS/` |
| `cargo deluge log [--port <p>]` | Stream a running app's USB serial log (the app's `usb-log` feature) |
| `cargo deluge sim [--release] [--headless ...]` | Build for the host and run the app in the [desktop simulator](../../docs/simulator.md) |
| `cargo deluge linux [opts]` | Build for the Deluge's Linux userland and pack a bootable image (needs `DELUGE_BASE`) |
| `cargo deluge debug [--release] [-- ...]` | Build, then `probe-rs run` over J-Link |
| `cargo deluge trace [--release] [--flow] [-- ...]` | Build, then `probe-rs read-trace` (needs the `trace-a9` probe-rs fork) |

Notes on the less obvious ones:

- **`run`** strips the ELF to its loadable segments before sending it (`--no-strip` sends the
  whole file) and finds the Deluge's serial port by its USB identity unless `--port` is given.
  `--log` follows the app's USB log after launch.
- **`deploy`** is the path for a unit without dev mode. Without `--dest` it builds and prints
  where to copy the file.
- **`sim`** also takes `--audio-in <wav>`, `--audio-out <wav>` and `--hardware [<port>]`.
- **`linux`** cross-builds a static armv7 musl binary and packs it with the bundle's
  `deluge-mkimage`; `DELUGE_BASE` points at an unpacked release bundle. Options include
  `--bare`, `--profile <base|platform>`, `--overlay <dir>`, `--out <name>`, `--features <list>`,
  `--no-strip` and `--run` (dev-upload the packed image and launch it from RAM).
- **`debug`** and **`trace`** pass the RZ/A1L chip name to `probe-rs`; anything after `--` is
  forwarded to it. `trace` also takes `--duration-ms <n>` (default 2000).

## Usage

```sh
cargo deluge new myapp
cd myapp
cargo deluge run           # or: cargo deluge sim
```

See [Getting started](../../docs/getting-started.md) for the toolchain and the first app.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE) at your
option.
