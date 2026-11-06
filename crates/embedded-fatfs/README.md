# deluge-embedded-fatfs

A fork of [MabezDev's `embedded-fatfs`](https://github.com/MabezDev/embedded-fatfs) with upstream
fixes and a few additions applied, maintained for the
[Deluge SDK](https://github.com/FirestormAudio/deluge-sdk). The library keeps upstream's name,
`embedded_fatfs`, so it is a drop-in replacement:

```toml
[dependencies]
embedded-fatfs = { package = "deluge-embedded-fatfs", version = "0.2" }
```

Unlike upstream, the default features (`lfn`, `unicode`) build for `no_std` targets: `chrono`
(which needs `std`) and logging are opt-in, and `log` and `defmt` may both be on, in which case
`defmt` is used. See [VENDOR.md](VENDOR.md) for exactly what differs from upstream. The rest of
this README is upstream's.

A FAT filesystem library implemented in Rust. Built on the shoulders of the amazing [rust-fatfs](https://github.com/rafalh/rust-fatfs) crate by [@rafalh](https://github.com/rafalh).

## Features
* async
* read/write to files using `embedded-io-async` Read/Write traits
* read directory contents
* create/remove file or directory
* rename/move file or directory
* read/write file timestamps (updated automatically if `chrono` feature is enabled)
* format volume
* FAT12, FAT16, FAT32 compatibility
* LFN (Long File Names) extension is supported
* `no_std` environment support

## Porting from rust-fatfs to embedded-fatfs

There a are a few key differences between the crates:

- embedded-fatfs is async, therefore your storage device must implement the [embedded-io-async](https://github.com/rust-embedded/embedded-hal/tree/master/embedded-io-async) traits.
- You must call `flush` on `File`s before they are dropped. See the CHANGELOG for details.

`no_std` usage
------------

Add this to your `Cargo.toml`:

    [dependencies]
    embedded-fatfs = { package = "deluge-embedded-fatfs", version = "0.2", default-features = false }

Additional features:

* `lfn` - LFN (long file name) support
* `alloc` - use `alloc` crate for dynamic allocation. Needed for API which uses `String` type. You may have to provide
a memory allocator implementation.
* `unicode` - use Unicode-compatible case conversion in file names - you may want to have it disabled for lower memory
footprint
* `chrono` - `chrono` conversions and the local clock as the time provider (implies `std`)
* `log` / `defmt` - log through either; `defmt` wins when both are on

License
-------
The MIT license. See `LICENSE`.
