# deluge-block-device-adapters

Byte-level access to block devices, and partition windows over them: `BufStream` implements
`embedded-io-async`'s `Read`, `Write` and `Seek` over a
[`block-device-driver`](https://crates.io/crates/block-device-driver) `BlockDevice`, and
`StreamSlice` limits a stream to an offset range.

This is a fork of [MabezDev's `block-device-adapters`](https://github.com/MabezDev/embedded-fatfs)
with two upstream pull requests applied (zero-length transfers, 32-bit length truncation, and
checked seek arithmetic), maintained for the [Deluge SDK](https://github.com/FirestormAudio/deluge-sdk).
The library keeps upstream's name, `block_device_adapters`:

```toml
[dependencies]
block-device-adapters = { package = "deluge-block-device-adapters", version = "0.3" }
```

It is not a drop-in for upstream 0.2.0, though: it moves to `embedded-io-async` 0.7, so its
`Read`, `Write` and `Seek` impls are for 0.7's traits. `log` and `defmt` may both be on, in
which case `defmt` is used.

See [VENDOR.md](VENDOR.md) for exactly what differs from upstream.

## License

MIT, as upstream.
