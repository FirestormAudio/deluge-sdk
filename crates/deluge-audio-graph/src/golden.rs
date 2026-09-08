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
//! Every golden is **target-invariant**: a digest must hold on both the 64-bit
//! host and the 32-bit ARM bucket. That is what makes them a usable gate.
//!
//! Most are also **config-invariant** — the same digest in the scalar and
//! `simd` configurations. Two are not, and the distinction is worth
//! understanding rather than working around. Some `f32x8` kernels are bit-exact
//! reimplementations of their scalar oracle; others reorder the arithmetic and
//! agree only to a tolerance. The kernels say which is which in their own test
//! names — `wavetable::simd_matches_scalar_within_tol` is explicit about it,
//! and `polysync_matches_scalar_oracle_all_waves` asserts `<= 1e-4`. A digest
//! is bit-exact by construction, so it sees a difference no audible measure
//! does: for both divergent kinds the two configurations render to the same
//! peak and an RMS agreeing to ~1e-6.
//!
//! Those two pin one constant per configuration rather than loosening to a
//! tolerance, so the gate keeps its full strength — a voice-chunking change
//! that misroutes a lane breaks the constant in whichever configuration it is
//! built. A `#[cfg]` pair on a digest means "this kernel reorders arithmetic
//! under SIMD", not "this test was hard to pin".

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

/// CHARACTERIZATION golden: `PolyCtrl` → `PolyOsc` → `PolyMs20Lp` →
/// `VoiceSum`.
///
/// `PolyMs20` holds four `f32x8` fields — `ic1`, `ic2`, `dc_x`, `dc_y` — all
/// `#[cfg(feature = "simd")]`-only; the scalar config keeps `[Ms20; VOICES]`
/// and already scales with the voice count. So this golden's real work is done
/// in the `simd` configuration, and the two configurations agreeing is the
/// assertion that matters.
#[test]
fn golden_poly_ms20_lp() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(0), 165.0);
    e.create(NodeId(1), Kind::PolyOsc);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
        node: NodeId(0),
        port: 0,
    };
    e.create(NodeId(2), Kind::PolyMs20Lp);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
        node: NodeId(1),
        port: 0,
    };
    *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(900.0);
    *e.node_input_mut(NodeId(2), 2).unwrap() = Input::Const(0.6);
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
    assert_eq!(crate::nrt::digest(&out), POLY_MS20_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_MS20_DIGEST: u64 = 16_487_278_233_059_771_593;

/// CHARACTERIZATION golden: two `PolyCtrl`s → `PolySyncSaw` → `VoiceSum`.
///
/// `PolySyncOsc` holds `master_phase` and `slave_phase` as `f32x8`. Hard sync
/// is driven by phase resets, so a lane that reads the wrong chunk resets at
/// the wrong instant — which is both clearly audible and a clean digest break,
/// making this the sharpest of the poly goldens.
///
/// Note both pitches are *poly edges*, not a mono ratio control: port 0 is the
/// master pitch tile and port 1 the slave pitch tile. Feeding port 1 a
/// `Const` would broadcast that value as a frequency in Hz to every lane, so
/// the slave gets its own `PolyCtrl` running at ~2.5× the master.
#[test]
fn golden_poly_sync_saw() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(0), 130.0);
    e.create(NodeId(1), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(1), 325.0); // 2.5× the master
    e.create(NodeId(2), Kind::PolySyncSaw);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
        node: NodeId(0),
        port: 0,
    };
    *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Node {
        node: NodeId(1),
        port: 0,
    };
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
    assert_eq!(crate::nrt::digest(&out), POLY_SYNC_DIGEST);
}

// Config-divergent (see the module doc). `PolySync` reorders the phase-reset
// arithmetic under SIMD; `polysync_matches_scalar_oracle_all_waves`
// (poly.rs:1719) asserts the two agree to `<= 1e-4`, and this patch renders to
// an identical peak (0.768475) and RMS (0.17343627) in both configurations.
/// Pinned 2026-09-08 (scalar path). See the module doc before regenerating.
#[cfg(not(feature = "simd"))]
const POLY_SYNC_DIGEST: u64 = 5_048_936_015_975_409_105;
/// Pinned 2026-09-08 (`f32x8` path). See the module doc before regenerating.
#[cfg(feature = "simd")]
const POLY_SYNC_DIGEST: u64 = 12_807_496_639_129_318_169;

/// CHARACTERIZATION golden: per-voice gating, `PolyCtrl` → `PolyOsc` ×
/// `PolyAdsr` → `VoiceSum`.
///
/// The other poly goldens drive every voice continuously, which cannot
/// distinguish "voice 3" from "lane 3 of a single wide register". This one
/// gates voices on at staggered times and releases half of them early, so the
/// digest depends on each voice index reaching the correct lane — the
/// allocator-side half of the voice/lane mapping, which a chunking change
/// touches from the opposite direction to the kernels.
#[test]
fn golden_poly_adsr_staggered_voices() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(0), 196.0);
    e.create(NodeId(1), Kind::PolyOsc);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
        node: NodeId(0),
        port: 0,
    };
    e.create(NodeId(2), Kind::PolyAdsr);
    // attack, decay, sustain — shared mono controls, short enough that 4096
    // frames covers the whole shape.
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Const(0.005);
    *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(0.05);
    *e.node_input_mut(NodeId(2), 2).unwrap() = Input::Const(0.6);
    e.create(NodeId(3), Kind::PolyMul);
    *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node {
        node: NodeId(1),
        port: 0,
    };
    *e.node_input_mut(NodeId(3), 1).unwrap() = Input::Node {
        node: NodeId(2),
        port: 0,
    };
    e.create(NodeId(4), Kind::VoiceSum);
    *e.node_input_mut(NodeId(4), 0).unwrap() = Input::Node {
        node: NodeId(3),
        port: 0,
    };
    e.apply(Cmd::SetParam {
        node: NodeId(4),
        param: 0,
        value: 0.15,
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(4),
            port: 0,
        },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    // Stagger the gates across the render so each voice sits at a distinct
    // envelope position — a chunking bug that swaps lanes changes the sum.
    for v in 0..crate::VOICES {
        e.apply_at(
            (v as u64) * 128,
            Cmd::GateVoice {
                node: NodeId(2),
                voice: v as u8,
                on: true,
            },
        );
    }
    // Release the even voices early, the odd ones not at all.
    for v in (0..crate::VOICES).step_by(2) {
        e.apply_at(
            2048 + (v as u64) * 64,
            Cmd::GateVoice {
                node: NodeId(2),
                voice: v as u8,
                on: false,
            },
        );
    }

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_ADSR_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_ADSR_DIGEST: u64 = 9_789_364_865_555_762_341;

/// CHARACTERIZATION golden: a static-table `Wavetable` oscillator stepped
/// across its mip pyramid.
///
/// `wavetable.rs` carries 37 `f32x8` sites, the largest SIMD surface outside
/// `poly.rs`, and its read path is index arithmetic over a flat static slice —
/// the kind of code that a 32-bit `usize` and a changed block size both
/// threaten. Stepping the pitch up makes the render cross mip levels, so the
/// digest covers level selection and not just one table.
#[test]
fn golden_wavetable_static_sweep() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::Wavetable);
    *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(110.0);
    e.apply(Cmd::BindTable {
        node: NodeId(0),
        src: crate::node::TableSrc::Static(crate::TableId(0)),
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(0),
            port: 0,
        },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    // Step the pitch up across the render so several mip levels are selected.
    for (i, hz) in [220.0f32, 440.0, 880.0, 1760.0, 3520.0].iter().enumerate() {
        e.apply_at(
            (i as u64 + 1) * 640,
            Cmd::SetInput {
                node: NodeId(0),
                port: 0,
                src: Input::Const(*hz),
            },
        );
    }

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), WAVETABLE_DIGEST);
}
// Config-divergent (see the module doc). The wavetable interpolation reorders
// under SIMD; the kernels name that outright in
// `wavetable::simd_matches_scalar_within_tol`. Both configurations render to
// an identical peak (1.0) and an RMS agreeing to ~1e-6 (0.5673494 scalar,
// 0.56734884 simd).
/// Pinned 2026-09-08 (scalar path). See the module doc before regenerating.
#[cfg(not(feature = "simd"))]
const WAVETABLE_DIGEST: u64 = 6_994_743_113_167_035_253;
/// Pinned 2026-09-08 (`f32x8` path). See the module doc before regenerating.
#[cfg(feature = "simd")]
const WAVETABLE_DIGEST: u64 = 18_147_409_328_667_502_341;

/// CHARACTERIZATION golden: two sources into two buses, one sending into the
/// other, through the full master chain.
///
/// Everything else in this module ends at a bare bus. This covers the parts a
/// rename of the aux-output surface touches, and the master DC-blocker → EQ →
/// limiter chain, plus the multi-bus summing that is how several instruments
/// play at once.
#[test]
fn golden_master_chain_two_buses() {
    let mut e = G::new(48_000.0);
    // Instrument A: saw → bus 0.
    e.create(NodeId(0), Kind::Saw);
    *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(110.0);
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(0),
            port: 0,
        },
        bus: BusId(0),
    });
    // Instrument B: square → bus 1.
    e.create(NodeId(1), Kind::Square);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Const(164.81);
    e.apply(Cmd::BusWrite {
        src: Input::Node {
            node: NodeId(1),
            port: 0,
        },
        bus: BusId(1),
    });
    // Distinct gains, so a bug that swaps the two buses changes the digest.
    e.apply(Cmd::BusGain {
        bus: BusId(0),
        gain: 0.4,
    });
    e.apply(Cmd::BusGain {
        bus: BusId(1),
        gain: 0.25,
    });
    // B sends into A, so the render covers the send path as well as the sum.
    // `from` must exceed `to` (see `Cmd::BusSend`).
    e.apply(Cmd::BusSend {
        from: BusId(1),
        to: BusId(0),
        gain: 0.3,
    });
    // The whole master chain, in its applied order: DC block, then EQ, then
    // the limiter.
    e.apply(Cmd::SetMasterDcBlock { cutoff_hz: 20.0 });
    e.apply(Cmd::SetMasterEq {
        freq: 1000.0,
        gain_db: 6.0,
        q: 0.707,
        eq_type: 1,
    });
    e.apply(Cmd::SetMasterLimit {
        ceiling: 0.9,
        release: 0.05,
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), MASTER_CHAIN_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const MASTER_CHAIN_DIGEST: u64 = 11_429_719_371_133_759_189;
