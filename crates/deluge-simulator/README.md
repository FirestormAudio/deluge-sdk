# deluge-simulator

A desktop front panel for the Synthstrom Deluge. It renders the OLED, the 18 × 8 RGB pad grid,
buttons, encoders and LEDs, plays audio through your speakers and bridges MIDI, so Deluge apps
can be built and debugged without hardware.

It drives one of two kinds of "brain":

- **An SDK app in the same process.** This is what `cargo deluge sim` builds: the app's
  executor runs on a worker thread and shares state with the panel through the `link` module,
  with no serialization.
- **An external process** over the [`deluge-panel-protocol`](https://crates.io/crates/deluge-panel-protocol)
  wire (TCP or a Unix socket): `deluge-simulator --connect <target>`.

A headless, scripted mode renders golden frames for CI.

Most users never depend on this crate directly: enable the `sim` feature of
[`deluge-sdk`](https://crates.io/crates/deluge-sdk) and run `cargo deluge sim`. The
[simulator guide](https://github.com/FirestormAudio/deluge-sdk/blob/main/docs/simulator.md) covers the panel, audio, MIDI, the SD card directory and
headless testing.

## Features

| Feature | Default | Effect |
|---|---|---|
| `rack` | yes | The strip above the faceplate: CV/gate meters and L/R output scopes |
| `cli` | yes | The `deluge-simulator` binary (argument parsing and the logger). A library embedding the simulator turns default features off |

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-APACHE) or
[MIT license](https://github.com/FirestormAudio/deluge-sdk/blob/main/LICENSE-MIT) at your option.
