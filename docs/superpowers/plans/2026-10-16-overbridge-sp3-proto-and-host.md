# Overbridge sub-project 3: protocol and host class Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Deluge hosting a Digitakt (or any listed Overbridge 2 device) receives every stem as
48 kHz `i32` frames and sends a stream back, through `usb-classes`' new `host-overbridge` class
and the BSP's USB host supervisor. The pipes are driven by the existing per-transfer CPU path;
sub-project 2 adds DMA under it.

**Architecture:**
- **`overbridge::proto`** (no USB dependency): the device table, block sizes, sample codecs for
  v2 and v2.1 in both directions, and a frame-counter tracker. Shared with sub-project 4's
  device class.
- **`overbridge::stems`**: `OverbridgeHandle<RX, TX>`, a lock-protected pair of interleaved
  `i32` rings plus `OverbridgeInfo` and `StreamStats`. It is the app-facing API; the stream task
  pushes received frames into it and pulls frames to send.
- **`host::overbridge`**: match, setup (`SET_INTERFACE` 1/3 then 2/3), and `pump_once`. Each
  `pump_once` reads one IN block, decodes it into the handle, and sends one OUT block (OUT is
  paced by IN). Interrupt endpoints are opened as interrupt pipes; sub-project 1's HAL carries
  those over 64 bytes on bulk pipes.
- **BSP**: feature `usb-host-overbridge`. The supervisor tries Overbridge first for VID 0x1935,
  skips UAC when it binds, and runs a stream task that restarts on transport errors (three in
  10 s ends it).
- **Hardware harness**: `hw-tests/overbridge-host-firmware` logs info, stats and per-stem peaks
  every second, and sends a 1 kHz tone. It goes with `docs/hardware-tasks.md` (spec §8.4).

**Tech Stack:** Rust no_std; `usb-classes` (embassy-usb-host 0.1 traits, the existing
`host::mock`); `deluge-bsp`; embassy-sync blocking mutex.

**Spec:** `docs/superpowers/specs/2026-10-15-overbridge-design.md` §2, §5 (5.1–5.5), §8.1, §8.4.

## Global Constraints

- **Wire format** (spec §2):
  - Block = 32-byte header (`0x0700` from device / `0x07FF` to device, big-endian; counter +7
    per block, big-endian; 28 zero bytes) + 7 frames interleaved by channel.
  - v2: full-scale big-endian `i32`.
  - v2.1 from device: channels 0–13 are 4-byte (decode `i32::from_be_bytes(b) << 8`), the rest
    are 3-byte (decode `i32::from_be_bytes([b0, b1, b2, 0])`).
  - v2.1 to device: 4-byte `(x >> 8).to_be_bytes()`.
- **Setup:** `SET_INTERFACE`(1, 3) then `SET_INTERFACE`(2, 3); no clear-halt or name request;
  prime OUT with 2 silent blocks.
- **Matching:** VID 0x1935; interface 1 alt 3 interrupt IN; interface 2 alt 3 interrupt OUT.
  Channel counts come from MaxPacket and the format, and descriptors win over the table. A
  non-integral or zero channel count is rejected.
- **Errors** (§5.5):
  - Wrong header or length → drop the block and count a framing error; 64 consecutive →
    transport error.
  - Counter gap → count the missing blocks.
  - Transport error → restart (alternates again); 3 in 10 s → give up.
  - `SET_INTERFACE` failure → `SetupFailed`; retry once after 100 ms, then give up until re-plug.
- **Stems:** frames are stored decoded at full scale. On overrun, drop the oldest and count. On
  underrun, pad silence and count. A device whose rings can't hold 56 frames (8 blocks) is
  refused with `TooManyChannels`.
- **The class never resamples.** `rate_ratio_ppm` is the device's frame rate against local time.

## Review Focus

1. **A short or zero-length IN transfer** (a device glitch, or the mock's empty script). It must
   count a framing error, still send the OUT block, and never panic on slicing. Pinned by
   `a_short_block_is_a_framing_error_and_out_still_runs`.
2. **Counter wrap and restart.** The counter wraps at 16 bits (0xFFFA → 0x0001 is no gap). A
   device that restarts its counter mid-stream must not report thousands of missing blocks.
   Pinned by `counter_wraps_without_a_gap` and `a_misaligned_counter_resyncs_without_counting`.
3. **More channels than the handle holds** (a 42-channel Digitakt II into a small handle). It
   must refuse at bind time, never truncate. Pinned by `a_handle_too_small_is_refused`.
4. **An Elektron device in its Audio/MIDI USB mode** (no vendor interfaces). The Overbridge
   match must fail cleanly, so UAC still binds. Pinned by `an_elektron_device_without_alt_3_does_not_match`.
5. **Detach mid-stream.** The pipe returns `BadResponse`; `pump_once` must return `Err`, never
   hang or loop. Pinned by `a_pipe_error_is_a_transport_error`.

---

### Task 1: `overbridge::proto`

**Files:**
- Modify: `usb/usb-classes/Cargo.toml` (features)
- Modify: `usb/usb-classes/src/lib.rs`
- Create: `usb/usb-classes/src/overbridge/mod.rs`, `usb/usb-classes/src/overbridge/proto.rs`
- Modify: `tools/test.sh` (usb-classes test features)

**Interfaces:**
- Produces (`usb_classes::overbridge::proto`):
  - Constants: `VID_ELEKTRON`, `IN_INTERFACE`, `OUT_INTERFACE`, `STREAM_ALT`,
    `FRAMES_PER_BLOCK`, `HEADER_LEN`, `HEADER_TO_DEVICE`, `HEADER_FROM_DEVICE`, `SAMPLE_RATE`,
    `MAX_BLOCK`, `MAX_CHANNELS`, `V2_1_WIDE`.
  - `enum Format { V2, V2_1 }`, `struct Device`, `DEVICES`, `fn lookup(pid) -> Option<&'static Device>`.
  - `Format::{from_device_frame_bytes, to_device_frame_bytes, channels_from_device, channels_to_device}`.
  - `fn block_len(frame_bytes) -> usize`.
  - `enum BlockError { Length, Header }`.
  - `fn decode_from_device(Format, ch, &[u8], &mut [i32]) -> Result<u16, BlockError>`.
  - `fn encode_to_device(Format, ch, counter, &[i32], &mut [u8]) -> usize`.
  - `fn encode_from_device(..) -> usize` and `fn decode_to_device(..) -> Result<u16, BlockError>`
    (for sub-project 4).
  - `struct CounterTracker { fn observe(&mut self, u16) -> u32; fn reset(&mut self) }`.

- [ ] **Step 1: Features and module skeleton**

In `usb/usb-classes/Cargo.toml` `[features]`, add after `host-uac`:

```toml
## Elektron Overbridge 2 host class driver (`host::overbridge`), with `overbridge::proto` and
## `overbridge::stems`.
host-overbridge = ["dep:embassy-usb-host"]
## Elektron Overbridge 2 device class (`overbridge::device`), with `overbridge::proto` and
## `overbridge::stems`.
device-overbridge = []
```

In `usb/usb-classes/src/lib.rs`, add after the `host` module:

```rust
#[cfg(any(feature = "host-overbridge", feature = "device-overbridge"))]
pub mod overbridge;
```

Change the host module gate to
`#[cfg(any(feature = "host-midi", feature = "host-uac", feature = "host-overbridge"))]`.

Create `usb/usb-classes/src/overbridge/mod.rs`:

```rust
//! Elektron Overbridge 2: the wire format ([`proto`]) shared by the host class
//! (`host::overbridge`) and the device class.

pub mod proto;
```

In `tools/test.sh`, change the usb-classes test line's features to
`device-audio,device-midi,host-midi,host-uac,host-overbridge,device-overbridge`.

- [ ] **Step 2: Write the failing tests**

Create `usb/usb-classes/src/overbridge/proto.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// A device-to-host block built byte by byte, independently of the encoder: header 0x0700,
    /// `counter`, 28 zero bytes, then `samples` (already in wire form) in order.
    fn wire_block(header: u16, counter: u16, samples: &[&[u8]]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&header.to_be_bytes());
        b.extend_from_slice(&counter.to_be_bytes());
        b.extend_from_slice(&[0; 28]);
        for s in samples {
            b.extend_from_slice(s);
        }
        b
    }

    /// Distinct test value for frame `f`, channel `c`, negative on odd channels.
    fn value(f: usize, c: usize) -> i32 {
        let v = ((f * 64 + c + 1) as i32) << 16;
        if c % 2 == 1 { -v } else { v }
    }

    #[test]
    fn the_digitakt_is_listed_with_12_channels_from_and_2_to() {
        let d = lookup(0x0B2C).unwrap();
        assert_eq!((d.name, d.ch_from, d.ch_to, d.format), ("Digitakt", 12, 2, Format::V2));
        let d2 = lookup(0x0B2B).unwrap();
        assert_eq!((d2.name, d2.ch_from, d2.ch_to, d2.format), ("Digitakt II", 42, 8, Format::V2_1));
        assert!(lookup(0x0001).is_none());
    }

    #[test]
    fn block_sizes_match_the_spec_table() {
        assert_eq!(block_len(Format::V2.from_device_frame_bytes(12)), 368);
        assert_eq!(block_len(Format::V2.to_device_frame_bytes(2)), 88);
        assert_eq!(block_len(Format::V2_1.from_device_frame_bytes(42)), 1012);
        assert_eq!(block_len(Format::V2_1.to_device_frame_bytes(8)), 256);
    }

    #[test]
    fn channels_follow_from_the_block_size() {
        assert_eq!(Format::V2.channels_from_device(368), Some(12));
        assert_eq!(Format::V2.channels_to_device(88), Some(2));
        assert_eq!(Format::V2_1.channels_from_device(1012), Some(42));
        assert_eq!(Format::V2_1.channels_to_device(256), Some(8));
        assert_eq!(Format::V2_1.channels_from_device(32 + 7 * 8), Some(2), "below 14 channels all are wide");
        // Rejected: not a whole number of 7-frame blocks, not whole samples, or no channels.
        assert_eq!(Format::V2.channels_from_device(369), None);
        assert_eq!(Format::V2.channels_from_device(32 + 7 * 6), None);
        assert_eq!(Format::V2.channels_from_device(32), None);
        assert_eq!(Format::V2.channels_from_device(10), None);
    }

    #[test]
    fn a_digitakt_block_decodes_to_its_stems_in_order() {
        let mut wire: Vec<[u8; 4]> = Vec::new();
        for f in 0..7 {
            for c in 0..12 {
                wire.push(value(f, c).to_be_bytes());
            }
        }
        let refs: Vec<&[u8]> = wire.iter().map(|w| &w[..]).collect();
        let block = wire_block(0x0700, 0x1234, &refs);
        assert_eq!(block.len(), 368);
        let mut out = [0i32; 7 * 12];
        assert_eq!(decode_from_device(Format::V2, 12, &block, &mut out), Ok(0x1234));
        for f in 0..7 {
            for c in 0..12 {
                assert_eq!(out[f * 12 + c], value(f, c), "frame {f} channel {c}");
            }
        }
    }

    #[test]
    fn a_digitakt_out_block_encodes_header_counter_and_samples() {
        let frames: Vec<i32> = (0..7).flat_map(|f| [value(f, 0), value(f, 1)]).collect();
        let mut block = [0xAAu8; 128];
        let n = encode_to_device(Format::V2, 2, 0x0007, &frames, &mut block);
        assert_eq!(n, 88);
        let mut wire: Vec<[u8; 4]> = Vec::new();
        for f in 0..7 {
            wire.push(value(f, 0).to_be_bytes());
            wire.push(value(f, 1).to_be_bytes());
        }
        let refs: Vec<&[u8]> = wire.iter().map(|w| &w[..]).collect();
        assert_eq!(&block[..88], &wire_block(0x07FF, 0x0007, &refs)[..]);
    }

    /// Spec §8.1 "v2.1 sample widths".
    #[test]
    fn v2_1_samples_are_24_bit_in_both_widths() {
        // From the device: channel 0 (wide) `00 12 34 56`, channel 14 (narrow) `12 34 56`.
        let mut samples: Vec<Vec<u8>> = Vec::new();
        for _f in 0..7 {
            for c in 0..42 {
                samples.push(if c < 14 { std::vec![0x00, 0x12, 0x34, 0x56] } else { std::vec![0x12, 0x34, 0x56] });
            }
        }
        let refs: Vec<&[u8]> = samples.iter().map(|s| &s[..]).collect();
        let block = wire_block(0x0700, 0, &refs);
        assert_eq!(block.len(), 1012);
        let mut out = [0i32; 7 * 42];
        decode_from_device(Format::V2_1, 42, &block, &mut out).unwrap();
        assert!(out.iter().all(|&s| s == 0x1234_5600), "{:x?}", &out[..16]);

        // To the device: 0x1234_5678 → `00 12 34 56`; -0x100 → `ff ff ff ff`.
        let mut frames = [0i32; 7 * 8];
        frames[0] = 0x1234_5678;
        frames[1] = -0x100;
        let mut block = [0u8; 256];
        assert_eq!(encode_to_device(Format::V2_1, 8, 0, &frames, &mut block), 256);
        assert_eq!(&block[32..36], &[0x00, 0x12, 0x34, 0x56]);
        assert_eq!(&block[36..40], &[0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn device_side_codecs_round_trip_with_the_host_side() {
        for (format, ch_from, ch_to) in [(Format::V2, 12, 2), (Format::V2_1, 42, 8)] {
            // Device → host. v2.1 carries 24 bits, so test values have a zero low byte.
            let frames: Vec<i32> = (0..7 * ch_from).map(|i| value(i / ch_from, i % ch_from)).collect();
            let mut block = [0u8; MAX_BLOCK];
            let n = encode_from_device(format, ch_from, 21, &frames, &mut block);
            let mut back = std::vec![0i32; 7 * ch_from];
            assert_eq!(decode_from_device(format, ch_from, &block[..n], &mut back), Ok(21));
            assert_eq!(back, frames);
            // Host → device.
            let frames: Vec<i32> = (0..7 * ch_to).map(|i| value(i / ch_to, i % ch_to)).collect();
            let n = encode_to_device(format, ch_to, 28, &frames, &mut block);
            let mut back = std::vec![0i32; 7 * ch_to];
            assert_eq!(decode_to_device(format, ch_to, &block[..n], &mut back), Ok(28));
            assert_eq!(back, frames);
        }
    }

    #[test]
    fn a_wrong_header_or_length_is_refused() {
        let mut block = [0u8; 368];
        block[..2].copy_from_slice(&0x0700u16.to_be_bytes());
        let mut out = [0i32; 7 * 12];
        assert!(decode_from_device(Format::V2, 12, &block, &mut out).is_ok());
        assert_eq!(decode_from_device(Format::V2, 12, &block[..367], &mut out), Err(BlockError::Length));
        assert_eq!(decode_from_device(Format::V2, 12, &[], &mut out), Err(BlockError::Length));
        block[1] = 0xFF; // 0x07FF: a host-to-device header
        assert_eq!(decode_from_device(Format::V2, 12, &block, &mut out), Err(BlockError::Header));
    }

    #[test]
    fn counter_gaps_count_missing_blocks() {
        let mut t = CounterTracker::default();
        assert_eq!(t.observe(100), 0, "the first block sets the expectation");
        assert_eq!(t.observe(107), 0);
        assert_eq!(t.observe(128), 2, "114 and 121 missing");
        assert_eq!(t.observe(135), 0);
    }

    #[test]
    fn counter_wraps_without_a_gap() {
        let mut t = CounterTracker::default();
        t.observe(0xFFFA);
        assert_eq!(t.observe(0x0001), 0);
    }

    #[test]
    fn a_misaligned_counter_resyncs_without_counting() {
        let mut t = CounterTracker::default();
        t.observe(700);
        assert_eq!(t.observe(3), 0, "device restarted its counter");
        assert_eq!(t.observe(10), 0, "and the new sequence is tracked");
        assert_eq!(t.observe(17 + 7 * 5000), 0, "an implausible jump resyncs too");
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

```bash
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p usb-classes \
  --features device-audio,device-midi,host-midi,host-uac,host-overbridge,device-overbridge --lib overbridge 2>&1 | grep -E "^error" | sort | uniq -c
```
Expected: compile errors (`lookup`, `Format`, `block_len`... not found).

- [ ] **Step 4: Implement**

Put this above the test module in `proto.rs`:

```rust
//! Elektron Overbridge 2 wire format: the device table, block sizes, sample codecs and the
//! frame counter. No USB dependency; the host class and the device class both use it.
//!
//! From Overwitch (read for the format only) and the Outbox 8 firmware analysis; see
//! `docs/superpowers/specs/2026-10-15-overbridge-design.md` §2.

/// Elektron's USB vendor ID.
pub const VID_ELEKTRON: u16 = 0x1935;
/// Audio from the device: interface 1, interrupt IN in [`STREAM_ALT`].
pub const IN_INTERFACE: u8 = 1;
/// Audio to the device: interface 2, interrupt OUT in [`STREAM_ALT`].
pub const OUT_INTERFACE: u8 = 2;
/// The alternate setting hosts select on both audio interfaces.
pub const STREAM_ALT: u8 = 3;
/// Frames in one block (one USB packet).
pub const FRAMES_PER_BLOCK: usize = 7;
/// Bytes before the samples: header word, frame counter, 28 reserved.
pub const HEADER_LEN: usize = 32;
/// Header word of a host-to-device block.
pub const HEADER_TO_DEVICE: u16 = 0x07FF;
/// Header word of a device-to-host block.
pub const HEADER_FROM_DEVICE: u16 = 0x0700;
/// Every Overbridge 2 device runs at 48 kHz.
pub const SAMPLE_RATE: u32 = 48_000;
/// The high-speed packet limit, which bounds any block (the largest listed is 1012 bytes).
pub const MAX_BLOCK: usize = 1024;
/// The most channels a block can carry in either direction (Overwitch's limit too).
pub const MAX_CHANNELS: usize = 64;
/// v2.1, from the device: this many leading channels are 4-byte samples, the rest 3-byte.
pub const V2_1_WIDE: usize = 14;

/// Sample layout of a device's blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Every sample a full-scale big-endian `i32`.
    V2,
    /// Digitakt II and Digitone II: 24-bit audio. From the device the first [`V2_1_WIDE`]
    /// channels are 4-byte (a 24-bit value sign-extended into a big-endian `i32`) and the rest
    /// packed 3-byte big-endian; to the device every channel is 4-byte, sign-extended.
    V2_1,
}

/// A listed Overbridge device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub pid: u16,
    pub name: &'static str,
    pub format: Format,
    /// Channels the device sends.
    pub ch_from: u8,
    /// Channels the device receives.
    pub ch_to: u8,
}

const fn dev(pid: u16, name: &'static str, ch_from: u8, ch_to: u8, format: Format) -> Device {
    Device { pid, name, format, ch_from, ch_to }
}

/// The known devices (Overwitch's `devices.json`, cross-checked against the Outbox 8's table).
/// An unlisted Elektron device is still driven as v2 when its block sizes give whole channels.
pub const DEVICES: &[Device] = &[
    dev(0x0B2C, "Digitakt", 12, 2, Format::V2),
    dev(0x0B36, "Digitone", 12, 2, Format::V2),
    dev(0x0B35, "Digitone Keys", 12, 2, Format::V2),
    dev(0x0B4A, "Syntakt", 20, 8, Format::V2),
    dev(0x0B47, "Analog Four MKII", 8, 6, Format::V2),
    dev(0x0B48, "Analog Rytm MKII", 12, 12, Format::V2),
    dev(0x0B50, "Analog Heat", 4, 4, Format::V2),
    dev(0x0B52, "Analog Heat MKII", 4, 4, Format::V2),
    dev(0x0B53, "Analog Heat +FX", 4, 4, Format::V2),
    dev(0x0B2B, "Digitakt II", 42, 8, Format::V2_1),
    dev(0x0B34, "Digitone II", 42, 8, Format::V2_1),
];

/// The listed device with product ID `pid`.
pub fn lookup(pid: u16) -> Option<&'static Device> {
    DEVICES.iter().find(|d| d.pid == pid)
}

impl Format {
    /// Bytes per frame sent by the device, for `ch` channels.
    pub const fn from_device_frame_bytes(self, ch: usize) -> usize {
        match self {
            Format::V2_1 if ch > V2_1_WIDE => 4 * V2_1_WIDE + 3 * (ch - V2_1_WIDE),
            _ => 4 * ch,
        }
    }

    /// Bytes per frame sent to the device, for `ch` channels.
    pub const fn to_device_frame_bytes(self, ch: usize) -> usize {
        4 * ch
    }

    /// Channels in a device-to-host block of `block_len` bytes, or `None` when it isn't a
    /// whole, non-zero number.
    pub fn channels_from_device(self, block_len: usize) -> Option<usize> {
        let fb = frame_bytes(block_len)?;
        match self {
            Format::V2_1 if fb > 4 * V2_1_WIDE => {
                let rest = fb - 4 * V2_1_WIDE;
                (rest % 3 == 0).then_some(V2_1_WIDE + rest / 3)
            }
            _ => (fb % 4 == 0).then_some(fb / 4),
        }
    }

    /// Channels in a host-to-device block of `block_len` bytes, or `None` when it isn't a
    /// whole, non-zero number.
    pub fn channels_to_device(self, block_len: usize) -> Option<usize> {
        let fb = frame_bytes(block_len)?;
        (fb % 4 == 0).then_some(fb / 4)
    }

    /// Bytes of channel `c`'s sample in a block going the given way.
    const fn width(self, c: usize, from_device: bool) -> usize {
        match self {
            Format::V2_1 if from_device && c >= V2_1_WIDE => 3,
            _ => 4,
        }
    }

    const fn frame_bytes(self, ch: usize, from_device: bool) -> usize {
        if from_device { self.from_device_frame_bytes(ch) } else { self.to_device_frame_bytes(ch) }
    }
}

/// Length of a block whose frames are `frame_bytes` long.
pub const fn block_len(frame_bytes: usize) -> usize {
    HEADER_LEN + FRAMES_PER_BLOCK * frame_bytes
}

/// Frame length in a block of `block_len` bytes, if it holds a whole, non-empty 7 frames.
fn frame_bytes(block_len: usize) -> Option<usize> {
    let samples = block_len.checked_sub(HEADER_LEN)?;
    (samples > 0 && samples % FRAMES_PER_BLOCK == 0).then_some(samples / FRAMES_PER_BLOCK)
}

/// Why a received block was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockError {
    /// Not the length the channel count and format give.
    Length,
    /// The header word isn't the one this direction uses.
    Header,
}

fn decode_sample(format: Format, b: &[u8]) -> i32 {
    match (format, b.len()) {
        (Format::V2, _) => i32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        (Format::V2_1, 4) => i32::from_be_bytes([b[0], b[1], b[2], b[3]]) << 8,
        (Format::V2_1, _) => i32::from_be_bytes([b[0], b[1], b[2], 0]),
    }
}

fn encode_sample(format: Format, x: i32, out: &mut [u8]) {
    match (format, out.len()) {
        (Format::V2, _) => out.copy_from_slice(&x.to_be_bytes()),
        (Format::V2_1, 4) => out.copy_from_slice(&(x >> 8).to_be_bytes()),
        (Format::V2_1, _) => out.copy_from_slice(&x.to_be_bytes()[..3]),
    }
}

fn decode(format: Format, ch: usize, from_device: bool, block: &[u8], frames: &mut [i32]) -> Result<u16, BlockError> {
    if block.len() != block_len(format.frame_bytes(ch, from_device)) {
        return Err(BlockError::Length);
    }
    let header = if from_device { HEADER_FROM_DEVICE } else { HEADER_TO_DEVICE };
    if u16::from_be_bytes([block[0], block[1]]) != header {
        return Err(BlockError::Header);
    }
    let mut at = HEADER_LEN;
    for (i, out) in frames[..FRAMES_PER_BLOCK * ch].iter_mut().enumerate() {
        let w = format.width(i % ch, from_device);
        *out = decode_sample(format, &block[at..at + w]);
        at += w;
    }
    Ok(u16::from_be_bytes([block[2], block[3]]))
}

fn encode(format: Format, ch: usize, from_device: bool, counter: u16, frames: &[i32], block: &mut [u8]) -> usize {
    let len = block_len(format.frame_bytes(ch, from_device));
    let block = &mut block[..len];
    let header = if from_device { HEADER_FROM_DEVICE } else { HEADER_TO_DEVICE };
    block[..2].copy_from_slice(&header.to_be_bytes());
    block[2..4].copy_from_slice(&counter.to_be_bytes());
    block[4..HEADER_LEN].fill(0);
    let mut at = HEADER_LEN;
    for (i, &x) in frames[..FRAMES_PER_BLOCK * ch].iter().enumerate() {
        let w = format.width(i % ch, from_device);
        encode_sample(format, x, &mut block[at..at + w]);
        at += w;
    }
    len
}

/// Decode a device-to-host block into 7 × `ch` interleaved full-scale frames. Returns the
/// block's frame counter.
pub fn decode_from_device(format: Format, ch: usize, block: &[u8], frames: &mut [i32]) -> Result<u16, BlockError> {
    decode(format, ch, true, block, frames)
}

/// Encode 7 × `ch` interleaved frames as a host-to-device block. Returns its length.
pub fn encode_to_device(format: Format, ch: usize, counter: u16, frames: &[i32], block: &mut [u8]) -> usize {
    encode(format, ch, false, counter, frames, block)
}

/// Device side: encode 7 × `ch` interleaved frames as a device-to-host block. Returns its length.
pub fn encode_from_device(format: Format, ch: usize, counter: u16, frames: &[i32], block: &mut [u8]) -> usize {
    encode(format, ch, true, counter, frames, block)
}

/// Device side: decode a host-to-device block. Returns its frame counter.
pub fn decode_to_device(format: Format, ch: usize, block: &[u8], frames: &mut [i32]) -> Result<u16, BlockError> {
    decode(format, ch, false, block, frames)
}

/// Follows a stream's frame counter (+7 per block, 16-bit wrapping) and counts missing blocks.
#[derive(Clone, Copy, Debug, Default)]
pub struct CounterTracker {
    next: Option<u16>,
}

impl CounterTracker {
    /// A jump of more blocks than this (≈150 ms) is a restarted counter, not a gap.
    const MAX_GAP: u32 = 1024;

    /// Record a block's counter. Returns how many blocks were missing before it. A counter
    /// that isn't on the expected 7-frame grid, or jumps implausibly far, resyncs and counts 0.
    pub fn observe(&mut self, counter: u16) -> u32 {
        let missing = match self.next {
            Some(n) => {
                let d = counter.wrapping_sub(n) as u32;
                let blocks = d / FRAMES_PER_BLOCK as u32;
                if d % FRAMES_PER_BLOCK as u32 == 0 && blocks <= Self::MAX_GAP { blocks } else { 0 }
            }
            None => 0,
        };
        self.next = Some(counter.wrapping_add(FRAMES_PER_BLOCK as u16));
        missing
    }

    /// Forget the stream, so the next block sets the expectation.
    pub fn reset(&mut self) {
        self.next = None;
    }
}
```

Note the wrap case: 0xFFFA + 7 wraps to 0x0001, so `wrapping_sub` gives 0.

- [ ] **Step 5: Run the tests to verify they pass**

Run the Step 3 command with `| tail -3` instead of the grep.
Expected: 11 passed.

- [ ] **Step 6: Commit**

```bash
git add usb/usb-classes/Cargo.toml usb/usb-classes/src/lib.rs usb/usb-classes/src/overbridge tools/test.sh
git commit -m "usb-classes: overbridge::proto (device table, v2/v2.1 block codecs, frame counter)"
```

---

### Task 2: `overbridge::stems`, the app-facing handle

**Files:**
- Create: `usb/usb-classes/src/overbridge/stems.rs`
- Modify: `usb/usb-classes/src/overbridge/mod.rs` (`pub mod stems;`)

**Interfaces:**
- Consumes: `proto::{Format, FRAMES_PER_BLOCK}`.
- Produces (`usb_classes::overbridge::stems`):
  - `pub struct OverbridgeInfo { pub pid: u16, pub name: &'static str, pub ch_from: u8, pub ch_to: u8, pub format: Format }`.
  - `pub struct StreamStats { fill_in, fill_out, blocks_in, blocks_out, counter_gaps, framing_errors, overruns, underruns: u32, rate_ratio_ppm: i32 }`
    ("in" is what arrives at this side; for the host, from the device).
  - `pub struct TooManyChannels;` and `pub const MIN_RING_FRAMES: usize = 56`.
  - `pub struct OverbridgeHandle<const RX: usize, const TX: usize>` with:
    - app side: `const fn new()`, `info()`, `read(&mut [i32]) -> usize`,
      `write(&[i32]) -> usize`, `stats()`, `connection() -> u32`;
    - driver side: `begin(info, rx_ch, tx_ch) -> Result<(), TooManyChannels>`, `end()`,
      `push_rx(&[i32])`, `pull_tx(&mut [i32])`, `update_stats(impl FnOnce(&mut StreamStats))`.

- [ ] **Step 1: Write the failing tests**

Create `stems.rs` with only:

```rust
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::overbridge::proto::Format;

    fn info(ch_from: u8, ch_to: u8) -> OverbridgeInfo {
        OverbridgeInfo { pid: 0x0B2C, name: "Digitakt", ch_from, ch_to, format: Format::V2 }
    }

    #[test]
    fn nothing_is_connected_until_begin() {
        let h: OverbridgeHandle<1024, 256> = OverbridgeHandle::new();
        assert_eq!(h.info(), None);
        assert_eq!(h.read(&mut [0; 24]), 0);
        assert_eq!(h.write(&[1, 2]), 0, "nothing to write to");
        h.begin(info(12, 2), 12, 2).unwrap();
        assert_eq!(h.info(), Some(info(12, 2)));
        assert_eq!(h.connection(), 1);
        h.end();
        assert_eq!(h.info(), None);
    }

    #[test]
    fn received_frames_are_read_back_in_order() {
        let h: OverbridgeHandle<1200, 256> = OverbridgeHandle::new();
        h.begin(info(12, 2), 12, 2).unwrap();
        let block: std::vec::Vec<i32> = (0..7 * 12).collect();
        h.push_rx(&block);
        let mut out = [0i32; 5 * 12];
        assert_eq!(h.read(&mut out), 5, "frames, not samples");
        assert_eq!(&out[..], &block[..5 * 12]);
        assert_eq!(h.stats().fill_in, 2);
        let mut rest = [0i32; 7 * 12];
        assert_eq!(h.read(&mut rest), 2);
        assert_eq!(&rest[..2 * 12], &block[5 * 12..]);
    }

    #[test]
    fn an_overrun_drops_the_oldest_frames_and_counts_them() {
        // 120 samples hold 10 frames of 12 channels... too few for begin; use 2 channels.
        let h: OverbridgeHandle<128, 128> = OverbridgeHandle::new();
        h.begin(info(2, 2), 2, 2).unwrap(); // 64 frames of 2 channels
        for b in 0..10 {
            let block: std::vec::Vec<i32> = (0..14).map(|i| b * 100 + i).collect();
            h.push_rx(&block);
        }
        let s = h.stats();
        assert_eq!(s.overruns, 70 - 64);
        assert_eq!(s.fill_in, 64);
        let mut out = [0i32; 2];
        h.read(&mut out);
        assert_eq!(out, [12, 13], "frames 0-5 of block 0 were dropped");
    }

    #[test]
    fn the_tx_side_pads_silence_and_counts_underruns() {
        let h: OverbridgeHandle<1024, 256> = OverbridgeHandle::new();
        h.begin(info(12, 2), 12, 2).unwrap();
        assert_eq!(h.write(&[1, 2, 3, 4, 5, 6]), 3);
        let mut block = [9i32; 14];
        h.pull_tx(&mut block);
        assert_eq!(&block[..6], &[1, 2, 3, 4, 5, 6]);
        assert!(block[6..].iter().all(|&s| s == 0));
        assert_eq!(h.stats().underruns, 4);
    }

    #[test]
    fn a_handle_too_small_is_refused() {
        // 42 channels × 56 frames = 2352 samples; 2048 is too few.
        let h: OverbridgeHandle<2048, 512> = OverbridgeHandle::new();
        assert_eq!(h.begin(info(42, 8), 42, 8), Err(TooManyChannels));
        assert_eq!(h.info(), None, "a refused device is not connected");
    }

    #[test]
    fn begin_resets_rings_and_stats_and_counts_connections() {
        let h: OverbridgeHandle<1024, 256> = OverbridgeHandle::new();
        h.begin(info(2, 2), 2, 2).unwrap();
        h.push_rx(&[1; 14]);
        h.update_stats(|s| s.counter_gaps = 5);
        h.begin(info(2, 2), 2, 2).unwrap();
        assert_eq!(h.stats(), StreamStats::default());
        assert_eq!(h.read(&mut [0; 2]), 0);
        assert_eq!(h.connection(), 2);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Add `pub mod stems;` to `overbridge/mod.rs`, then run Task 1's Step 3 command.
Expected: compile errors (`OverbridgeHandle` etc. not found).

- [ ] **Step 3: Implement**

Put above the tests in `stems.rs`:

```rust
//! The app-facing side of an Overbridge stream: a pair of interleaved full-scale `i32` rings
//! behind one lock, the device's identity, and the stream's statistics.
//!
//! The stream task pushes what it receives ([`OverbridgeHandle::push_rx`]) and pulls what it
//! sends ([`OverbridgeHandle::pull_tx`]); the app [`read`](OverbridgeHandle::read)s and
//! [`write`](OverbridgeHandle::write)s. Neither side blocks: an overrun drops the oldest frames
//! and an underrun sends silence, each counted in [`StreamStats`].

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use super::proto::Format;

/// The connected device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverbridgeInfo {
    pub pid: u16,
    pub name: &'static str,
    /// Channels the Overbridge device sends.
    pub ch_from: u8,
    /// Channels the Overbridge device receives.
    pub ch_to: u8,
    pub format: Format,
}

/// Stream counters. "In" is what arrives at this side (for a host, from the device).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Frames waiting for the app to read.
    pub fill_in: u32,
    /// Frames the app has written and the stream hasn't sent yet.
    pub fill_out: u32,
    pub blocks_in: u32,
    pub blocks_out: u32,
    /// Blocks missing by the +7 frame counter.
    pub counter_gaps: u32,
    /// Blocks dropped for a wrong header or length.
    pub framing_errors: u32,
    /// Received frames dropped because the app didn't read them in time.
    pub overruns: u32,
    /// Sent frames padded with silence because the app hadn't written them.
    pub underruns: u32,
    /// The device's frame rate against local time, in parts per million off nominal.
    pub rate_ratio_ppm: i32,
}

/// The device has more channels than this handle's rings hold [`MIN_RING_FRAMES`] of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyChannels;

/// The fewest frames a ring must hold for a device to be accepted (8 blocks).
pub const MIN_RING_FRAMES: usize = 56;

/// Interleaved ring of `N` samples, holding `N / channels` frames.
struct Ring<const N: usize> {
    buf: [i32; N],
    head: usize,
    frames: usize,
    channels: usize,
}

impl<const N: usize> Ring<N> {
    const fn new() -> Self {
        Self { buf: [0; N], head: 0, frames: 0, channels: 0 }
    }

    fn capacity(&self) -> usize {
        if self.channels == 0 { 0 } else { N / self.channels }
    }

    fn reset(&mut self, channels: usize) {
        self.head = 0;
        self.frames = 0;
        self.channels = channels;
    }

    /// Append whole frames; returns how many of the oldest were dropped to make room.
    fn push(&mut self, samples: &[i32]) -> usize {
        let (ch, cap) = (self.channels, self.capacity());
        if cap == 0 {
            return 0;
        }
        let mut dropped = 0;
        for frame in samples.chunks_exact(ch) {
            if self.frames == cap {
                self.head = (self.head + 1) % cap;
                self.frames -= 1;
                dropped += 1;
            }
            let at = (self.head + self.frames) % cap * ch;
            self.buf[at..at + ch].copy_from_slice(frame);
            self.frames += 1;
        }
        dropped
    }

    /// Append whole frames while there is room; returns how many were taken.
    fn push_some(&mut self, samples: &[i32]) -> usize {
        let room = self.capacity() - self.frames;
        let take = (samples.len() / self.channels.max(1)).min(room);
        self.push(&samples[..take * self.channels]);
        take
    }

    /// Move up to `out.len() / channels` frames out; returns how many.
    fn pop(&mut self, out: &mut [i32]) -> usize {
        let (ch, cap) = (self.channels, self.capacity());
        if cap == 0 {
            return 0;
        }
        let n = (out.len() / ch).min(self.frames);
        for f in 0..n {
            let at = (self.head + f) % cap * ch;
            out[f * ch..(f + 1) * ch].copy_from_slice(&self.buf[at..at + ch]);
        }
        self.head = (self.head + n) % cap;
        self.frames -= n;
        n
    }
}

struct Inner<const RX: usize, const TX: usize> {
    info: Option<OverbridgeInfo>,
    rx: Ring<RX>,
    tx: Ring<TX>,
    stats: StreamStats,
    connection: u32,
}

/// One Overbridge stream, shared by its stream task and the app. `RX` and `TX` are the ring
/// sizes in samples; a ring holds `RX / channels` frames.
pub struct OverbridgeHandle<const RX: usize, const TX: usize> {
    inner: Mutex<CriticalSectionRawMutex, RefCell<Inner<RX, TX>>>,
}

impl<const RX: usize, const TX: usize> OverbridgeHandle<RX, TX> {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(Inner {
                info: None,
                rx: Ring::new(),
                tx: Ring::new(),
                stats: StreamStats { fill_in: 0, fill_out: 0, blocks_in: 0, blocks_out: 0, counter_gaps: 0, framing_errors: 0, overruns: 0, underruns: 0, rate_ratio_ppm: 0 },
                connection: 0,
            })),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner<RX, TX>) -> R) -> R {
        self.inner.lock(|i| f(&mut i.borrow_mut()))
    }

    /// The connected device, or `None`.
    pub fn info(&self) -> Option<OverbridgeInfo> {
        self.with(|i| i.info)
    }

    /// Copy up to `out.len() / ch_from` received frames, interleaved. Returns frames copied.
    pub fn read(&self, out: &mut [i32]) -> usize {
        self.with(|i| i.rx.pop(out))
    }

    /// Queue frames to send (interleaved, `ch_to` wide). Returns frames taken; the rest didn't
    /// fit, or nothing is connected.
    pub fn write(&self, inp: &[i32]) -> usize {
        self.with(|i| if i.info.is_some() { i.tx.push_some(inp) } else { 0 })
    }

    /// The stream's counters, with current fill levels.
    pub fn stats(&self) -> StreamStats {
        self.with(|i| StreamStats { fill_in: i.rx.frames as u32, fill_out: i.tx.frames as u32, ..i.stats })
    }

    /// Counts connections, so an app can tell the device (or driver) changed between reads.
    pub fn connection(&self) -> u32 {
        self.with(|i| i.connection)
    }

    /// Driver side: a device is connected, with `rx_ch` channels arriving and `tx_ch` sent.
    pub fn begin(&self, info: OverbridgeInfo, rx_ch: usize, tx_ch: usize) -> Result<(), TooManyChannels> {
        if rx_ch == 0 || tx_ch == 0 || RX / rx_ch < MIN_RING_FRAMES || TX / tx_ch < MIN_RING_FRAMES {
            return Err(TooManyChannels);
        }
        self.with(|i| {
            i.rx.reset(rx_ch);
            i.tx.reset(tx_ch);
            i.stats = StreamStats::default();
            i.info = Some(info);
            i.connection = i.connection.wrapping_add(1);
        });
        Ok(())
    }

    /// Driver side: the device is gone.
    pub fn end(&self) {
        self.with(|i| {
            i.info = None;
            i.rx.reset(0);
            i.tx.reset(0);
        });
    }

    /// Driver side: frames received (interleaved, `rx_ch` wide).
    pub fn push_rx(&self, frames: &[i32]) {
        self.with(|i| {
            let dropped = i.rx.push(frames) as u32;
            i.stats.overruns = i.stats.overruns.saturating_add(dropped);
        });
    }

    /// Driver side: fill `frames` (interleaved, `tx_ch` wide) with what to send, padding with
    /// silence when the app hasn't written enough.
    pub fn pull_tx(&self, frames: &mut [i32]) {
        self.with(|i| {
            let ch = i.tx.channels.max(1);
            let got = i.tx.pop(frames);
            frames[got * ch..].fill(0);
            let short = (frames.len() / ch - got) as u32;
            i.stats.underruns = i.stats.underruns.saturating_add(short);
        });
    }

    /// Driver side: update the counters.
    pub fn update_stats(&self, f: impl FnOnce(&mut StreamStats)) {
        self.with(|i| f(&mut i.stats));
    }
}

impl<const RX: usize, const TX: usize> Default for OverbridgeHandle<RX, TX> {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run Task 1's Step 3 command with `| tail -3`.
Expected: all overbridge tests pass (11 + 6).

- [ ] **Step 5: Commit**

```bash
git add usb/usb-classes/src/overbridge
git commit -m "usb-classes: overbridge::stems, the app-facing stem rings, info and stats"
```

---

### Task 3: `host::overbridge`, the host class

**Files:**
- Create: `usb/usb-classes/src/host/overbridge.rs`
- Modify: `usb/usb-classes/src/host/mod.rs` (`#[cfg(feature = "host-overbridge")] pub mod overbridge;`)
- Modify: `usb/usb-classes/src/host/mock.rs` (larger scripted reads and the OUT log)

**Interfaces:**
- Consumes: `proto::*`, `stems::{OverbridgeHandle, OverbridgeInfo}`.
- Produces (`usb_classes::host::overbridge`):
  - `pub struct OverbridgeMatch { pub pid: u16, pub name: &'static str, pub format: Format, pub ch_from: usize, pub ch_to: usize, pub ep_in: EndpointDescriptor, pub ep_out: EndpointDescriptor }`
    with `fn info(&self) -> OverbridgeInfo`.
  - `pub fn match_device(vid: u16, pid: u16, cfg: &ConfigurationDescriptor<'_>) -> Result<OverbridgeMatch, OverbridgeError>`.
  - `pub enum OverbridgeError { NotOverbridge, BadBlockSize, TooManyChannels, NoPipe(HostError), SetupFailed(PipeError), Transport(PipeError), Framing }`.
  - `pub struct OverbridgeHost<'d, A>` with:
    - `async fn try_register(alloc: &A, addr: u8, split: Option<SplitInfo>, vid: u16, pid: u16, cfg: &ConfigurationDescriptor<'_>) -> Result<Self, OverbridgeError>`;
    - `fn matched(&self) -> &OverbridgeMatch`;
    - `async fn pump_once<const RX: usize, const TX: usize>(&mut self, h: &OverbridgeHandle<RX, TX>, now_us: u64) -> Result<(), OverbridgeError>`;
    - `async fn restart(&mut self) -> Result<(), OverbridgeError>`.

- [ ] **Step 1: Widen the mock**

In `host/mock.rs`:
- change `pub reads: Vec<Vec<u8, 64>, 8>,` to `pub reads: Vec<Vec<u8, 1024>, 80>,`;
- change `pub sent: Vec<u8, 256>,` to `pub sent: Vec<u8, 16384>,`.

Then run the usb-classes suite (Task 1 Step 3 command, `| tail -3`).
Expected: still passes. Existing tests build `Vec::from_slice` values whose capacity is
inferred from the field. If any test names `Vec<u8, 64>` explicitly, change it to 1024.

- [ ] **Step 2: Write the failing tests**

Create `host/overbridge.rs` with only:

```rust
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    extern crate std;
    use super::*;
    use crate::host::mock::{MockAlloc, MockState};
    use crate::overbridge::stems::OverbridgeHandle;
    use embassy_futures::block_on;
    use std::vec::Vec;

    /// The Digitakt's Overbridge configuration descriptor (OS 1.54; spec Appendix A).
    const DIGITAKT_CONFIG: [u8; 263] = [
        0x09, 0x02, 0x07, 0x01, 0x06, 0x01, 0x00, 0xc0, 0x00, 0x08, 0x0b, 0x00, 0x04, 0xff, 0x00, 0x00,
        0x00, 0x09, 0x04, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x09, 0x04, 0x01, 0x00, 0x00, 0xff,
        0x00, 0x00, 0x00, 0x09, 0x04, 0x01, 0x01, 0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x83, 0x03,
        0x20, 0x00, 0x01, 0x09, 0x04, 0x01, 0x02, 0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x83, 0x03,
        0x58, 0x00, 0x01, 0x09, 0x04, 0x01, 0x03, 0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x83, 0x03,
        0x70, 0x01, 0x01, 0x09, 0x04, 0x02, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x09, 0x04, 0x02, 0x01,
        0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x03, 0x03, 0x20, 0x00, 0x01, 0x09, 0x04, 0x02, 0x02,
        0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x03, 0x03, 0x58, 0x00, 0x01, 0x09, 0x04, 0x02, 0x03,
        0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x03, 0x03, 0x58, 0x00, 0x01, 0x09, 0x04, 0x03, 0x00,
        0x02, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x02, 0x02, 0x00, 0x02, 0x00, 0x07, 0x05, 0x82, 0x03,
        0x40, 0x00, 0x01, 0x08, 0x0b, 0x04, 0x02, 0x01, 0x01, 0x00, 0x00, 0x09, 0x04, 0x04, 0x00, 0x00,
        0x01, 0x01, 0x00, 0x00, 0x09, 0x24, 0x01, 0x00, 0x01, 0x09, 0x00, 0x01, 0x05, 0x09, 0x04, 0x05,
        0x00, 0x02, 0x01, 0x03, 0x00, 0x00, 0x07, 0x24, 0x01, 0x00, 0x01, 0x41, 0x00, 0x06, 0x24, 0x02,
        0x01, 0x01, 0x00, 0x06, 0x24, 0x02, 0x02, 0x02, 0x00, 0x09, 0x24, 0x03, 0x01, 0x03, 0x01, 0x02,
        0x01, 0x00, 0x09, 0x24, 0x03, 0x02, 0x04, 0x01, 0x01, 0x01, 0x00, 0x09, 0x05, 0x01, 0x02, 0x00,
        0x02, 0x00, 0x00, 0x00, 0x05, 0x25, 0x01, 0x01, 0x01, 0x09, 0x05, 0x81, 0x02, 0x00, 0x02, 0x00,
        0x00, 0x00, 0x05, 0x25, 0x01, 0x01, 0x03,
    ];

    fn cfg(bytes: &[u8]) -> ConfigurationDescriptor<'_> {
        ConfigurationDescriptor::try_from_slice(bytes).unwrap()
    }

    /// A config with only interfaces 1 and 2, alt 3 interrupt endpoints of `in_mps`/`out_mps`.
    fn stream_config(in_mps: u16, out_mps: u16) -> Vec<u8> {
        let mut b = std::vec![0x09, 0x02, 0, 0, 2, 1, 0, 0xc0, 0];
        for (iface, ep, mps) in [(1u8, 0x83u8, in_mps), (2, 0x03, out_mps)] {
            b.extend_from_slice(&[9, 4, iface, 0, 0, 0xff, 0, 0, 0]);
            b.extend_from_slice(&[9, 4, iface, 3, 1, 0xff, 0, 0, 0]);
            b.extend_from_slice(&[7, 5, ep, 0x03, mps as u8, (mps >> 8) as u8, 1]);
        }
        let len = b.len() as u16;
        b[2..4].copy_from_slice(&len.to_le_bytes());
        b
    }

    /// A Digitakt from-device block: counter `counter`, sample (f, c) = (counter + f) * 16 + c.
    fn digitakt_block(counter: u16) -> heapless::Vec<u8, 1024> {
        let mut frames = [0i32; 7 * 12];
        for (i, s) in frames.iter_mut().enumerate() {
            *s = ((counter as i32 + (i / 12) as i32) * 16 + (i % 12) as i32) << 8;
        }
        let mut block = [0u8; 368];
        proto::encode_from_device(Format::V2, 12, counter, &frames, &mut block);
        heapless::Vec::from_slice(&block).unwrap()
    }

    fn digitakt(state: &'static MockState) -> OverbridgeHost<'static, MockAlloc> {
        let alloc = MockAlloc::new(state);
        block_on(OverbridgeHost::try_register(&alloc, 1, None, 0x1935, 0x0B2C, &cfg(&DIGITAKT_CONFIG))).unwrap()
    }

    #[test]
    fn the_digitakt_matches_with_12_and_2_channels() {
        let m = match_device(0x1935, 0x0B2C, &cfg(&DIGITAKT_CONFIG)).unwrap();
        assert_eq!((m.name, m.format, m.ch_from, m.ch_to), ("Digitakt", Format::V2, 12, 2));
        assert_eq!((m.ep_in.endpoint_address, m.ep_in.max_packet_size), (0x83, 368));
        assert_eq!((m.ep_out.endpoint_address, m.ep_out.max_packet_size), (0x03, 88));
    }

    #[test]
    fn another_vendor_does_not_match() {
        assert!(matches!(match_device(0x1234, 0x0B2C, &cfg(&DIGITAKT_CONFIG)), Err(OverbridgeError::NotOverbridge)));
    }

    #[test]
    fn an_elektron_device_without_alt_3_does_not_match() {
        // Audio/MIDI mode: only a MIDIStreaming interface.
        let b = [0x09, 0x02, 25, 0, 1, 1, 0, 0xc0, 0, 9, 4, 0, 0, 1, 0x01, 0x03, 0, 0, 7, 5, 0x81, 0x02, 0, 2, 0];
        assert!(matches!(match_device(0x1935, 0x102C, &cfg(&b)), Err(OverbridgeError::NotOverbridge)));
    }

    #[test]
    fn a_digitakt_ii_matches_as_v2_1_with_42_and_8_channels() {
        let b = stream_config(1012, 256);
        let m = match_device(0x1935, 0x0B2B, &cfg(&b)).unwrap();
        assert_eq!((m.name, m.format, m.ch_from, m.ch_to), ("Digitakt II", Format::V2_1, 42, 8));
    }

    #[test]
    fn an_unlisted_elektron_device_is_driven_as_v2_from_its_sizes() {
        let b = stream_config(32 + 7 * 16, 32 + 7 * 8);
        let m = match_device(0x1935, 0x0BEE, &cfg(&b)).unwrap();
        assert_eq!((m.format, m.ch_from, m.ch_to), (Format::V2, 4, 2));
    }

    #[test]
    fn a_non_integral_block_size_is_refused() {
        let b = stream_config(369, 88);
        assert!(matches!(match_device(0x1935, 0x0B2C, &cfg(&b)), Err(OverbridgeError::BadBlockSize)));
    }

    #[test]
    fn setup_selects_alt_3_on_both_interfaces_then_opens_pipes_and_primes_out() {
        let state = MockState::leak();
        let _h = digitakt(state);
        let setups = state.setups.borrow();
        assert_eq!(&setups[..], &[[0x01, 0x0B, 3, 0, 1, 0, 0, 0], [0x01, 0x0B, 3, 0, 2, 0, 0, 0]]);
        let allocs = state.allocs.borrow();
        let data: Vec<_> = allocs.iter().filter(|a| a.ep_addr != 0).map(|a| (a.ep_addr, a.max_packet_size, a.setups_before)).collect();
        assert_eq!(data, [(0x83, 368, 2), (0x03, 88, 2)], "pipes open after both alternates are selected");
        let sent = state.log.borrow();
        assert_eq!(sent.sent.len(), 2 * 88, "two silent blocks prime the OUT stream");
        assert_eq!(&sent.sent[..4], &[0x07, 0xFF, 0x00, 0x00]);
        assert_eq!(&sent.sent[88..92], &[0x07, 0xFF, 0x00, 0x07]);
        assert!(sent.sent[32..88].iter().all(|&b| b == 0));
    }

    #[test]
    fn streamed_blocks_reach_the_handle_in_order_and_out_follows_in() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        for c in [0u16, 7, 14] {
            state.script.borrow_mut().reads.push(digitakt_block(c)).unwrap();
        }
        h.write(&[0x0100_0000, -0x0100_0000]);
        for t in 0..3 {
            block_on(host.pump_once(&h, t * 146)).unwrap();
        }
        let mut out = [0i32; 21 * 12];
        assert_eq!(h.read(&mut out), 21);
        for f in 0..21 {
            for c in 0..12 {
                assert_eq!(out[f * 12 + c], ((f as i32 * 16) + c as i32) << 8, "frame {f} channel {c}");
            }
        }
        let s = h.stats();
        assert_eq!((s.blocks_in, s.blocks_out, s.counter_gaps, s.framing_errors), (3, 3, 0, 0));
        let sent = &state.log.borrow().sent;
        assert_eq!(sent.len(), 5 * 88, "2 priming + 3 paced blocks");
        let first = &sent[2 * 88..3 * 88];
        assert_eq!(&first[..4], &[0x07, 0xFF, 0x00, 0x0E], "counter continues after priming");
        assert_eq!(&first[32..40], &[0x01, 0, 0, 0, 0xFF, 0, 0, 0], "the app's frame goes out first");
    }

    #[test]
    fn a_counter_gap_is_counted() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        for c in [0u16, 7, 28] {
            state.script.borrow_mut().reads.push(digitakt_block(c)).unwrap();
        }
        for t in 0..3 {
            block_on(host.pump_once(&h, t)).unwrap();
        }
        assert_eq!(h.stats().counter_gaps, 2);
    }

    #[test]
    fn a_short_block_is_a_framing_error_and_out_still_runs() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        state.script.borrow_mut().reads.push(heapless::Vec::from_slice(&[0x07, 0x00, 0, 0]).unwrap()).unwrap();
        // An empty script makes the mock return a zero-length read.
        block_on(host.pump_once(&h, 0)).unwrap();
        block_on(host.pump_once(&h, 1)).unwrap();
        let s = h.stats();
        assert_eq!((s.blocks_in, s.framing_errors, s.blocks_out), (0, 2, 2));
        assert_eq!(state.log.borrow().sent.len(), 4 * 88);
    }

    #[test]
    fn sixty_four_framing_errors_in_a_row_are_a_transport_error() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        for t in 0..63 {
            block_on(host.pump_once(&h, t)).unwrap();
        }
        assert!(matches!(block_on(host.pump_once(&h, 63)), Err(OverbridgeError::Framing)));
    }

    #[test]
    fn a_pipe_error_is_a_transport_error() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        state.script.borrow_mut().read_err = Some(PipeError::BadResponse);
        assert!(matches!(block_on(host.pump_once(&h, 0)), Err(OverbridgeError::Transport(PipeError::BadResponse))));
    }

    #[test]
    fn the_rate_ratio_is_measured_over_a_second() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<65536, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        // 6858 blocks = 48 006 frames in 1 000 000 µs: +125 ppm.
        let blocks = 6858u64;
        for b in 0..=blocks {
            if state.script.borrow().reads.is_empty() {
                for k in 0..64u16 {
                    state.script.borrow_mut().reads.push(digitakt_block(((b as u16).wrapping_add(k)).wrapping_mul(7))).unwrap();
                }
            }
            block_on(host.pump_once(&h, b * 1_000_000 / blocks)).unwrap();
            let mut sink = [0i32; 7 * 12];
            h.read(&mut sink);
            state.log.borrow_mut().sent.clear();
        }
        assert_eq!(h.stats().rate_ratio_ppm, 125);
    }

    #[test]
    fn restart_selects_the_alternates_again_and_reprimes() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        block_on(host.restart()).unwrap();
        assert_eq!(state.setups.borrow().len(), 4);
        assert_eq!(state.log.borrow().sent.len(), 4 * 88);
        assert_eq!(state.log.borrow().toggle_resets, 2);
    }
}
```

Note on the rate test: 6858 blocks of 7 frames is 48 006 frames, and (48 006 / 48 000 − 1) × 10⁶
= 125 ppm. The script is refilled 64 blocks at a time; the counters run in steps of 7 because
`k` advances one block per pump.

- [ ] **Step 3: Run them to verify they fail**

Add `#[cfg(feature = "host-overbridge")] pub mod overbridge;` to `host/mod.rs`, then run Task 1's
Step 3 command with `--lib host::overbridge`.
Expected: compile errors (`match_device`, `OverbridgeHost`... not found).

- [ ] **Step 4: Implement**

Put above the tests in `host/overbridge.rs`:

```rust
//! USB **host** class for Elektron Overbridge 2 devices (Digitakt, Digitone, Syntakt, Analog
//! Four/Rytm/Heat MKII, Digitakt II, Digitone II).
//!
//! Setup selects alternate setting 3 on interfaces 1 (audio from the device) and 2 (audio to
//! it), then streams: each [`OverbridgeHost::pump_once`] reads one block, decodes it into an
//! [`OverbridgeHandle`], and sends one block back, so the device's clock paces both
//! directions. The endpoints are high-speed interrupt endpoints; the RZ/A1L HAL carries those
//! over 64 bytes on bulk pipes.

use embassy_usb_driver::host::{HostError, PipeError, SplitInfo, UsbHostAllocator, UsbPipe, pipe};
use embassy_usb_driver::{Direction, EndpointType};
use embassy_usb_host::control::{ControlType, Recipient, RequestType, SetupPacket};
use embassy_usb_host::descriptor::{ConfigurationDescriptor, EndpointDescriptor};

use crate::overbridge::proto::{self, CounterTracker, Format, FRAMES_PER_BLOCK, MAX_BLOCK, MAX_CHANNELS};
use crate::overbridge::stems::{OverbridgeHandle, OverbridgeInfo};

const REQ_SET_INTERFACE: u8 = 0x0B;
/// Consecutive bad blocks that end the stream.
const FRAMING_LIMIT: u32 = 64;
/// Silent blocks sent at setup, so the device sees host traffic at once.
const PRIME_BLOCKS: usize = 2;
/// Window for the rate measurement, in microseconds.
const RATE_WINDOW_US: u64 = 1_000_000;

/// A matched Overbridge device.
#[derive(Clone, Copy, Debug)]
pub struct OverbridgeMatch {
    pub pid: u16,
    pub name: &'static str,
    pub format: Format,
    /// Channels the device sends.
    pub ch_from: usize,
    /// Channels the device receives.
    pub ch_to: usize,
    pub ep_in: EndpointDescriptor,
    pub ep_out: EndpointDescriptor,
}

impl OverbridgeMatch {
    pub fn info(&self) -> OverbridgeInfo {
        OverbridgeInfo { pid: self.pid, name: self.name, ch_from: self.ch_from as u8, ch_to: self.ch_to as u8, format: self.format }
    }
}

/// Errors from the Overbridge host class.
#[derive(Debug)]
pub enum OverbridgeError {
    /// Not Elektron's VID, or no interrupt endpoints in interfaces 1 and 2 at alt 3.
    NotOverbridge,
    /// An alt-3 endpoint's size isn't a whole, non-zero number of channels.
    BadBlockSize,
    /// More channels than a block can carry, or than the handle's rings hold.
    TooManyChannels,
    /// A pipe couldn't be allocated.
    NoPipe(HostError),
    /// `SET_INTERFACE` failed.
    SetupFailed(PipeError),
    /// A transfer failed (including a detach).
    Transport(PipeError),
    /// [`FRAMING_LIMIT`] bad blocks in a row.
    Framing,
}

/// The alt-3 interrupt endpoint of `interface` going `dir`.
fn stream_endpoint(cfg: &ConfigurationDescriptor<'_>, interface: u8, dir: Direction) -> Option<EndpointDescriptor> {
    cfg.iter_interface()
        .filter(|i| i.interface_number == interface && i.alternate_setting == proto::STREAM_ALT)
        .flat_map(|i| i.iter_endpoints().collect::<heapless::Vec<_, 4>>())
        .find(|e| e.ep_type() == EndpointType::Interrupt && e.ep_dir() == dir)
}

/// Match an enumerated device. Channel counts come from the endpoint sizes; the device table
/// supplies the name and the v2.1 layout, and the descriptors win when they disagree.
pub fn match_device(vid: u16, pid: u16, cfg: &ConfigurationDescriptor<'_>) -> Result<OverbridgeMatch, OverbridgeError> {
    if vid != proto::VID_ELEKTRON {
        return Err(OverbridgeError::NotOverbridge);
    }
    let ep_in = stream_endpoint(cfg, proto::IN_INTERFACE, Direction::In).ok_or(OverbridgeError::NotOverbridge)?;
    let ep_out = stream_endpoint(cfg, proto::OUT_INTERFACE, Direction::Out).ok_or(OverbridgeError::NotOverbridge)?;
    let listed = proto::lookup(pid);
    let format = listed.map_or(Format::V2, |d| d.format);
    let size = |e: &EndpointDescriptor| (e.max_packet_size & 0x7FF) as usize;
    let ch_from = format.channels_from_device(size(&ep_in)).ok_or(OverbridgeError::BadBlockSize)?;
    let ch_to = format.channels_to_device(size(&ep_out)).ok_or(OverbridgeError::BadBlockSize)?;
    if ch_from > MAX_CHANNELS || ch_to > MAX_CHANNELS {
        return Err(OverbridgeError::TooManyChannels);
    }
    if let Some(d) = listed.filter(|d| (d.ch_from as usize, d.ch_to as usize) != (ch_from, ch_to)) {
        log::debug!("overbridge: {} declares {}/{} channels; the table says {}/{}", d.name, ch_from, ch_to, d.ch_from, d.ch_to);
    }
    Ok(OverbridgeMatch { pid, name: listed.map_or("Elektron device", |d| d.name), format, ch_from, ch_to, ep_in, ep_out })
}

/// Frames received against local time, over [`RATE_WINDOW_US`] windows.
#[derive(Default)]
struct RateMeter {
    start_us: Option<u64>,
    frames: u64,
}

impl RateMeter {
    /// Count `frames` received at `now_us`; returns the ppm offset when a window closes.
    fn add(&mut self, frames: u64, now_us: u64) -> Option<i32> {
        let start = *self.start_us.get_or_insert(now_us);
        let elapsed = now_us.saturating_sub(start);
        if elapsed >= RATE_WINDOW_US {
            let ppm = (self.frames as i64 * 1_000_000_000_000 / (elapsed as i64 * proto::SAMPLE_RATE as i64)) - 1_000_000;
            self.start_us = Some(now_us);
            self.frames = frames;
            return Some(ppm as i32);
        }
        self.frames += frames;
        None
    }
}

/// A hosted Overbridge device.
pub struct OverbridgeHost<'d, A: UsbHostAllocator<'d>> {
    control: A::Pipe<pipe::Control, pipe::InOut>,
    rx: A::Pipe<pipe::Interrupt, pipe::In>,
    tx: A::Pipe<pipe::Interrupt, pipe::Out>,
    m: OverbridgeMatch,
    tx_counter: u16,
    rx_counter: CounterTracker,
    framing_run: u32,
    rate: RateMeter,
    rx_block: [u8; MAX_BLOCK],
    tx_block: [u8; MAX_BLOCK],
    frames: [i32; FRAMES_PER_BLOCK * MAX_CHANNELS],
    _p: core::marker::PhantomData<&'d ()>,
}

impl<'d, A: UsbHostAllocator<'d>> OverbridgeHost<'d, A> {
    /// Match, select alternate setting 3 on both audio interfaces, open the stream pipes, and
    /// prime the OUT stream.
    pub async fn try_register(alloc: &A, addr: u8, split: Option<SplitInfo>, vid: u16, pid: u16, cfg: &ConfigurationDescriptor<'_>) -> Result<Self, OverbridgeError> {
        let m = match_device(vid, pid, cfg)?;
        let mut control = alloc
            .alloc_pipe::<pipe::Control, pipe::InOut>(
                addr,
                &embassy_usb_driver::EndpointInfo { addr: 0u8.into(), ep_type: EndpointType::Control, max_packet_size: 64, interval_ms: 0 },
                split,
            )
            .map_err(OverbridgeError::NoPipe)?;
        Self::select_alternates(&mut control).await?;
        // Open the pipes only now: the device must not see traffic for an endpoint it hasn't
        // enabled.
        let rx = alloc.alloc_pipe::<pipe::Interrupt, pipe::In>(addr, &m.ep_in.into(), split).map_err(OverbridgeError::NoPipe)?;
        let tx = alloc.alloc_pipe::<pipe::Interrupt, pipe::Out>(addr, &m.ep_out.into(), split).map_err(OverbridgeError::NoPipe)?;
        log::info!("overbridge: {} (PID {:04x}), {} ch from / {} ch to, {:?}", m.name, m.pid, m.ch_from, m.ch_to, m.format);
        let mut host = Self {
            control,
            rx,
            tx,
            m,
            tx_counter: 0,
            rx_counter: CounterTracker::default(),
            framing_run: 0,
            rate: RateMeter::default(),
            rx_block: [0; MAX_BLOCK],
            tx_block: [0; MAX_BLOCK],
            frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS],
            _p: core::marker::PhantomData,
        };
        host.prime().await?;
        Ok(host)
    }

    /// The matched device.
    pub fn matched(&self) -> &OverbridgeMatch {
        &self.m
    }

    async fn select_alternates(control: &mut A::Pipe<pipe::Control, pipe::InOut>) -> Result<(), OverbridgeError> {
        for interface in [proto::IN_INTERFACE, proto::OUT_INTERFACE] {
            let setup = SetupPacket {
                request_type: RequestType { direction: Direction::Out, control_type: ControlType::Standard, recipient: Recipient::Interface },
                request: REQ_SET_INTERFACE,
                value: proto::STREAM_ALT as u16,
                index: interface as u16,
                length: 0,
            };
            control.control_out(&setup.to_bytes(), &[]).await.map_err(OverbridgeError::SetupFailed)?;
        }
        Ok(())
    }

    /// Send one OUT block of `frames` (7 × ch_to, interleaved).
    async fn send(&mut self, silent: bool) -> Result<(), OverbridgeError> {
        let n = FRAMES_PER_BLOCK * self.m.ch_to;
        if silent {
            self.frames[..n].fill(0);
        }
        let len = proto::encode_to_device(self.m.format, self.m.ch_to, self.tx_counter, &self.frames[..n], &mut self.tx_block);
        self.tx.request_out(&self.tx_block[..len], false).await.map_err(OverbridgeError::Transport)?;
        self.tx_counter = self.tx_counter.wrapping_add(FRAMES_PER_BLOCK as u16);
        Ok(())
    }

    async fn prime(&mut self) -> Result<(), OverbridgeError> {
        for _ in 0..PRIME_BLOCKS {
            self.send(true).await?;
        }
        Ok(())
    }

    /// Receive one block into `h`, then send one block from it. `now_us` is local time, for
    /// the rate measurement.
    pub async fn pump_once<const RX: usize, const TX: usize>(&mut self, h: &OverbridgeHandle<RX, TX>, now_us: u64) -> Result<(), OverbridgeError> {
        let want = (self.m.ep_in.max_packet_size & 0x7FF) as usize;
        let got = self.rx.request_in(&mut self.rx_block[..want]).await.map_err(OverbridgeError::Transport)?;
        let n = FRAMES_PER_BLOCK * self.m.ch_from;
        match proto::decode_from_device(self.m.format, self.m.ch_from, &self.rx_block[..got], &mut self.frames[..n]) {
            Ok(counter) => {
                self.framing_run = 0;
                let gaps = self.rx_counter.observe(counter);
                h.push_rx(&self.frames[..n]);
                let ppm = self.rate.add(FRAMES_PER_BLOCK as u64, now_us);
                h.update_stats(|s| {
                    s.blocks_in = s.blocks_in.wrapping_add(1);
                    s.counter_gaps = s.counter_gaps.saturating_add(gaps);
                    if let Some(ppm) = ppm {
                        s.rate_ratio_ppm = ppm;
                    }
                });
            }
            Err(_) => {
                self.framing_run += 1;
                h.update_stats(|s| s.framing_errors = s.framing_errors.saturating_add(1));
                if self.framing_run >= FRAMING_LIMIT {
                    return Err(OverbridgeError::Framing);
                }
            }
        }
        // One block out per block in, even after a bad one: the device's clock paces both.
        let n = FRAMES_PER_BLOCK * self.m.ch_to;
        h.pull_tx(&mut self.frames[..n]);
        self.send(false).await?;
        h.update_stats(|s| s.blocks_out = s.blocks_out.wrapping_add(1));
        Ok(())
    }

    /// Recover from a transport error: select the alternates again, reset the data toggles
    /// and the counter, and prime the OUT stream.
    pub async fn restart(&mut self) -> Result<(), OverbridgeError> {
        Self::select_alternates(&mut self.control).await?;
        self.rx.reset_data_toggle();
        self.tx.reset_data_toggle();
        self.rx_counter.reset();
        self.framing_run = 0;
        self.rate = RateMeter::default();
        self.prime().await
    }
}
```

`embassy_usb_host::control` provides `SetupPacket`/`RequestType`/`ControlType`/`Recipient`. UAC
already uses them, so copy its `use` line if the path differs.

The rate-test arithmetic: the meter starts at the first block (t = 0) and closes the window at
the block whose `now_us` reaches 1 000 000. The frames counted before that block are 6858 × 7 =
48 006, so (48 006 × 10¹² / (10⁶ × 48 000)) − 10⁶ = 125.

- [ ] **Step 5: Run the tests to verify they pass**

Run Task 1's Step 3 command with `| tail -3`.
Expected: all usb-classes tests pass.

- [ ] **Step 6: Commit**

```bash
git add usb/usb-classes/src/host
git commit -m "usb-classes: host::overbridge, the Overbridge 2 host class (match, setup, paced streaming)"
```

---

### Task 4: BSP binding

**Files:**
- Modify: `crates/deluge-bsp/Cargo.toml` (feature `usb-host-overbridge`)
- Modify: `crates/deluge-bsp/src/usb/host.rs` (static handle, bind, task, supervisor order)
- Modify: `tools/test.sh` (deluge-bsp test features)
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `OverbridgeHost::{try_register, pump_once, restart, matched}`, `OverbridgeHandle`.
- Produces:
  - `deluge_bsp::usb::host::OverbridgeStems`
    (= `OverbridgeHandle<OVERBRIDGE_RX_SAMPLES, OVERBRIDGE_TX_SAMPLES>`);
  - `pub fn overbridge() -> &'static OverbridgeStems`;
  - `pub fn is_overbridge_candidate(vid: u16) -> bool`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/deluge-bsp/src/usb/host.rs`:

```rust
    #[cfg(feature = "usb-host-overbridge")]
    #[test]
    fn only_elektron_devices_are_offered_to_overbridge() {
        assert!(is_overbridge_candidate(0x1935));
        assert!(!is_overbridge_candidate(0x1234));
    }

    #[cfg(feature = "usb-host-overbridge")]
    #[test]
    fn the_overbridge_handle_holds_a_digitakt_ii() {
        use usb_classes::overbridge::stems::MIN_RING_FRAMES;
        assert!(OVERBRIDGE_RX_SAMPLES / 42 >= 512, "≥ 512 frames (10.7 ms) at 42 channels");
        assert!(OVERBRIDGE_TX_SAMPLES / 8 >= 512);
        assert!(OVERBRIDGE_RX_SAMPLES / 42 >= MIN_RING_FRAMES);
    }
```

In `Cargo.toml` `[features]`, after `usb-host`:

```toml
## The Elektron Overbridge 2 host class on top of `usb-host` (`usb::host::overbridge`): a
## Digitakt or another Overbridge device streams its stems to the Deluge.
usb-host-overbridge = ["usb-host", "usb-classes/host-overbridge"]
```

In `tools/test.sh`, change `-p deluge-bsp --features usb-host --lib` to
`-p deluge-bsp --features usb-host,usb-host-overbridge --lib`.

- [ ] **Step 2: Run it to verify it fails**

```bash
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host,usb-host-overbridge --lib usb::host 2>&1 | grep -E "^error" | sort | uniq -c
```
Expected: compile errors (`is_overbridge_candidate`, `OVERBRIDGE_RX_SAMPLES` not found).

- [ ] **Step 3: Implement the handle and the candidate check**

In `crates/deluge-bsp/src/usb/host.rs`, after `has_hub_interface`:

```rust
/// Samples in the Overbridge receive ring: 512 frames at 42 channels (a Digitakt II), more
/// at fewer (2730 at a Digitakt's 12). 84 KB.
#[cfg(feature = "usb-host-overbridge")]
pub const OVERBRIDGE_RX_SAMPLES: usize = 42 * 512;
/// Samples in the Overbridge send ring: 512 frames at 8 channels. 16 KB.
#[cfg(feature = "usb-host-overbridge")]
pub const OVERBRIDGE_TX_SAMPLES: usize = 8 * 512;

/// The app's view of a hosted Overbridge device.
#[cfg(feature = "usb-host-overbridge")]
pub type OverbridgeStems = usb_classes::overbridge::stems::OverbridgeHandle<OVERBRIDGE_RX_SAMPLES, OVERBRIDGE_TX_SAMPLES>;

#[cfg(feature = "usb-host-overbridge")]
static OVERBRIDGE: OverbridgeStems = OverbridgeStems::new();

/// The hosted Overbridge device's stems: [`info`](OverbridgeStems::info) is `None` until one
/// is connected.
#[cfg(feature = "usb-host-overbridge")]
pub fn overbridge() -> &'static OverbridgeStems {
    &OVERBRIDGE
}

/// Whether a device is offered to the Overbridge class before UAC: Elektron's VID. An
/// Elektron box in its Audio/MIDI USB mode doesn't match there and falls through to UAC.
#[cfg(feature = "usb-host-overbridge")]
pub fn is_overbridge_candidate(vid: u16) -> bool {
    vid == usb_classes::overbridge::proto::VID_ELEKTRON
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run the Step 2 command with `| tail -3`.
Expected: PASS.

- [ ] **Step 5: Bind it in the supervisor**

In the `runtime` module, after `uac_capture_task`:

```rust
    /// Bind an Elektron device's Overbridge interfaces and spawn its stream task. `false` when
    /// it doesn't match (an Elektron box in Audio/MIDI mode) or setup fails twice.
    #[cfg(feature = "usb-host-overbridge")]
    fn bind_overbridge(
        handle: &HostAlloc,
        spawner: Spawner,
        dev_info: &embassy_usb_host::handler::EnumerationInfo,
        cfg: &ConfigurationDescriptor<'_>,
    ) -> bool {
        use usb_classes::host::overbridge::{OverbridgeError, OverbridgeHost};
        let d = &dev_info.device_desc;
        let register = || {
            embassy_futures::block_on(OverbridgeHost::try_register(
                handle,
                dev_info.device_address,
                dev_info.split(),
                d.vendor_id,
                d.product_id,
                cfg,
            ))
        };
        // Spec §5.5: a failed SET_INTERFACE is retried once after 100 ms.
        let host = match register() {
            Err(OverbridgeError::SetupFailed(e)) => {
                warn!("usb_host: overbridge setup failed ({:?}); retrying", e);
                embassy_futures::block_on(embassy_time::Timer::after_millis(100));
                register()
            }
            r => r,
        };
        let host = match host {
            Ok(h) => h,
            Err(OverbridgeError::NotOverbridge) => return false,
            Err(e) => {
                warn!("usb_host: overbridge device not bound: {:?}", e);
                return false;
            }
        };
        let m = *host.matched();
        if super::overbridge().begin(m.info(), m.ch_from, m.ch_to).is_err() {
            warn!("usb_host: overbridge {} has more channels than the handle holds", m.name);
            return false;
        }
        match overbridge_task(host) {
            Ok(token) => {
                spawner.spawn(token);
                true
            }
            Err(_) => {
                super::overbridge().end();
                error!("usb_host: could not spawn the overbridge task");
                false
            }
        }
    }

    /// Streams one Overbridge device until it detaches or keeps failing: a transport error
    /// restarts the stream (spec §5.5), and the third within 10 s ends it.
    #[cfg(feature = "usb-host-overbridge")]
    #[embassy_executor::task]
    async fn overbridge_task(mut host: usb_classes::host::overbridge::OverbridgeHost<'static, HostAlloc>) {
        use embassy_time::{Duration, Instant};
        const WINDOW: Duration = Duration::from_secs(10);
        let mut failures: heapless::Deque<Instant, 3> = heapless::Deque::new();
        let stems = super::overbridge();
        loop {
            let Err(e) = host.pump_once(stems, Instant::now().as_micros()).await else {
                continue;
            };
            let now = Instant::now();
            while failures.front().is_some_and(|t| now.duration_since(*t) > WINDOW) {
                failures.pop_front();
            }
            if failures.is_full() {
                failures.pop_front();
            }
            let _ = failures.push_back(now);
            warn!("usb_host: overbridge stream error {:?} ({} in 10 s)", e, failures.len());
            if failures.len() >= 3 || host.restart().await.is_err() {
                break;
            }
        }
        info!("usb_host: overbridge stream ended");
        stems.end();
    }
```

In the root-port composite bind (`let midi = bind_midi(...); let uac = bind_uac(...); midi || uac`),
replace the two `let` lines and the result with:

```rust
                        let midi = bind_midi(&handle, spawner, &dev_info, &cfg);
                        // Overbridge first for Elektron's VID; UAC otherwise, or if it doesn't match.
                        #[cfg(feature = "usb-host-overbridge")]
                        let audio = (super::is_overbridge_candidate(dev_info.device_desc.vendor_id)
                            && bind_overbridge(&handle, spawner, &dev_info, &cfg))
                            || bind_uac(&handle, spawner, &dev_info, &cfg);
                        #[cfg(not(feature = "usb-host-overbridge"))]
                        let audio = bind_uac(&handle, spawner, &dev_info, &cfg);
                        midi || audio
```

Check the names the file already uses for `info!/warn!/error!`, `heapless` and `embassy_time`
(they're used elsewhere in `runtime`). Add `heapless` to deluge-bsp's dependencies if it's
missing.

- [ ] **Step 6: Build the BSP for the device with the feature**

```bash
cargo build -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem --release --no-default-features -p deluge-bsp --features usb-host-overbridge --target armv7a-none-eabihf 2>&1 | grep -E "^(error|warning: unused)" | head
```
Expected: no errors. If the BSP needs a default feature for its linker script, add the BSP's
default features back (`--features usb-host-overbridge,<defaults>`); the point is that the
`runtime` module compiles.

- [ ] **Step 7: CHANGELOG and commit**

Under `### Added`:

```markdown
- Elektron Overbridge 2 host support (`usb-classes` feature `host-overbridge`; `deluge-bsp`
  feature `usb-host-overbridge`). A Digitakt, Digitone, Syntakt, Analog Four/Rytm/Heat MKII,
  Digitakt II or Digitone II in Overbridge mode streams every track to the Deluge as 48 kHz
  stems, read through `deluge_bsp::usb::host::overbridge()`, with a stream back to the
  device. Elektron devices are offered to Overbridge before UAC.
```

```bash
git add crates/deluge-bsp tools/test.sh CHANGELOG.md
git commit -m "deluge-bsp: usb-host-overbridge binds Elektron devices to the Overbridge class before UAC"
```

---

### Task 5: Hardware harness and the hardware-task log

**Files:**
- Create: `hw-tests/overbridge-host-firmware/` (copy `hw-tests/uac-host-firmware`'s layout:
  `Cargo.toml`, `build.rs` if any, `src/main.rs`, `README.md`)
- Modify: root `Cargo.toml` `members`
- Create: `docs/hardware-tasks.md`

- [ ] **Step 1: The harness**

Base it on `hw-tests/uac-host-firmware` (read its `Cargo.toml`, `main.rs` and `tasks/` first and
keep its bring-up verbatim: `rza1l_hal::init`, `Board::new`, USB host start, supervisor spawn).
Swap the UAC-specific parts for:
- the `deluge-bsp` feature `usb-host-overbridge`;
- a `report_task` that, once per second, logs `overbridge().info()` and `overbridge().stats()`
  and drains the stems, logging each stem's peak in dBFS (`20·log10(peak / 2^31)`, or `-inf`);
- a `tone_task` that keeps the send ring topped up with a 1 kHz sine at −12 dBFS on every
  channel. 48 samples per cycle at 48 kHz; generate it from a 48-entry `i32` table computed at
  start with `libm::sinf`, or a hand-written table if `libm` isn't a dependency.

Both tasks run `loop { ...; Timer::after_millis(N).await }`. The report runs every 1000 ms. The
tone task writes 48 frames whenever `stats().fill_out < 256`, checking every 2 ms.

- [ ] **Step 2: Build it**

```bash
cargo build -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem --release --no-default-features -p overbridge-host-firmware 2>&1 | grep -E "^error" | head
```
Expected: no errors. Mirror whatever flags `uac-host-firmware`'s README gives if they differ.

- [ ] **Step 3: `docs/hardware-tasks.md`**

Create it with a short preamble ("checks only hardware can answer; each lists its setup, what
passes, and where results go"), and one section per spec §8.4 step. Each section has Setup /
Pass / Record:
1. Confirm the Digitakt descriptors against spec Appendix A (any host build with `log` at debug:
   the supervisor logs the configuration).
2. Digitakt stems: `overbridge-host-firmware`; 10 minutes with `counter_gaps == 0` and
   `framing_errors == 0`; record the CPU load; repeat once DMA lands (sub-project 2) to record
   the difference.
3. OUT path: the harness's 1 kHz tone is audible on the Digitakt (its Overbridge input routed to
   its output).
4. Digitakt II (when available): as 2–3 at 42/8; also settles the bulk-over-512-on-pipes-1–2
   question (spec §10).
5. Outbox: a Deluge running `AudioClass<8>` into an Outbox 8; 8 distinct channels on its
   outputs; record the sample rate and sync mode it accepts (spec §7.1).
6. Device class: two Deluges (host and device with a test identity), then the device against
   Overwitch on Linux with a local `devices.json` entry (sub-project 4).

Also add a section for sub-project 1: a USB-MIDI keyboard and a hub still enumerate and play,
since hub and MIDI interrupt pipes changed their polling interval.

- [ ] **Step 4: Commit**

```bash
git add hw-tests/overbridge-host-firmware Cargo.toml Cargo.lock docs/hardware-tasks.md
git commit -m "hw-tests: overbridge-host-firmware harness; docs: hardware-tasks.md for the Overbridge checks"
```

- [ ] **Step 5: Suites**

```bash
rza1/tools/test.sh > "$SCRATCH/sp3-rza1.log" 2>&1; echo "rza1 $?"
tools/test.sh > "$SCRATCH/sp3-root.log" 2>&1; echo "root $?"; grep -n "test result" "$SCRATCH/sp3-root.log" | tail -5
```
Expected: rza1 exits 0. While `../flare` is ahead of CI's pin the root suite stops at
`deluge-wren-core` (E0004); in that case confirm the `deluge-bsp` and `usb-classes` lines passed.
