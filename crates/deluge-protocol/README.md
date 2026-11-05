# deluge-protocol

The wire protocol between a Deluge acting as a control surface and the host that drives it. The
host owns all illumination (OLED, pad LEDs, button LEDs, knob indicators, CV/gate) and the
device forwards raw input (pads, buttons, encoders). This crate defines the message type bytes,
the length-prefixed framing, a streaming frame decoder and typed `no_std` codecs for both
directions. The desktop simulator ([`deluge-simulator`](../deluge-simulator)) uses it for the
panel side; [`controller-firmware`](../../firmwares/controller-firmware) speaks the device side
over USB-CDC.

## Framing

Every message is `[len: u16 LE][type: u8][data: N bytes]`, where `len = 1 + N`. A device-to-host
frame fits one 64-byte full-speed bulk packet; host-to-device frames (the 768-byte display blit,
the 432-byte full pad grid) are reassembled from packets by `FrameDecoder`.

Directions are named from the device's point of view: `ToDeluge` is host to device (display,
LEDs, CV/gate, version and ping queries), `FromDeluge` is device to host (input, version, pong,
ready). Button ids on the wire are offset by `BUTTON_ID_BASE` (144); see `cdc_button_id`.

## Features

| Feature | Effect |
|---|---|
| `alloc` | `frame` and the `to_frame` methods, which return the encoded frame as a `Vec<u8>` |
| `std` | Host-side conveniences; implies `alloc`. Enable it in std tools such as the simulator |

With no features the crate is `no_std` and allocation-free: encode with `encode_frame` into a
caller's buffer and decode with `FrameDecoder` and `ToDeluge::decode` / `FromDeluge::decode`.

## Usage

```rust
use deluge_protocol::{FrameDecoder, FromDeluge, ToDeluge, encode_frame, to};

// Encode a host-to-device message without allocating.
let mut wire = [0u8; 16];
let n = encode_frame(to::SET_PAD_RGB, &[3, 5, 0xFF, 0x00, 0x40], &mut wire).unwrap();

// Reassemble frames from arbitrary chunks and decode them.
let mut decoder = FrameDecoder::<2048>::new();
decoder.push(&wire[..n]);
let mut payload = [0u8; 1024];
while let Some((type_byte, len)) = decoder.pop_frame(&mut payload) {
    if let Some(msg) = ToDeluge::decode(type_byte, &payload[..len]) {
        assert_eq!(msg, ToDeluge::SetPadRgb { col: 3, row: 5, rgb: [0xFF, 0x00, 0x40] });
    }
}

// With the `alloc` feature: build a framed device-to-host message directly.
let frame = FromDeluge::EncoderRotated { id: 2, delta: -1 }.to_frame();
```

The `golden` integration test pins the encoding against the vectors in
[`tools/deluge-web-sim/src/protocol-golden.json`](../../tools/deluge-web-sim/src/protocol-golden.json),
which the web simulator's TypeScript codec is also checked against:

```sh
cargo test -p deluge-protocol --features std --test golden
```

## License

Licensed under either of [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE) at your
option.
