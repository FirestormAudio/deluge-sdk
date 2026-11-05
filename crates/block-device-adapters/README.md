# deluge-block-device-adapters

Byte-level access to block devices, and partition windows over them: `BufStream` implements
`embedded-io-async`'s `Read`, `Write` and `Seek` over a
[`block-device-driver`](https://crates.io/crates/block-device-driver) `BlockDevice`, and
`StreamSlice` limits a stream to an offset range.

This is a fork of [MabezDev's `block-device-adapters`](https://github.com/MabezDev/embedded-fatfs)
with two upstream pull requests applied (zero-length transfers, 32-bit length truncation, and
checked seek arithmetic), maintained for the [Deluge SDK](https://github.com/FirestormAudio/deluge-sdk).
The library keeps upstream's name, `block_device_adapters`, so it is a drop-in replacement:

```toml
[dependencies]
block-device-adapters = { package = "deluge-block-device-adapters", version = "0.2" }
```

See [VENDOR.md](VENDOR.md) for exactly what differs from upstream.

## License

MIT, as upstream.
