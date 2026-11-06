# libdeluge

A safe Rust interface to the Deluge's hardware from its Linux userland, over
[`libdeluge-sys`](https://crates.io/crates/libdeluge-sys) (`libdeluge`): a real-time audio callback,
MIDI, front-panel input events, LEDs and pads, the OLED, CV and gate outputs, and USB hotplug.
Callbacks run on `libdeluge`'s own threads.

[`deluge-sdk`](https://crates.io/crates/deluge-sdk)'s `linux` feature runs an ordinary SDK app on
top of this crate, which is the usual way to use it. See the
[Linux examples](https://github.com/FirestormAudio/deluge-sdk/blob/main/examples/linux/README.md) for building and packing an image with
`cargo deluge linux`, and the crate's `examples/` for direct use.

Building needs `libdeluge`; see [`libdeluge-sys`](https://crates.io/crates/libdeluge-sys).

Callbacks are called from C, where a panic cannot unwind: a callback that panics aborts the
process.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-APACHE) or
[MIT license](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-MIT) at your option.
