//! Offline (non-real-time) rendering.
//!
//! The engine has no wall clock — it *cannot* have one, being `no_std` — so
//! nothing about it is tied to running at 1 block per 1.3 ms. `sample_clock`
//! advances by `BLOCK` per [`Engine::render_block`], and scheduled commands
//! ([`Engine::apply_at`]) fire against that counter rather than against real
//! time. Drive it in a tight loop and it renders as fast as the host can go,
//! deterministically.
//!
//! That is what scsynth calls NRT mode. There, it exists to bounce a piece to
//! disk. Here the valuable use is **regression testing**: render a whole patch
//! and compare it against a stored reference, rather than pinning eight samples
//! as literal constants and hoping the ninth never drifts.
//!
//! Deliberately not included: score files, soundfile output, `DiskOut`. Those
//! are what make NRT a *product* feature for a desktop server, and they buy a
//! hardware instrument nothing. This is a test driver.
//!
//! ```ignore
//! let mut e = Engine::<64, 8, 16, 4, 45056, 2048>::new(48_000.0);
//! // ... build a patch ...
//! e.apply_at(24_000, Cmd::Gate { node: env, on: false }); // half a second in
//! let mut out = [StereoFrame::default(); 48_000];
//! e.render_offline(&mut out);
//! assert_eq!(nrt::digest(&out), 0x9e37_79b9_7f4a_7c15);
//! ```

use crate::{Engine, StereoFrame};

impl<
    const BLOCK: usize,
    const NODES: usize,
    const OUTS: usize,
    const BUSES: usize,
    const PCAP: usize,
    const PCHUNK: usize,
> Engine<BLOCK, NODES, OUTS, BUSES, PCAP, PCHUNK>
{
    /// Render `out.len()` frames as fast as the host can, against silence.
    ///
    /// Equivalent to calling [`Engine::render`] in a loop, but it handles a
    /// length that is not a whole number of blocks. Scheduled commands fire on
    /// their sample positions exactly as they would in real time, because the
    /// clock they fire against is the same one either way.
    ///
    /// A trailing partial block is rendered in full and truncated — the engine
    /// advances by a whole block regardless, so this keeps the sample clock and
    /// the returned audio in step rather than pretending a half block elapsed.
    pub fn render_offline(&mut self, out: &mut [StereoFrame]) {
        let silence = [StereoFrame::default(); MAX_OFFLINE_BLOCK];
        let mut buf = [StereoFrame::default(); MAX_OFFLINE_BLOCK];
        let mut done = 0;
        while done < out.len() {
            let n = (out.len() - done).min(BLOCK);
            self.render(&mut buf[..BLOCK], &silence[..BLOCK]);
            out[done..done + n].copy_from_slice(&buf[..n]);
            done += n;
        }
    }
}

/// Scratch size for [`Engine::render_offline`]. Matches `node::MAX_BLOCK`, the
/// largest `BLOCK` any engine may be instantiated with, so one fixed buffer
/// serves every instantiation without allocating.
const MAX_OFFLINE_BLOCK: usize = crate::node::MAX_BLOCK;

/// A 64-bit digest of rendered audio, for pinning a patch's output.
///
/// FNV-1a over the raw bit patterns of every sample, so it is exact rather
/// than tolerant: two renders match only if they are bit-identical. That is
/// the right strictness for a characterisation test, whose job is to notice
/// *any* drift and make a human decide whether it was intended.
///
/// Hashing the bits rather than the values means `-0.0` and `+0.0` differ, and
/// any NaN sample makes the digest depend on which NaN — both are things a
/// characterisation test should flag, not smooth over.
///
/// A digest is portable across builds *because* the kernels hold themselves to
/// scalar/SIMD bit-exactness (every kernel has a scalar oracle and a
/// `scalar == simd` test). Verified: the pinned goldens match on x86-64 and on
/// 32-bit ARM, with and without `--features simd`. If that ever stops being
/// true, a digest test is where it surfaces — which is the point, not a flaw.
/// Where a tolerant comparison is genuinely wanted, use [`rms`].
pub fn digest(frames: &[StereoFrame]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    let mut mix = |bits: u32| {
        for b in bits.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(PRIME);
        }
    };
    for f in frames {
        mix(f.l.to_bits());
        mix(f.r.to_bits());
    }
    h
}

/// Largest absolute sample across both channels. `0.0` for an empty slice.
pub fn peak(frames: &[StereoFrame]) -> f32 {
    frames
        .iter()
        .fold(0.0f32, |m, f| m.max(f.l.abs()).max(f.r.abs()))
}

/// Root-mean-square level across both channels. `0.0` for an empty slice.
///
/// Useful where `peak` is too brittle — comparing two renders that should be
/// *musically* the same without being bit-identical, e.g. across a SIMD and a
/// scalar build.
pub fn rms(frames: &[StereoFrame]) -> f32 {
    if frames.is_empty() {
        return 0.0;
    }
    let sum: f32 = frames.iter().map(|f| f.l * f.l + f.r * f.r).sum();
    libm::sqrtf(sum / (frames.len() as f32 * 2.0))
}

/// `true` if every sample is finite and within `[-1.0, 1.0]`.
///
/// The blanket assertion most render tests want: no NaN, no infinity, nothing
/// that would clip the DAC.
pub fn is_clean(frames: &[StereoFrame]) -> bool {
    frames
        .iter()
        .all(|f| f.l.is_finite() && f.r.is_finite() && f.l.abs() <= 1.0 && f.r.abs() <= 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::Cmd;
    use crate::node::Kind;
    use crate::{Input, NodeId};

    type E = Engine<64, 8, 16, 4, 45056, 2048>;

    /// A 220 Hz saw routed to the root bus.
    fn saw(e: &mut E) {
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Saw,
            args: [Input::Const(220.0), Input::Const(0.0), Input::Const(0.0)],
        });
        e.apply(Cmd::BusWrite {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: crate::BusId(0),
        });
        e.apply(Cmd::SetRoot {
            bus: crate::BusId(0),
        });
    }

    #[test]
    fn offline_matches_a_manual_render_loop() {
        // The whole premise: rendering offline is not a different code path,
        // just a different caller. Two engines given identical input must
        // produce identical audio.
        let mut a = E::new(48_000.0);
        let mut b = E::new(48_000.0);
        saw(&mut a);
        saw(&mut b);

        let mut offline = [StereoFrame::default(); 256];
        a.render_offline(&mut offline);

        let mut manual = [StereoFrame::default(); 256];
        let silence = [StereoFrame::default(); 64];
        for chunk in manual.chunks_mut(64) {
            b.render(chunk, &silence);
        }
        assert_eq!(digest(&offline), digest(&manual));
    }

    #[test]
    fn a_partial_trailing_block_still_advances_the_clock_by_a_whole_block() {
        let mut e = E::new(48_000.0);
        saw(&mut e);
        let mut out = [StereoFrame::default(); 100]; // 1 block + 36 frames
        e.render_offline(&mut out);
        assert_eq!(
            e.sample_time(),
            128,
            "two render_block calls, not one and a bit"
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        // What makes a stored reference meaningful at all.
        let d: [u64; 2] = core::array::from_fn(|_| {
            let mut e = E::new(48_000.0);
            saw(&mut e);
            let mut out = [StereoFrame::default(); 512];
            e.render_offline(&mut out);
            digest(&out)
        });
        assert_eq!(d[0], d[1], "same patch, same audio, same digest");
    }

    #[test]
    fn the_digest_notices_a_changed_sample() {
        let mut out = [StereoFrame::default(); 8];
        let before = digest(&out);
        out[7].r = f32::from_bits(1); // the smallest possible subnormal
        assert_ne!(digest(&out), before, "one bit in the last sample");
    }

    #[test]
    fn the_digest_distinguishes_signed_zero() {
        // Hashing bits, not values: `-0.0 == 0.0` compares true, but a render
        // that started producing one instead of the other has changed.
        let a = [StereoFrame { l: 0.0, r: 0.0 }];
        let b = [StereoFrame { l: -0.0, r: 0.0 }];
        assert_eq!(a[0].l, b[0].l, "equal as values");
        assert_ne!(digest(&a), digest(&b), "distinct as renders");
    }

    #[test]
    fn scheduled_commands_fire_on_their_sample_positions_offline() {
        // The reason offline rendering is worth having: a whole timed score
        // plays out in one call, at whatever speed the host manages.
        let mut e = E::new(48_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Ctrl,
            args: [Input::Const(0.0); 3],
        });
        e.apply(Cmd::SetParam {
            node: NodeId(0),
            param: 0,
            value: 0.25,
        });
        e.apply(Cmd::BusWrite {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: crate::BusId(0),
        });
        e.apply(Cmd::SetRoot {
            bus: crate::BusId(0),
        });
        // Halfway through the render, move the value.
        e.apply_at(
            256,
            Cmd::SetParam {
                node: NodeId(0),
                param: 0,
                value: 0.75,
            },
        );

        let mut out = [StereoFrame::default(); 512];
        e.render_offline(&mut out);
        assert!((out[0].l - 0.25).abs() < 1e-6, "before the command");
        assert!((out[511].l - 0.75).abs() < 1e-6, "after it");
    }

    #[test]
    fn metrics_read_a_rendered_patch() {
        let mut e = E::new(48_000.0);
        saw(&mut e);
        let mut out = [StereoFrame::default(); 512];
        e.render_offline(&mut out);
        assert!(is_clean(&out), "finite and within [-1, 1]");
        assert!(peak(&out) > 0.1, "actually sounding: peak {}", peak(&out));
        assert!(rms(&out) > 0.0);
        assert!(rms(&out) <= peak(&out), "RMS cannot exceed peak");
    }

    #[test]
    fn metrics_handle_an_empty_render() {
        let none: [StereoFrame; 0] = [];
        assert_eq!(peak(&none), 0.0);
        assert_eq!(rms(&none), 0.0);
        assert!(is_clean(&none), "vacuously");
    }

    #[test]
    fn is_clean_rejects_what_it_should() {
        assert!(!is_clean(&[StereoFrame {
            l: f32::NAN,
            r: 0.0
        }]));
        assert!(!is_clean(&[StereoFrame {
            l: 0.0,
            r: f32::INFINITY
        }]));
        assert!(!is_clean(&[StereoFrame { l: 1.5, r: 0.0 }]), "clipping");
        assert!(is_clean(&[StereoFrame { l: -1.0, r: 1.0 }]), "full scale");
    }
}
