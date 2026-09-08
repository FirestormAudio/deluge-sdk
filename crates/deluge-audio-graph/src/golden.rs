//! Characterisation goldens: fixed patches rendered offline and pinned as
//! digests.
//!
//! These do not assert that the audio is *good* — they assert that it has not
//! *changed*. A failure here means some edit moved the numeric output of a
//! kernel or of the graph. That is occasionally intended and usually not.
//!
//! **Re-pinning a constant is not how you fix a failure here.** Re-pin only
//! when the output change is intended and has been reviewed, and when you do,
//! say so in a dated comment. A golden that gets re-pinned whenever it goes red
//! is not a golden, it is a very slow way of writing `assert!(true)`.
//!
//! Every golden is deliberately config-invariant: the same digest must hold in
//! the scalar and `simd` configurations, and on both the 64-bit host and the
//! 32-bit ARM bucket. That is what makes them a usable gate for a SIMD
//! refactor.

#![cfg(test)]

use crate::node::Kind;
use crate::{BusId, Cmd, Engine, Input, NodeId, StereoFrame};

/// The engine shape every digest golden uses. Wider than `cmd.rs`'s `E`
/// because the poly patches need the extra nodes and output slots.
type G = Engine<64, 16, 64, 4, 45056, 2048>;

/// Frames rendered by every digest golden. 4096 at 48 kHz is ~85 ms — long
/// enough for an envelope to open and close and for a filter's transient to
/// settle, so a coefficient change that only shows up after a few hundred
/// samples cannot slip through.
const FRAMES: usize = 4096;

/// Render `FRAMES` frames offline and assert the output is not obviously
/// broken before anyone pins a digest of it.
///
/// The cleanliness check is the point: a digest of NaN or of silence is a
/// perfectly stable digest, and pinning one would produce a green test that
/// guards nothing. Every golden goes through here.
fn render_4096(e: &mut G) -> [StereoFrame; FRAMES] {
    let mut out = [StereoFrame::default(); FRAMES];
    e.render_offline(&mut out);
    assert!(
        crate::nrt::is_clean(&out),
        "output must be finite and within [-1, 1] before it is worth pinning"
    );
    assert!(
        crate::nrt::peak(&out) > 1e-3,
        "output is silent ({}) — a digest of silence guards nothing",
        crate::nrt::peak(&out)
    );
    out
}

/// CHARACTERIZATION golden (spec §8 P0 testing gate): pins the first 8
/// rendered samples of the deterministic saw→lpf→env patch as literal
/// constants. Complements the digest golden below: this one says *how* the
/// output drifted, because you can read the numbers.
///
/// Regenerating the pinned constants is the correct response ONLY when the
/// output change is intended and has been reviewed.
#[test]
fn golden_saw_lpf_env_first_block() {
    type E = Engine<16, 8, 8, 4, 45056, 2048>;
    let mut e = E::new(48_000.0);
    e.apply(Cmd::NewNode {
        node: NodeId(0),
        kind: Kind::Saw,
        args: [Input::Const(4.0), Input::Const(0.0), Input::Const(0.0)],
    });
    e.apply(Cmd::NewNode {
        node: NodeId(1),
        kind: Kind::Lpf,
        args: [
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            Input::Const(800.0),
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::NewNode {
        node: NodeId(2),
        kind: Kind::Env,
        args: [Input::Const(0.01), Input::Const(0.1), Input::Const(0.0)],
    });
    e.apply(Cmd::Gate {
        node: NodeId(2),
        on: true,
    });
    e.apply(Cmd::NewNode {
        node: NodeId(3),
        kind: Kind::Mul,
        args: [
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            Input::Node {
                node: NodeId(2),
                port: 0,
            },
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(3),
            port: 0,
        },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let mut out = [StereoFrame::default(); 8];
    let sil = [StereoFrame::default(); 8];
    e.render(&mut out, &sil);

    // CHARACTERIZATION golden re-pinned 2026-07-07 after Osc band-limiting (Tasks 1-3).
    // Regenerate only on an intended, reviewed output change.
    const EXPECTED: [f32; 8] = [
        0.0,
        -0.000_399_898_62,
        -0.001_191_312_4,
        -0.002_294_306_4,
        -0.003_657_662_3,
        -0.005_237_465_3,
        -0.006_996_135,
        -0.008_901_581,
    ];
    let actual: [f32; 8] = core::array::from_fn(|i| out[i].l);
    for (i, (a, x)) in actual.iter().zip(EXPECTED.iter()).enumerate() {
        assert!(
            (a - x).abs() < 1e-6,
            "sample {i}: actual {a} vs pinned {x} (diff {})",
            (a - x).abs()
        );
    }
}

/// CHARACTERIZATION golden, wide rather than deep: the same patch as
/// [`golden_saw_lpf_env_first_block`], rendered offline for 4096 frames and
/// pinned as one digest. Released partway through, so the decay is covered.
///
/// The two are complementary, not redundant. The sample-wise golden covers 8
/// frames and tells you *how* the output drifted — you can read the numbers.
/// This one covers 512× more audio, through the envelope's attack and decay
/// rather than just its first moments, and tells you *that* something drifted.
#[test]
fn golden_saw_lpf_env_offline_digest() {
    type E = Engine<16, 8, 8, 4, 45056, 2048>;
    let mut e = E::new(48_000.0);
    e.apply(Cmd::NewNode {
        node: NodeId(0),
        kind: Kind::Saw,
        args: [Input::Const(4.0), Input::Const(0.0), Input::Const(0.0)],
    });
    e.apply(Cmd::NewNode {
        node: NodeId(1),
        kind: Kind::Lpf,
        args: [
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            Input::Const(800.0),
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::NewNode {
        node: NodeId(2),
        kind: Kind::Env,
        args: [Input::Const(0.01), Input::Const(0.1), Input::Const(0.0)],
    });
    e.apply(Cmd::Gate {
        node: NodeId(2),
        on: true,
    });
    e.apply(Cmd::NewNode {
        node: NodeId(3),
        kind: Kind::Mul,
        args: [
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            Input::Node {
                node: NodeId(2),
                port: 0,
            },
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(3),
            port: 0,
        },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });
    // Release partway through, so the digest covers the decay too.
    e.apply_at(
        2048,
        Cmd::Gate {
            node: NodeId(2),
            on: false,
        },
    );

    let mut out = [StereoFrame::default(); 4096];
    e.render_offline(&mut out);

    assert!(crate::nrt::is_clean(&out), "finite and within [-1, 1]");
    assert_eq!(
        crate::nrt::digest(&out),
        SAW_LPF_ENV_DIGEST,
        "patch output changed; the sample-wise golden above will say how"
    );
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const SAW_LPF_ENV_DIGEST: u64 = 4_722_302_078_756_468_769;

/// Set each voice of a `PolyCtrl` node to a distinct frequency.
///
/// Distinct rather than uniform on purpose: identical voices would sum to
/// exactly 8× one voice, and a digest of that cannot tell "eight independent
/// lanes" from "one lane copied eight times" — which is precisely the
/// distinction a voice-chunking refactor could break.
fn poly_pitches(e: &mut G, ctrl: NodeId, base_hz: f32) {
    for v in 0..crate::VOICES {
        e.apply(Cmd::SetParam {
            node: ctrl,
            param: v as u8,
            value: base_hz * (1.0 + 0.13 * v as f32),
        });
    }
}

/// CHARACTERIZATION golden: the basic poly chain, `PolyCtrl` → `PolyOsc` →
/// `VoiceSum` → bus.
///
/// Guards the voice-interleaved tile layout and the voice→mono collapse. In the
/// `simd` configuration `PolyOsc::process` is the `f32x8` fast path, so this
/// digest must hold identically in both configurations — that is what makes it
/// a usable gate for a change to how voices map onto SIMD lanes.
#[test]
fn golden_poly_osc_voicesum() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(0), 220.0);
    e.create(NodeId(1), Kind::PolyOsc);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
        node: NodeId(0),
        port: 0,
    };
    e.create(NodeId(2), Kind::VoiceSum);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
        node: NodeId(1),
        port: 0,
    };
    // VoiceSum gain: 8 voices would otherwise clip the [-1, 1] check.
    e.apply(Cmd::SetParam {
        node: NodeId(2),
        param: 0,
        value: 0.1,
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(2),
            port: 0,
        },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_OSC_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_OSC_DIGEST: u64 = 6_026_717_215_524_760_865;

/// CHARACTERIZATION golden: `PolyCtrl` → `PolyOsc` → `PolyMoogLp4` →
/// `VoiceSum`.
///
/// `PolyMoog` carries `[f32x8; POLES]` of per-voice ladder state, the most
/// structurally invasive of the four SIMD state layouts a voice-chunking change
/// has to rework. A ladder is also the least forgiving thing to get wrong: its
/// state is fed back per sample, so a lane that reads the wrong chunk does not
/// produce slightly wrong audio, it diverges.
#[test]
fn golden_poly_moog_lp4() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(0), 110.0);
    e.create(NodeId(1), Kind::PolyOsc);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
        node: NodeId(0),
        port: 0,
    };
    e.create(NodeId(2), Kind::PolyMoogLp4);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
        node: NodeId(1),
        port: 0,
    };
    // Trailing shared mono controls: cutoff, resonance. Resonance is set high
    // enough that the feedback path is doing real work — a near-zero-resonance
    // ladder is close to a plain cascade and would hide a feedback-state bug.
    *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(1200.0);
    *e.node_input_mut(NodeId(2), 2).unwrap() = Input::Const(0.7);
    e.create(NodeId(3), Kind::VoiceSum);
    *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node {
        node: NodeId(2),
        port: 0,
    };
    e.apply(Cmd::SetParam {
        node: NodeId(3),
        param: 0,
        value: 0.1,
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(3),
            port: 0,
        },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_MOOG_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_MOOG_DIGEST: u64 = 2_880_730_269_612_293_409;
