# usb-classes

Board-agnostic USB classes on [Embassy](https://embassy.dev). Device classes are generic over
any `embassy_usb::driver::Driver` and host class drivers over any
`embassy_usb_driver::host::UsbHostAllocator`, so nothing here knows which chip or board it runs
on. On the Deluge the chip-level RUSB1 drivers come from
[`rza1-hal`](../../rza1/rza1-hal), and [`deluge-bsp`](../../crates/deluge-bsp) adds the board's
USB identities, the SD card as a Mass Storage medium and the host-role supervisor.

Device classes build only for the bare-metal target (`target_os = "none"`), where `embassy-usb`
is a dependency. The MIDI packet queues, the Overbridge wire format and the host class drivers
also build on Linux, and are unit-tested under QEMU against a mock host allocator.

## Features

No class is enabled by default; turn on only the ones a firmware uses.

| Feature | Effect |
|---|---|
| `device-audio` | `audio`: the UAC2 audio device class |
| `device-midi` | `midi`: the USB-MIDI device class (MIDI 1.0 on alt 0, MIDI 2.0 UMP on alt 1), and `midi_packets`, the packet-mode queues behind it |
| `device-msc` | `msc`: the Mass Storage device class, and `bot`, the Bulk-Only Transport / SCSI engine over a `bot::BlockDevice` |
| `host-midi` | `host::midi`: the USB-MIDI 1.0 host class driver |
| `host-uac` | `host::uac`: the UAC host class driver (capture and playback) |
| `host-overbridge` | `host::overbridge`: the Elektron Overbridge 2 host class driver, with `overbridge::proto` and `overbridge::stems` |
| `device-overbridge` | `overbridge::device`: the Elektron Overbridge 2 device class, with `overbridge::proto` and `overbridge::stems` |

The `host-*` features pull in `embassy-usb-host`.

## Usage

The USB-MIDI device class, with the `device-midi` feature. `build` registers the function with
an `embassy_usb::Builder` and returns its control handler and four bulk endpoints; the `run_*`
futures drive the endpoints, and the app exchanges messages through non-blocking queues:

```rust,ignore
use usb_classes::midi;

let (mut midi_handler, eps) = midi::build(&mut builder);
builder.handler(&mut midi_handler);
let mut usb = builder.build();

// Run alongside `usb.run()`, e.g. one task each or joined.
midi::run_rx_midi1::<D>(eps.ep_out_midi1);
midi::run_tx_midi1::<D>(eps.ep_in_midi1);
midi::run_rx_midi2::<D>(eps.ep_out_midi2);
midi::run_tx_midi2::<D>(eps.ep_in_midi2);

// From application code:
if midi::midi2_active() {
    if let Some(ump) = midi::try_recv_ump_from_host() { /* ... */ }
} else {
    if let Some([status, d1, d2]) = midi::try_recv_from_host() { /* ... */ }
    midi::try_send_to_host_byte(0xF8); // MIDI clock
}
```

A consumer that already speaks USB-MIDI 1.0 event packets calls `midi::use_packets()` before
starting the alt-0 futures, then uses `try_recv_packet_from_host` / `try_send_packet_to_host`,
which pass packets through unchanged (SysEx and cable numbers included).

## License

Licensed under either of [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE) at your
option.
