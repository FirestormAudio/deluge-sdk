# libdeluge-sys

Raw FFI bindings to `libdeluge`, the hardware library of the Deluge's Linux userland. Use
[`libdeluge`](https://crates.io/crates/libdeluge) for a safe interface.

## Building

The bindings are checked in (`src/bindings.rs`, generated for `armv7-unknown-linux-musleabihf`),
so a build needs only libdeluge, from a `deluge-linux` release bundle's sysroot. The build script
looks under `DELUGE_SDK_ROOT` (the prefix holding `include/` and `lib/`) when it is set, then
asks `pkg-config` for `deluge`, then tries `/usr/local`. `cargo deluge linux` sets
`DELUGE_SDK_ROOT` for you from `DELUGE_BASE`. The crate links `libdeluge` statically when
`libdeluge.a` is there (dynamically otherwise) and `libasound` dynamically, and exports the header directory to dependent build scripts as
`DEP_DELUGE_INCLUDE`.

| Feature | Effect |
|---|---|
| `bindgen` | Regenerate the bindings from the installed headers at build time (needs `libclang`). With `LIBDELUGE_SYS_UPDATE=1` set, the checked-in copy is refreshed too. |

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-APACHE) or
[MIT license](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-MIT) at your option.
