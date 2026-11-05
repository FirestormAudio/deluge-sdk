# deluge-image

Pure firmware-image format logic for the Deluge SSB, with **no hardware
dependencies** so it unit-tests on the host and is shared by the on-device
app-loader — keeping the format decisions in one place.

This crate has **no dependencies**: it is pure `no_std` logic, so it builds and
tests on the host without a supply chain.

## Modules

| Module | Description |
|---|---|
| `elf` | `PT_LOAD` classification used by the streaming SD loader (uncached-mirror resolution, SRAM-staging / SDRAM-direct decisions), the slice-sourced load plan the USB dev-upload path uses, and FSB-metadata validation (`elf::validate_fsb_metadata`) the flash-store path runs before programming an image. |
| `crc` | The shared CRC-32 the USB upload framing and the on-flash settings record agree on, so the host tool and the device compute the same checksum. |
| `settings` | On-flash settings record layout. |

## Usage

```toml
[dependencies]
deluge-image = "0.1"
```

The crate is `no_std`; `cfg(test)` pulls in `std` so the test harness works,
which is why it builds on both the host and the embedded target.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
