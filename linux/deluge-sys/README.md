# deluge-sys

Raw FFI bindings to `libdeluge`, the hardware library of the Deluge's Linux userland, generated
with `bindgen` at build time. Use [`deluge-hal-linux`](https://crates.io/crates/deluge-hal-linux)
for a safe interface.

## Building

The build needs `libdeluge.a` and its headers, from a `deluge-linux` release bundle's sysroot.
Set `DELUGE_SDK_ROOT` to the prefix that holds `include/` and `lib/` (the default is
`/usr/local`); `cargo deluge linux` sets it for you from `DELUGE_BASE`. The crate links
`libdeluge` statically and `libasound` dynamically, and `bindgen` needs `libclang`.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-APACHE) or
[MIT license](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-MIT) at your option.
