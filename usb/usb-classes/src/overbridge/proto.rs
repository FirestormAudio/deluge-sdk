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
    Device {
        pid,
        name,
        format,
        ch_from,
        ch_to,
    }
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
        if from_device {
            self.from_device_frame_bytes(ch)
        } else {
            self.to_device_frame_bytes(ch)
        }
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

fn decode(
    format: Format,
    ch: usize,
    from_device: bool,
    block: &[u8],
    frames: &mut [i32],
) -> Result<u16, BlockError> {
    if block.len() != block_len(format.frame_bytes(ch, from_device)) {
        return Err(BlockError::Length);
    }
    let header = if from_device {
        HEADER_FROM_DEVICE
    } else {
        HEADER_TO_DEVICE
    };
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

fn encode(
    format: Format,
    ch: usize,
    from_device: bool,
    counter: u16,
    frames: &[i32],
    block: &mut [u8],
) -> usize {
    let len = block_len(format.frame_bytes(ch, from_device));
    let block = &mut block[..len];
    let header = if from_device {
        HEADER_FROM_DEVICE
    } else {
        HEADER_TO_DEVICE
    };
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
pub fn decode_from_device(
    format: Format,
    ch: usize,
    block: &[u8],
    frames: &mut [i32],
) -> Result<u16, BlockError> {
    decode(format, ch, true, block, frames)
}

/// Encode 7 × `ch` interleaved frames as a host-to-device block. Returns its length.
pub fn encode_to_device(
    format: Format,
    ch: usize,
    counter: u16,
    frames: &[i32],
    block: &mut [u8],
) -> usize {
    encode(format, ch, false, counter, frames, block)
}

/// Device side: encode 7 × `ch` interleaved frames as a device-to-host block. Returns its length.
pub fn encode_from_device(
    format: Format,
    ch: usize,
    counter: u16,
    frames: &[i32],
    block: &mut [u8],
) -> usize {
    encode(format, ch, true, counter, frames, block)
}

/// Device side: decode a host-to-device block. Returns its frame counter.
pub fn decode_to_device(
    format: Format,
    ch: usize,
    block: &[u8],
    frames: &mut [i32],
) -> Result<u16, BlockError> {
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
                if d % FRAMES_PER_BLOCK as u32 == 0 && blocks <= Self::MAX_GAP {
                    blocks
                } else {
                    0
                }
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
        assert_eq!(
            (d.name, d.ch_from, d.ch_to, d.format),
            ("Digitakt", 12, 2, Format::V2)
        );
        let d2 = lookup(0x0B2B).unwrap();
        assert_eq!(
            (d2.name, d2.ch_from, d2.ch_to, d2.format),
            ("Digitakt II", 42, 8, Format::V2_1)
        );
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
        assert_eq!(
            Format::V2_1.channels_from_device(32 + 7 * 8),
            Some(2),
            "below 14 channels all are wide"
        );
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
        assert_eq!(
            decode_from_device(Format::V2, 12, &block, &mut out),
            Ok(0x1234)
        );
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
                samples.push(if c < 14 {
                    std::vec![0x00, 0x12, 0x34, 0x56]
                } else {
                    std::vec![0x12, 0x34, 0x56]
                });
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
        assert_eq!(
            encode_to_device(Format::V2_1, 8, 0, &frames, &mut block),
            256
        );
        assert_eq!(&block[32..36], &[0x00, 0x12, 0x34, 0x56]);
        assert_eq!(&block[36..40], &[0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn device_side_codecs_round_trip_with_the_host_side() {
        for (format, ch_from, ch_to) in [(Format::V2, 12, 2), (Format::V2_1, 42, 8)] {
            // Device → host. v2.1 carries 24 bits, so test values have a zero low byte.
            let frames: Vec<i32> = (0..7 * ch_from)
                .map(|i| value(i / ch_from, i % ch_from))
                .collect();
            let mut block = [0u8; MAX_BLOCK];
            let n = encode_from_device(format, ch_from, 21, &frames, &mut block);
            let mut back = std::vec![0i32; 7 * ch_from];
            assert_eq!(
                decode_from_device(format, ch_from, &block[..n], &mut back),
                Ok(21)
            );
            assert_eq!(back, frames);
            // Host → device.
            let frames: Vec<i32> = (0..7 * ch_to)
                .map(|i| value(i / ch_to, i % ch_to))
                .collect();
            let n = encode_to_device(format, ch_to, 28, &frames, &mut block);
            let mut back = std::vec![0i32; 7 * ch_to];
            assert_eq!(
                decode_to_device(format, ch_to, &block[..n], &mut back),
                Ok(28)
            );
            assert_eq!(back, frames);
        }
    }

    #[test]
    fn a_wrong_header_or_length_is_refused() {
        let mut block = [0u8; 368];
        block[..2].copy_from_slice(&0x0700u16.to_be_bytes());
        let mut out = [0i32; 7 * 12];
        assert!(decode_from_device(Format::V2, 12, &block, &mut out).is_ok());
        assert_eq!(
            decode_from_device(Format::V2, 12, &block[..367], &mut out),
            Err(BlockError::Length)
        );
        assert_eq!(
            decode_from_device(Format::V2, 12, &[], &mut out),
            Err(BlockError::Length)
        );
        block[1] = 0xFF; // 0x07FF: a host-to-device header
        assert_eq!(
            decode_from_device(Format::V2, 12, &block, &mut out),
            Err(BlockError::Header)
        );
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
        assert_eq!(
            t.observe(17 + 7 * 5000),
            0,
            "an implausible jump resyncs too"
        );
    }
}
