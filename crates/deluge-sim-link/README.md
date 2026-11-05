# deluge-sim-link

The in-process link between a Deluge SDK app and the
[desktop simulator](https://crates.io/crates/deluge-simulator), used by `cargo deluge sim`.

When an SDK app is built for the host, the app and the simulator window run in one process: the
app's executor on a worker thread, the window on the main thread. This crate is the state they
share. `SharedPanel` carries what the app lights (OLED, pad colours, LEDs) and the input the
window sends back (pads, buttons, encoders); `audio` carries the real-time audio blocks over
lock-free rings, clocked by the window's audio callback.

It is an implementation detail of [`deluge-sdk`](https://crates.io/crates/deluge-sdk)'s `sim`
feature; apps don't use it directly.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-APACHE) or
[MIT license](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-MIT) at your option.
