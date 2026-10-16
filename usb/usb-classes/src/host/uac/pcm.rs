//! Type-I PCM sample codec for the subslot sizes UAC2 devices use: 2, 3 and 4 bytes per
//! sample, little-endian, signed.
//!
//! A sample narrower than its subslot is MSB-justified (UAC2 Format Type I: the low bits are
//! padding), so a 4-byte subslot decodes as a full-scale 32-bit sample whatever its
//! `bBitResolution` (24-in-32 and 32-bit alike), as Linux's `S32_LE` does.

/// The encoding of one sample, chosen by the format's subslot size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// 2-byte subslot.
    S16,
    /// 3-byte subslot.
    S24,
    /// 4-byte subslot (24-in-32 or 32-bit).
    S32,
}

impl SampleFormat {
    /// The format for a `bSubslotSize`, or `None` for one this driver does not handle.
    pub fn from_subslot(subslot: u8) -> Option<Self> {
        match subslot {
            2 => Some(Self::S16),
            3 => Some(Self::S24),
            4 => Some(Self::S32),
            _ => None,
        }
    }

    /// Bytes per sample (the subslot size).
    pub fn bytes(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::S24 => 3,
            Self::S32 => 4,
        }
    }

    /// Decode one sample (`b.len() == self.bytes()`) to `f32` in `[-1.0, 1.0)`.
    pub fn decode(self, b: &[u8]) -> f32 {
        // Left-align the sample in an i32 so every width shares one full-scale divisor.
        let v = match self {
            Self::S16 => i32::from_le_bytes([0, 0, b[0], b[1]]),
            Self::S24 => i32::from_le_bytes([0, b[0], b[1], b[2]]),
            Self::S32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        };
        v as f32 / FULL_SCALE
    }

    /// Encode `x` (clamped to `[-1.0, 1.0)`) into `out[..self.bytes()]`.
    pub fn encode(self, x: f32, out: &mut [u8]) {
        // f32 -> i32 casts saturate, so +1.0 and above land on i32::MAX.
        let v = (x.clamp(-1.0, 1.0) * FULL_SCALE) as i32;
        let b = v.to_le_bytes();
        match self {
            Self::S16 => out[..2].copy_from_slice(&b[2..4]),
            Self::S24 => out[..3].copy_from_slice(&b[1..4]),
            Self::S32 => out[..4].copy_from_slice(&b),
        }
    }
}

/// 2^31: full scale for a left-aligned 32-bit sample.
const FULL_SCALE: f32 = 2_147_483_648.0;

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn the_subslot_size_picks_the_format() {
        assert_eq!(SampleFormat::from_subslot(2), Some(SampleFormat::S16));
        assert_eq!(SampleFormat::from_subslot(3), Some(SampleFormat::S24));
        assert_eq!(SampleFormat::from_subslot(4), Some(SampleFormat::S32));
        assert_eq!(SampleFormat::from_subslot(1), None);
        assert_eq!(SampleFormat::from_subslot(0), None);
        assert_eq!(SampleFormat::S16.bytes(), 2);
        assert_eq!(SampleFormat::S24.bytes(), 3);
        assert_eq!(SampleFormat::S32.bytes(), 4);
    }

    #[test]
    fn decoding_maps_full_scale_to_minus_one_and_half_scale_to_half() {
        assert_eq!(SampleFormat::S16.decode(&[0x00, 0x40]), 0.5);
        assert_eq!(SampleFormat::S16.decode(&[0x00, 0x80]), -1.0);
        assert_eq!(SampleFormat::S24.decode(&[0, 0, 0x40]), 0.5);
        assert_eq!(SampleFormat::S24.decode(&[0, 0, 0x80]), -1.0);
        assert_eq!(SampleFormat::S32.decode(&[0, 0, 0, 0x40]), 0.5);
        assert_eq!(SampleFormat::S32.decode(&[0, 0, 0, 0x80]), -1.0);
        for f in [SampleFormat::S16, SampleFormat::S24, SampleFormat::S32] {
            assert_eq!(f.decode(&[0; 4][..f.bytes()]), 0.0);
        }
    }

    /// A 24-bit sample in a 4-byte subslot sits in the top three bytes; the padding byte is the
    /// lowest, so the value is the same as the 3-byte encoding of it.
    #[test]
    fn a_24_bit_sample_in_a_4_byte_subslot_is_msb_justified() {
        assert_eq!(SampleFormat::S32.decode(&[0x00, 0x00, 0x00, 0x40]), 0.5);
        assert_eq!(
            SampleFormat::S32.decode(&[0x00, 0x56, 0x34, 0x12]),
            SampleFormat::S24.decode(&[0x56, 0x34, 0x12])
        );
    }

    #[test]
    fn encoding_round_trips_and_clamps() {
        for f in [SampleFormat::S16, SampleFormat::S24, SampleFormat::S32] {
            let mut b = [0u8; 4];
            for &x in &[0.0f32, 0.5, -0.5, -1.0, 0.25] {
                f.encode(x, &mut b);
                assert_eq!(f.decode(&b[..f.bytes()]), x, "{f:?} {x}");
            }
            f.encode(2.0, &mut b);
            assert!(f.decode(&b[..f.bytes()]) > 0.99, "{f:?} clamps high");
            f.encode(-2.0, &mut b);
            assert_eq!(f.decode(&b[..f.bytes()]), -1.0, "{f:?} clamps low");
        }
    }
}
