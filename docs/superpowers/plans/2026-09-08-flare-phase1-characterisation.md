# Flare Phase 1 — Characterisation Coverage — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the safety net that Phase 2 (the repo move) and Phase 3 (the
`VOICES`/`MAX_BLOCK` generalization) are gated on — characterisation goldens
covering the poly SIMD path, plus a test runner that actually exercises the
`simd` configuration.

**Architecture:** Every golden is an offline render of a fixed patch, pinned as
a single `u64` from `nrt::digest`. Goldens live in one new `#[cfg(test)] mod
golden` so later phases have exactly one file to watch. Nothing in `src/` outside
that module changes; this phase adds tests and moves one test file.

**Tech Stack:** Rust nightly (`rust-toolchain.toml`), `no_std`,
`core::simd` behind the non-default `simd` feature, `cargo test` across two
target buckets (QEMU ARM + host x86-64) driven by `tools/test.sh`.

**Spec:** `docs/superpowers/specs/2026-09-08-flare-audio-engine-extraction-design.md`
(this plan implements Phase 1, spec §4)

## Global Constraints

- **Licensing:** MIT / Apache-2.0 only. No GPL. (Spec §Background.)
- **`no_std`, no heap, no panics in DSP paths.** Bounded loops. `libm` for float
  maths, never `std` float methods.
- **The scalar path is the correctness oracle.** The `#[cfg(feature = "simd")]`
  `f32x8` path is null-tested against it lane-for-lane to ≤ 1e-4.
- **`simd` is default-off.** A plain `cargo test` is the scalar path.
- **Test per-crate, never `--workspace`.** Host tests need an explicit
  `--target x86_64-unknown-linux-gnu`; the workspace default target is
  `armv7a-none-eabihf`, which has no test harness.
- **Both target buckets are load-bearing, not redundant.** The deploy target is
  32-bit (`usize == u32`) and these crates are full of offset arithmetic; an
  overflow panics on device and cannot be reproduced on the 64-bit host. See
  `tools/test.sh` lines 44-49.
- **A characterisation golden is never "fixed" by re-pinning.** Re-pin only on an
  intended, reviewed output change, and say so in the comment with a date.

## Baseline (verified 2026-09-08, before any change)

| command | result |
|---|---|
| `cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph` | 313 passed |
| `cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph --features simd` | 313 passed |
| `cargo test --target x86_64-unknown-linux-gnu -p deluge-dsp-kernels` | 288 passed |
| `cargo test --target x86_64-unknown-linux-gnu -p deluge-dsp-kernels --features simd` | 286 passed (2 are scalar-only by cfg) |

Existing goldens, both in `crates/deluge-audio-graph/src/cmd.rs`:
`golden_saw_lpf_env_first_block` (`cmd.rs:416`, 8 samples pinned literally) and
`golden_saw_lpf_env_offline_digest` (`cmd.rs:508`, 4096 frames, `GOLDEN_DIGEST =
4_722_302_078_756_468_769` at `cmd.rs:578`).

## File Structure

| file | responsibility |
|---|---|
| `tools/test.sh` (modify) | add the `simd` configuration to both buckets |
| `crates/deluge-audio-graph/src/golden.rs` (create) | **every** characterisation golden, and the shared patch-building helpers |
| `crates/deluge-audio-graph/src/lib.rs` (modify) | declare `#[cfg(test)] mod golden;` |
| `crates/deluge-audio-graph/src/cmd.rs` (modify) | the two existing goldens move out |
| `crates/deluge-fft/tests/dsp_pipeline.rs` (delete) | relocated |
| `crates/deluge-fixedpoint/tests/dsp_pipeline.rs` (create) | same test, on the side of the seam where both halves live |
| `crates/deluge-fft/Cargo.toml`, `crates/deluge-fixedpoint/Cargo.toml` (modify) | move the dev-dependencies with the test |

Goldens get their own module rather than accreting in `cmd.rs` (already 599
lines and about command transport, not characterisation) so Phase 2's
"any digest change is a bug" gate has one file to watch.

---

### Task 1: Test the `simd` configuration at all

The `simd` feature is currently exercised by **nothing** — `tools/test.sh` never
passes `--features simd` and there is no CI workflow that does. The scalar==simd
equivalence tests inside the kernels only run if a developer invokes them by
hand. Phase 3c refactors that SIMD code; it cannot be gated by a configuration
nobody builds. This task is therefore a prerequisite for the whole spec, not a
nicety.

**Files:**
- Modify: `tools/test.sh:50-52` (QEMU bucket) and `tools/test.sh:68-70` (host bucket)

**Interfaces:**
- Consumes: nothing.
- Produces: `tools/test.sh` runs each audio crate in both configurations. Every
  later task's verification step is "run `tools/test.sh`", which from here on
  means both configs.

- [ ] **Step 1: Confirm the gap is real**

Run:
```bash
grep -c 'features simd' tools/test.sh
ls .github/workflows/ && grep -rn 'simd' .github/workflows/*.yml
```
Expected: `0` from the grep, and no `simd` match in any workflow. If either
finds something, stop and re-read — the premise of this task has changed.

- [ ] **Step 2: Confirm both configs pass today, so this task adds coverage rather than debt**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-dsp-kernels --features simd 2>&1 | tail -3
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph --features simd 2>&1 | grep '^test result'
```
Expected: `286 passed; 0 failed` and `313 passed; 0 failed`. If anything fails,
**stop and report** — a pre-existing simd failure is a finding that changes the
plan, not something to fix silently inside this task.

- [ ] **Step 3: Add the simd runs to the QEMU bucket**

In `tools/test.sh`, replace these two lines (currently at 50-51):

```bash
cargo test --target "$QEMU" -p deluge-dsp-kernels
cargo test --target "$QEMU" -p deluge-audio-graph
```

with:

```bash
cargo test --target "$QEMU" -p deluge-dsp-kernels
cargo test --target "$QEMU" -p deluge-audio-graph
# Again with NEON. `simd` is default-OFF, so the runs above are the scalar
# oracle and these are the f32x8 fast path. Both are required: the kernels'
# scalar==simd equivalence tests only mean something if the simd config is
# actually built, and Phase 3 of the flare extraction refactors this code.
cargo test --target "$QEMU" -p deluge-dsp-kernels --features simd
cargo test --target "$QEMU" -p deluge-audio-graph --features simd
```

- [ ] **Step 4: Add the simd runs to the host bucket**

In `tools/test.sh`, after the existing host-bucket lines (currently at 68-69):

```bash
cargo test --target "$HOST" -p deluge-dsp-kernels
cargo test --target "$HOST" -p deluge-audio-graph
```

add:

```bash
# The portable-SIMD path off ARM: `core::simd` lowers to SSE/AVX here, so this
# also proves the f32x8 code is not secretly NEON-specific.
cargo test --target "$HOST" -p deluge-dsp-kernels --features simd
cargo test --target "$HOST" -p deluge-audio-graph --features simd
```

- [ ] **Step 5: Run the full runner and verify it passes**

Run: `tools/test.sh`
Expected: every invocation passes. The four new lines appear in the output with
the same pass counts as Step 2 (host) and their QEMU equivalents.

If the QEMU bucket cannot run locally (missing `qemu-arm` or the cross linker —
see the prerequisites at `tools/test.sh:17-21`), run the host bucket lines
directly and **say so in the commit message**; do not delete the QEMU lines.

- [ ] **Step 6: Commit**

```bash
git add tools/test.sh
git commit -m "test: run the audio crates in the simd configuration too

The \`simd\` feature was exercised by nothing — tools/test.sh never passed
\`--features simd\` and no workflow did either, so the kernels' scalar==simd
equivalence tests only ran when someone remembered to invoke them by hand.
That is a thin place to stand before refactoring the f32x8 poly code.

Both configs pass today (kernels 288 scalar / 286 simd, graph 313 both), so
this pins existing behaviour rather than fixing anything."
```

---

### Task 2: Give the goldens one home

Move the two existing goldens into a dedicated module. This is a **pure move**:
the digest constant does not change, which is what proves the move was inert.

**Files:**
- Create: `crates/deluge-audio-graph/src/golden.rs`
- Modify: `crates/deluge-audio-graph/src/lib.rs` (add the `mod` declaration)
- Modify: `crates/deluge-audio-graph/src/cmd.rs` (remove `golden_saw_lpf_env_first_block`, `golden_saw_lpf_env_offline_digest`, and `GOLDEN_DIGEST`)

**Interfaces:**
- Consumes: `Engine`, `Cmd`, `Kind`, `Input`, `NodeId`, `BusId`, `StereoFrame`,
  `nrt::{digest, is_clean, peak}` — all already `pub`.
- Produces: `crates/deluge-audio-graph/src/golden.rs` containing
  `type G = Engine<64, 16, 64, 4, 45056, 2048>;` and
  `fn render_4096(e: &mut G) -> [StereoFrame; 4096]`. Every later task adds its
  golden to this file and uses that alias and helper.

Note the engine alias differs from `cmd.rs`'s `type E = Engine<16, 8, 8, 4,
45056, 2048>`: the poly patches in later tasks need more nodes and output slots
than `E` provides. `BLOCK = 64` and the pool parameters match the existing poly
tests in `engine.rs:2100`.

- [ ] **Step 1: Create the module with the helper and the two moved goldens**

Create `crates/deluge-audio-graph/src/golden.rs`:

```rust
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

/// The engine shape every golden uses. Wider than `cmd.rs`'s `E` because the
/// poly patches need the extra nodes and output slots.
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
            Input::Node { node: NodeId(0), port: 0 },
            Input::Const(800.0),
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::NewNode {
        node: NodeId(2),
        kind: Kind::Env,
        args: [Input::Const(0.01), Input::Const(0.1), Input::Const(0.0)],
    });
    e.apply(Cmd::Gate { node: NodeId(2), on: true });
    e.apply(Cmd::NewNode {
        node: NodeId(3),
        kind: Kind::Mul,
        args: [
            Input::Node { node: NodeId(1), port: 0 },
            Input::Node { node: NodeId(2), port: 0 },
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(3), port: 0 },
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
/// `golden_saw_lpf_env_first_block`, rendered offline for 4096 frames and
/// pinned as one digest. Released partway through, so the decay is covered.
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
            Input::Node { node: NodeId(0), port: 0 },
            Input::Const(800.0),
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::NewNode {
        node: NodeId(2),
        kind: Kind::Env,
        args: [Input::Const(0.01), Input::Const(0.1), Input::Const(0.0)],
    });
    e.apply(Cmd::Gate { node: NodeId(2), on: true });
    e.apply(Cmd::NewNode {
        node: NodeId(3),
        kind: Kind::Mul,
        args: [
            Input::Node { node: NodeId(1), port: 0 },
            Input::Node { node: NodeId(2), port: 0 },
            Input::Const(0.0),
        ],
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(3), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });
    e.apply_at(2048, Cmd::Gate { node: NodeId(2), on: false });

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
```

- [ ] **Step 2: Declare the module**

In `crates/deluge-audio-graph/src/lib.rs`, after the existing `pub mod frame;`
line, add:

```rust
#[cfg(test)]
mod golden;
```

It is not `pub` and not re-exported: goldens are tests, not API.

- [ ] **Step 3: Delete the originals from `cmd.rs`**

In `crates/deluge-audio-graph/src/cmd.rs`, delete `golden_saw_lpf_env_first_block`
(starting at the `/// CHARACTERIZATION golden (spec §8 P0 testing gate)` doc
comment, ~line 407), `golden_saw_lpf_env_offline_digest`, and the
`const GOLDEN_DIGEST: u64 = 4_722_302_078_756_468_769;` line that follows them.

Leave everything else in that test module alone — in particular
`saw_lpf_env_parity_first_samples` and `cmds_are_comparable_and_debuggable`
stay.

- [ ] **Step 4: Verify the move was inert**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden 2>&1 | grep -E '^test golden|^test result'
```
Expected: `golden::golden_saw_lpf_env_first_block ... ok` and
`golden::golden_saw_lpf_env_offline_digest ... ok`.

The digest constant was carried across unchanged, so a pass here proves the move
did not disturb the patch. **If the digest test fails, the move was not inert —
do not re-pin it; find what changed.**

- [ ] **Step 5: Verify the whole crate, both configs**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph 2>&1 | grep '^test result'
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph --features simd 2>&1 | grep '^test result'
```
Expected: `313 passed; 0 failed` from both — the same count as the baseline,
because tests moved rather than appeared.

- [ ] **Step 6: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs crates/deluge-audio-graph/src/lib.rs crates/deluge-audio-graph/src/cmd.rs
git commit -m "test(audio-graph): collect the characterisation goldens into one module

cmd.rs is about command transport and was already 599 lines; the goldens were
lodged in its test module because that is where the first one happened to be
written. The flare extraction's Phase 2 gate is 'every golden digest
bit-identical', which wants one file to watch rather than a scatter.

A pure move: the pinned digest is carried across unchanged, so the tests
passing is what proves nothing was disturbed. 313 tests before and after."
```

---

### Task 3: Golden for the poly voice path

The first of four goldens covering the code Phase 3c refactors. This one pins
the basic poly chain — `PolyCtrl` → `PolyOsc` → `VoiceSum` — which exercises
voice-interleaved tile layout (`tile[i * VOICES + v]`) and the voice→mono
collapse. In the `simd` configuration `PolyOsc::process` is the `f32x8` path.

**Files:**
- Modify: `crates/deluge-audio-graph/src/golden.rs` (append)

**Interfaces:**
- Consumes: `type G`, `fn render_4096` from Task 2. `deluge_dsp_kernels::poly::VOICES`,
  re-exported as `crate::VOICES`.
- Produces: `fn poly_pitches(e: &mut G, ctrl: NodeId, base_hz: f32)` — sets every
  voice of a `PolyCtrl` node to a distinct frequency. Tasks 4, 5 and 6 use it.

- [ ] **Step 1: Write the golden with a deliberately wrong digest**

Append to `crates/deluge-audio-graph/src/golden.rs`:

```rust
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
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node { node: NodeId(0), port: 0 };
    e.create(NodeId(2), Kind::VoiceSum);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node { node: NodeId(1), port: 0 };
    // VoiceSum gain: 8 in-phase voices would otherwise clip the [-1, 1] check.
    e.apply(Cmd::SetParam { node: NodeId(2), param: 0, value: 0.1 });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(2), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_OSC_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_OSC_DIGEST: u64 = 0;
```

The `0` is deliberate: it makes the test fail so you can see the real value,
which is the only way to obtain a characterisation constant.

- [ ] **Step 2: Run it and read the real digest**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_poly_osc_voicesum -- --nocapture
```
Expected: FAIL on the `assert_eq!`, reporting `left: <some large u64>, right: 0`.

**Before pinning that number, check the two assertions inside `render_4096`
passed** — they run before the digest comparison, so reaching the `assert_eq!`
at all means the output was finite, in range, and not silent. If instead the
failure is "output is silent" or the `is_clean` assert, the patch is wrong;
fix the patch, do not pin.

- [ ] **Step 3: Pin the observed digest**

Replace `const POLY_OSC_DIGEST: u64 = 0;` with the `left:` value from Step 2,
written with `_` separators, e.g.:

```rust
const POLY_OSC_DIGEST: u64 = 12_345_678_901_234_567_890;
```

- [ ] **Step 4: Verify it passes, and passes identically under simd**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_poly_osc_voicesum
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph --features simd golden_poly_osc_voicesum
```
Expected: PASS from both, against the same constant.

**If the simd run produces a different digest, stop and report it.** That is a
real finding — it means the `f32x8` path is not bit-identical to the scalar
oracle, and the whole gating strategy for Phase 3 depends on knowing that before
the refactor rather than during it. Do not paper over it with a
config-conditional constant without saying so.

- [ ] **Step 5: Verify on the 32-bit ARM bucket**

Run:
```bash
cargo test --target armv7-unknown-linux-gnueabihf -p deluge-audio-graph golden_poly_osc_voicesum
cargo test --target armv7-unknown-linux-gnueabihf -p deluge-audio-graph --features simd golden_poly_osc_voicesum
```
Expected: PASS from both, same constant. This is the NEON path and the 32-bit
`usize` path.

If QEMU is unavailable locally, note it in the commit message rather than
skipping silently.

- [ ] **Step 6: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs
git commit -m "test(audio-graph): pin the poly osc → voicesum path

First of four goldens covering the code the flare extraction's Phase 3c
refactors (spec §2c). Pins the voice-interleaved tile layout and the
voice→mono collapse.

Voices are given distinct frequencies deliberately: identical voices sum to
exactly 8x one voice, and a digest of that cannot distinguish eight
independent lanes from one lane copied eight times — which is exactly what a
voice-chunking bug would look like."
```

---

### Task 4: Golden for `PolyMoog`

`PolyMoog` holds `state: [f32x8; POLES]` (`poly.rs:517`) — under Phase 3c that
becomes `[[f32x8; VOICE_CHUNKS]; POLES]`. It is the most structurally invasive
of the four changes, so it gets the most direct golden.

**Files:**
- Modify: `crates/deluge-audio-graph/src/golden.rs` (append)

**Interfaces:**
- Consumes: `type G`, `render_4096`, `poly_pitches` from Tasks 2-3.
- Produces: nothing new.

- [ ] **Step 1: Write the golden with a deliberately wrong digest**

Append to `crates/deluge-audio-graph/src/golden.rs`:

```rust
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
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node { node: NodeId(0), port: 0 };
    e.create(NodeId(2), Kind::PolyMoogLp4);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node { node: NodeId(1), port: 0 };
    // Trailing shared mono controls: cutoff, resonance. Resonance is set high
    // enough that the feedback path is doing real work — a near-zero-resonance
    // ladder is close to a plain cascade and would hide a feedback-state bug.
    *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(1200.0);
    *e.node_input_mut(NodeId(2), 2).unwrap() = Input::Const(0.7);
    e.create(NodeId(3), Kind::VoiceSum);
    *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node { node: NodeId(2), port: 0 };
    e.apply(Cmd::SetParam { node: NodeId(3), param: 0, value: 0.1 });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(3), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_MOOG_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_MOOG_DIGEST: u64 = 0;
```

- [ ] **Step 2: Run it and read the real digest**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_poly_moog_lp4 -- --nocapture
```
Expected: FAIL on the `assert_eq!`, reporting the real `left:` value.

If it instead fails on "output is silent", the cutoff is filtering everything
away — check that port 1 is cutoff and port 2 is resonance for this kind by
reading the `Kind::PolyMoogLp4` arm in `crates/deluge-audio-graph/src/node.rs`,
and fix the patch rather than the assertion.

- [ ] **Step 3: Pin the observed digest**

Replace `const POLY_MOOG_DIGEST: u64 = 0;` with the observed value, `_`-separated.

- [ ] **Step 4: Verify both configs and both targets**

Run:
```bash
for f in "" "--features simd"; do
  for t in x86_64-unknown-linux-gnu armv7-unknown-linux-gnueabihf; do
    echo "== $t $f"; cargo test --target $t -p deluge-audio-graph $f golden_poly_moog_lp4 2>&1 | grep '^test result'
  done
done
```
Expected: four passes against one constant. A simd/scalar divergence is a
finding to report, not to work around.

- [ ] **Step 5: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs
git commit -m "test(audio-graph): pin the poly Moog ladder

PolyMoog carries [f32x8; POLES] of per-voice ladder state (poly.rs:517), the
most structurally invasive of the four SIMD state layouts that the flare
extraction's voice-chunking change has to rework (spec §2c).

Resonance is set to 0.7 rather than left near zero so the feedback path is
actually exercised: a ladder with no resonance is close to a plain cascade and
would hide exactly the class of bug this guards against."
```

---

### Task 5: Golden for `PolyMs20`

`PolyMs20` holds four `f32x8` fields — `ic1`, `ic2`, `dc_x`, `dc_y`
(`poly.rs:595-601`), all `#[cfg(feature = "simd")]`-only — which Phase 3c turns
into `[f32x8; VOICE_CHUNKS]`. Its scalar config uses `voices: [Ms20; VOICES]`
and already scales, so this golden's real job is to hold the **simd** path
still.

**Files:**
- Modify: `crates/deluge-audio-graph/src/golden.rs` (append)

**Interfaces:**
- Consumes: `type G`, `render_4096`, `poly_pitches`.
- Produces: nothing new.

- [ ] **Step 1: Write the golden with a deliberately wrong digest**

Append to `crates/deluge-audio-graph/src/golden.rs`:

```rust
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
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node { node: NodeId(0), port: 0 };
    e.create(NodeId(2), Kind::PolyMs20Lp);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node { node: NodeId(1), port: 0 };
    *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(900.0);
    *e.node_input_mut(NodeId(2), 2).unwrap() = Input::Const(0.6);
    e.create(NodeId(3), Kind::VoiceSum);
    *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node { node: NodeId(2), port: 0 };
    e.apply(Cmd::SetParam { node: NodeId(3), param: 0, value: 0.1 });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(3), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_MS20_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_MS20_DIGEST: u64 = 0;
```

- [ ] **Step 2: Run it and read the real digest**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_poly_ms20_lp -- --nocapture
```
Expected: FAIL on the `assert_eq!`, reporting the real value.

- [ ] **Step 3: Pin the observed digest**

Replace the `0` with the observed value, `_`-separated.

- [ ] **Step 4: Verify both configs and both targets**

Run:
```bash
for f in "" "--features simd"; do
  for t in x86_64-unknown-linux-gnu armv7-unknown-linux-gnueabihf; do
    echo "== $t $f"; cargo test --target $t -p deluge-audio-graph $f golden_poly_ms20_lp 2>&1 | grep '^test result'
  done
done
```
Expected: four passes against one constant.

Because `PolyMs20`'s scalar and simd implementations are genuinely different
code (separate `new()` bodies at `poly.rs:606` and `poly.rs:614`), a divergence
here is more likely than elsewhere. If it happens, report the two digests and
the size of the audible difference (`nrt::rms` of the two renders) rather than
silently splitting the constant.

- [ ] **Step 5: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs
git commit -m "test(audio-graph): pin the poly MS-20 filter

PolyMs20 holds four f32x8 fields (ic1, ic2, dc_x, dc_y), all simd-only, which
the flare extraction's voice-chunking change turns into [f32x8; VOICE_CHUNKS]
(spec §2c). Its scalar config keeps [Ms20; VOICES] and already scales, so the
load-bearing assertion here is that the two configurations agree."
```

---

### Task 6: Golden for `PolySyncOsc`

`PolySyncOsc` holds `master_phase` and `slave_phase` as `f32x8`
(`poly.rs:723`, `:725`). Hard sync is phase-reset-driven, so a lane reading the
wrong chunk shows up as a wrong reset instant — audible, and a clean digest
break.

**Files:**
- Modify: `crates/deluge-audio-graph/src/golden.rs` (append)

**Interfaces:**
- Consumes: `type G`, `render_4096`, `poly_pitches`.
- Produces: nothing new.

- [ ] **Step 1: Write the golden with a deliberately wrong digest**

Append to `crates/deluge-audio-graph/src/golden.rs`:

```rust
/// CHARACTERIZATION golden: `PolyCtrl` → `PolySyncSaw` → `VoiceSum`.
///
/// `PolySyncOsc` holds `master_phase` and `slave_phase` as `f32x8`. Hard sync
/// is driven by phase resets, so a lane that reads the wrong chunk resets at
/// the wrong instant — which is both clearly audible and a clean digest break,
/// making this the sharpest of the four poly goldens.
#[test]
fn golden_poly_sync_saw() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(0), 130.0);
    e.create(NodeId(1), Kind::PolySyncSaw);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node { node: NodeId(0), port: 0 };
    // Slave ratio above 1 so the slave actually gets reset mid-cycle; a ratio
    // of 1 makes sync a no-op and the golden would guard nothing.
    *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(2.5);
    e.create(NodeId(2), Kind::VoiceSum);
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node { node: NodeId(1), port: 0 };
    e.apply(Cmd::SetParam { node: NodeId(2), param: 0, value: 0.1 });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(2), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_SYNC_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_SYNC_DIGEST: u64 = 0;
```

- [ ] **Step 2: Confirm the port map before running**

Run:
```bash
grep -n -A12 'Kind::PolySyncSine' crates/deluge-audio-graph/src/node.rs | head -30
```
Read which port carries the sync ratio. If it is not port 1, correct the patch
above to match the code — the plan's port numbering is a claim about the source,
and the source wins.

- [ ] **Step 3: Run it and read the real digest**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_poly_sync_saw -- --nocapture
```
Expected: FAIL on the `assert_eq!`, reporting the real value.

- [ ] **Step 4: Pin the observed digest**

Replace the `0` with the observed value, `_`-separated.

- [ ] **Step 5: Verify both configs and both targets**

Run:
```bash
for f in "" "--features simd"; do
  for t in x86_64-unknown-linux-gnu armv7-unknown-linux-gnueabihf; do
    echo "== $t $f"; cargo test --target $t -p deluge-audio-graph $f golden_poly_sync_saw 2>&1 | grep '^test result'
  done
done
```
Expected: four passes against one constant.

- [ ] **Step 6: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs
git commit -m "test(audio-graph): pin the poly hard-sync oscillator

PolySyncOsc holds master_phase and slave_phase as f32x8, the last of the four
SIMD state layouts the flare extraction's voice-chunking change reworks (spec
§2c). Sync is reset-driven, so a lane reading the wrong chunk resets at the
wrong instant — the sharpest digest break of the four."
```

---

### Task 7: Golden for per-voice gating through `PolyAdsr`

The four goldens so far drive every voice continuously. None covers the
**voice-addressed** commands — `Cmd::GateVoice` / `Cmd::TriggerVoice` — which
are the other place a voice index maps onto a lane, and which a chunking change
touches from the opposite direction (allocator side rather than kernel side).

**Files:**
- Modify: `crates/deluge-audio-graph/src/golden.rs` (append)

**Interfaces:**
- Consumes: `type G`, `render_4096`, `poly_pitches`.
- Produces: nothing new.

- [ ] **Step 1: Write the golden with a deliberately wrong digest**

Append to `crates/deluge-audio-graph/src/golden.rs`:

```rust
/// CHARACTERIZATION golden: per-voice gating, `PolyCtrl` → `PolyOsc` ×
/// `PolyAdsr` → `VoiceSum`.
///
/// The other four poly goldens drive every voice continuously, which cannot
/// distinguish "voice 3" from "lane 3 of a single wide register". This one
/// gates voices on at staggered times and releases half of them early, so the
/// digest depends on each voice index reaching the correct lane — the
/// allocator-side half of the voice/lane mapping.
#[test]
fn golden_poly_adsr_staggered_voices() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::PolyCtrl);
    poly_pitches(&mut e, NodeId(0), 196.0);
    e.create(NodeId(1), Kind::PolyOsc);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node { node: NodeId(0), port: 0 };
    e.create(NodeId(2), Kind::PolyAdsr);
    // attack, decay, sustain — short enough that 4096 frames covers the shape.
    *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Const(0.005);
    *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(0.05);
    *e.node_input_mut(NodeId(2), 2).unwrap() = Input::Const(0.6);
    e.create(NodeId(3), Kind::PolyMul);
    *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node { node: NodeId(1), port: 0 };
    *e.node_input_mut(NodeId(3), 1).unwrap() = Input::Node { node: NodeId(2), port: 0 };
    e.create(NodeId(4), Kind::VoiceSum);
    *e.node_input_mut(NodeId(4), 0).unwrap() = Input::Node { node: NodeId(3), port: 0 };
    e.apply(Cmd::SetParam { node: NodeId(4), param: 0, value: 0.15 });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(4), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    // Stagger the gates across the render so each voice has a distinct
    // envelope position — a chunking bug that swaps lanes changes the sum.
    for v in 0..crate::VOICES {
        e.apply_at(
            (v as u64) * 128,
            Cmd::GateVoice { node: NodeId(2), voice: v as u8, on: true },
        );
    }
    // Release the even voices early, the odd ones late.
    for v in (0..crate::VOICES).step_by(2) {
        e.apply_at(
            2048 + (v as u64) * 64,
            Cmd::GateVoice { node: NodeId(2), voice: v as u8, on: false },
        );
    }

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), POLY_ADSR_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const POLY_ADSR_DIGEST: u64 = 0;
```

- [ ] **Step 2: Run it and read the real digest**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_poly_adsr_staggered_voices -- --nocapture
```
Expected: FAIL on the `assert_eq!`, reporting the real value.

If it fails on "output is silent", check that `Kind::PolyAdsr`'s port map is
attack/decay/sustain in ports 0-2 and that `Kind::PolyMul` takes two poly
inputs, by reading their arms in `crates/deluge-audio-graph/src/node.rs`. Fix
the patch, not the assertion.

- [ ] **Step 3: Pin the observed digest**

Replace the `0` with the observed value, `_`-separated.

- [ ] **Step 4: Prove the golden actually discriminates between voices**

This is the one golden whose value depends on a claim worth testing directly:
that swapping two voices changes the output. Temporarily change the release
loop's `step_by(2)` to `skip(1).step_by(2)` (releasing the odd voices instead of
the even ones), and run:

```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_poly_adsr_staggered_voices -- --nocapture
```
Expected: FAIL with a **different** digest from the one just pinned.

Then **revert the change** and confirm it passes again. If the digest did not
move, the patch is not voice-discriminating and the golden is weaker than it
looks — say so rather than proceeding.

- [ ] **Step 5: Verify both configs and both targets**

Run:
```bash
for f in "" "--features simd"; do
  for t in x86_64-unknown-linux-gnu armv7-unknown-linux-gnueabihf; do
    echo "== $t $f"; cargo test --target $t -p deluge-audio-graph $f golden_poly_adsr_staggered_voices 2>&1 | grep '^test result'
  done
done
```
Expected: four passes against one constant.

- [ ] **Step 6: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs
git commit -m "test(audio-graph): pin staggered per-voice gating

The other poly goldens drive every voice continuously, which cannot tell
'voice 3' from 'lane 3 of one wide register'. This one gates voices on at
staggered times and releases half early, so the digest depends on each voice
index reaching the right lane — the allocator-side half of the voice/lane
mapping that the flare extraction's chunking change touches (spec §2c).

Verified discriminating: releasing the odd voices instead of the even ones
moves the digest."
```

---

### Task 8: Golden for the wavetable path

`wavetable.rs` has 37 `f32x8` sites — the second-largest SIMD surface after
`poly.rs` — and the mip-pyramid read path is index arithmetic over a flat
`&'static [f32]`, which is exactly the kind of code a 32-bit `usize` and a block
size change both threaten.

**Files:**
- Modify: `crates/deluge-audio-graph/src/golden.rs` (append)

**Interfaces:**
- Consumes: `type G`, `render_4096`.
- Produces: nothing new.

- [ ] **Step 1: Write the golden with a deliberately wrong digest**

Append to `crates/deluge-audio-graph/src/golden.rs`:

```rust
/// CHARACTERIZATION golden: a static-table `Wavetable` oscillator swept across
/// its mip pyramid.
///
/// `wavetable.rs` carries 37 `f32x8` sites, the largest SIMD surface outside
/// `poly.rs`, and its read path is index arithmetic over a flat static slice —
/// the kind of code that a 32-bit `usize` and a changed block size both
/// threaten. Sweeping the pitch makes the render cross mip levels, so the
/// digest covers level selection and not just one table.
#[test]
fn golden_wavetable_static_sweep() {
    let mut e = G::new(48_000.0);
    e.create(NodeId(0), Kind::Wavetable);
    *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(110.0);
    e.apply(Cmd::BindTable {
        node: NodeId(0),
        src: crate::node::TableSrc::Static(deluge_dsp_kernels::wavetable::TableId(0)),
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(0), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    // Step the pitch up across the render so several mip levels are selected.
    for (i, hz) in [220.0f32, 440.0, 880.0, 1760.0, 3520.0].iter().enumerate() {
        e.apply_at(
            (i as u64 + 1) * 640,
            Cmd::SetInput { node: NodeId(0), port: 0, src: Input::Const(*hz) },
        );
    }

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), WAVETABLE_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const WAVETABLE_DIGEST: u64 = 0;
```

- [ ] **Step 2: Add the dev-dependency needed to name `TableId`**

The golden names `deluge_dsp_kernels::wavetable::TableId`. That crate is already
a normal dependency of `deluge-audio-graph`
(`crates/deluge-audio-graph/Cargo.toml:14`), so **no manifest change should be
needed**. Confirm by building the tests:

```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph --no-run 2>&1 | tail -5
```
Expected: compiles. If it cannot resolve the path, use the crate's own
re-export `crate::TableId` (`lib.rs:39`) instead.

- [ ] **Step 3: Run it and read the real digest**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_wavetable_static_sweep -- --nocapture
```
Expected: FAIL on the `assert_eq!`, reporting the real value.

- [ ] **Step 4: Pin the observed digest**

Replace the `0` with the observed value, `_`-separated.

- [ ] **Step 5: Verify both configs and both targets**

Run:
```bash
for f in "" "--features simd"; do
  for t in x86_64-unknown-linux-gnu armv7-unknown-linux-gnueabihf; do
    echo "== $t $f"; cargo test --target $t -p deluge-audio-graph $f golden_wavetable_static_sweep 2>&1 | grep '^test result'
  done
done
```
Expected: four passes against one constant. The ARM run is the one that matters
most here — it is the 32-bit `usize` path through the pyramid offset arithmetic.

- [ ] **Step 6: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs
git commit -m "test(audio-graph): pin the static wavetable path across mip levels

wavetable.rs has 37 f32x8 sites, the largest SIMD surface outside poly.rs, and
its read path is index arithmetic over a flat static slice — threatened both by
the 32-bit usize on device and by the configurable MAX_BLOCK the flare
extraction adds (spec §2b). The pitch is stepped across the render so the
digest covers mip-level selection, not just one table."
```

---

### Task 9: Golden for the master chain and effects

The goldens so far all end at a bus. None covers the master chain — bus gains,
sends, the limiter, DC blocker and EQ — which is where `AUX_OUTS` and the
`Cmd::SetUsbOut` → `Cmd::SetAuxOut` rename land in Phase 3a.

**Files:**
- Modify: `crates/deluge-audio-graph/src/golden.rs` (append)

**Interfaces:**
- Consumes: `type G`, `render_4096`.
- Produces: nothing new.

- [ ] **Step 1: Confirm the master-chain command shapes**

The exact field names matter and the plan should not be trusted over the source.
Run:
```bash
sed -n '57,115p' crates/deluge-audio-graph/src/cmd.rs
```
Read the shapes of `BusWrite`, `BusGain`, `BusSend`, `SetMasterLimit`,
`SetMasterDcBlock` and `SetMasterEq`, and adjust Step 2's patch to match.

- [ ] **Step 2: Write the golden with a deliberately wrong digest**

Append to `crates/deluge-audio-graph/src/golden.rs`, adjusting the master-chain
commands to the shapes read in Step 1:

```rust
/// CHARACTERIZATION golden: two sources into two buses, one sending into the
/// other, through the master chain.
///
/// Everything else in this module ends at a bare bus. This covers the parts the
/// flare extraction's Phase 3a renames touch — bus gain, bus send, and the
/// master limiter / DC blocker / EQ — plus the multi-bus summing that makes
/// "several instruments at once" work (spec §3).
#[test]
fn golden_master_chain_two_buses() {
    let mut e = G::new(48_000.0);
    // Instrument A: saw → bus 0.
    e.create(NodeId(0), Kind::Saw);
    *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(110.0);
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(0), port: 0 },
        bus: BusId(0),
    });
    // Instrument B: square → bus 1.
    e.create(NodeId(1), Kind::Square);
    *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Const(164.81);
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(1), port: 0 },
        bus: BusId(1),
    });
    // Distinct gains, so a bug that swaps the two buses changes the digest.
    e.apply(Cmd::BusGain { bus: BusId(0), gain: 0.4 });
    e.apply(Cmd::BusGain { bus: BusId(1), gain: 0.25 });
    // B sends into A, so the render covers the send path as well as the sum.
    e.apply(Cmd::BusSend { from: BusId(1), to: BusId(0), gain: 0.3 });
    e.apply(Cmd::SetMasterDcBlock { on: true });
    e.apply(Cmd::SetMasterLimit { on: true });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let out = render_4096(&mut e);
    assert_eq!(crate::nrt::digest(&out), MASTER_CHAIN_DIGEST);
}
/// Pinned 2026-09-08. See the module doc before regenerating.
const MASTER_CHAIN_DIGEST: u64 = 0;
```

- [ ] **Step 3: Run it and read the real digest**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden_master_chain_two_buses -- --nocapture
```
Expected: FAIL on the `assert_eq!`, reporting the real value.

If it fails to compile, the command shapes differ from Step 1's reading — fix
the patch against the source.

- [ ] **Step 4: Pin the observed digest**

Replace the `0` with the observed value, `_`-separated.

- [ ] **Step 5: Verify both configs and both targets**

Run:
```bash
for f in "" "--features simd"; do
  for t in x86_64-unknown-linux-gnu armv7-unknown-linux-gnueabihf; do
    echo "== $t $f"; cargo test --target $t -p deluge-audio-graph $f golden_master_chain_two_buses 2>&1 | grep '^test result'
  done
done
```
Expected: four passes against one constant.

- [ ] **Step 6: Commit**

```bash
git add crates/deluge-audio-graph/src/golden.rs
git commit -m "test(audio-graph): pin the master chain and multi-bus summing

Every other golden ends at a bare bus. This one covers bus gain, bus send and
the master limiter/DC-blocker — the code the flare extraction's aux-output
rename touches (spec §2a) — and the multi-bus summing that is how several
instruments play at once (spec §3).

Bus gains differ deliberately so a bug that swaps the two buses moves the
digest."
```

---

### Task 10: Relocate the cross-crate FFT pipeline test

`crates/deluge-fft/tests/dsp_pipeline.rs` dev-depends on `fixedpoint` and
`armv7-dsp-intrinsics`, which stay in `deluge-sdk` when `deluge-fft` becomes
`flare-fft`. Left where it is, it would make flare depend backwards on the SDK —
the one thing the seam forbids (spec §1). It moves to `deluge-fixedpoint`, where
both halves of the pipeline live.

**Files:**
- Delete: `crates/deluge-fft/tests/dsp_pipeline.rs`
- Create: `crates/deluge-fixedpoint/tests/dsp_pipeline.rs`
- Modify: `crates/deluge-fft/Cargo.toml:28-32` (drop the two dev-dependencies)
- Modify: `crates/deluge-fixedpoint/Cargo.toml` (add `deluge-fft` as a dev-dependency)
- Modify: `tools/test.sh:42` (the `--features test-utils` comment about "no `--lib`")

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `deluge-fft` with no dev-dependency on any crate that stays in
  deluge-sdk. Phase 2 depends on this.

- [ ] **Step 1: Read the test and its dependencies**

Run:
```bash
cat crates/deluge-fft/tests/dsp_pipeline.rs
sed -n '20,35p' crates/deluge-fft/Cargo.toml
grep -n -A8 'dev-dependencies' crates/deluge-fixedpoint/Cargo.toml
```
Note which items the test imports from each crate — the move must preserve them
exactly.

- [ ] **Step 2: Move the file**

```bash
git mv crates/deluge-fft/tests/dsp_pipeline.rs crates/deluge-fixedpoint/tests/dsp_pipeline.rs
```

Update the file's module doc comment to say it now lives on the fixedpoint side,
and why:

```rust
//! Cross-crate DSP-pipeline integration test: quantise + fixed-point gain
//! feeding the FFT.
//!
//! Lives here rather than in `deluge-fft` because `deluge-fft` is destined to
//! become `flare-fft` in a separate repo, where a dev-dependency on
//! `fixedpoint` and `armv7-dsp-intrinsics` would be a dependency pointing back
//! at the SDK — the one thing the flare seam forbids. Both halves of this
//! pipeline are reachable from here, so nothing is lost by testing it from this
//! side.
```

- [ ] **Step 3: Move the dev-dependencies**

In `crates/deluge-fft/Cargo.toml`, delete the `fixedpoint` and
`armv7-dsp-intrinsics` dev-dependency lines and the comment block above them
that explains the pipeline test (lines 28-32). Keep `approx`, `criterion` and
`rustfft` — those serve `deluge-fft`'s own tests and benches.

In `crates/deluge-fixedpoint/Cargo.toml`, add to `[dev-dependencies]`:

```toml
# The cross-crate dsp_pipeline integration test (tests/dsp_pipeline.rs):
# quantise + fixed-point gain feeding the FFT. Lives on this side of the
# eventual flare seam so `deluge-fft` keeps no dev-dependency on a crate that
# stays in the SDK.
deluge-fft = { path = "../deluge-fft", features = ["test-utils"] }
```

- [ ] **Step 4: Verify both crates still test clean**

Run:
```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-fft --features test-utils 2>&1 | grep '^test result'
cargo test --target x86_64-unknown-linux-gnu -p deluge-fixedpoint 2>&1 | grep '^test result'
cargo test --target armv7-unknown-linux-gnueabihf -p deluge-fixedpoint 2>&1 | grep '^test result'
```
Expected: all pass. The `dsp_pipeline` test now appears in `deluge-fixedpoint`'s
output rather than `deluge-fft`'s — confirm it is actually being run and did not
silently vanish:

```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-fixedpoint 2>&1 | grep dsp_pipeline
```
Expected: at least one test name printed. **An empty result means the test was
lost in the move, not that it passed.**

- [ ] **Step 5: Prove `deluge-fft` no longer reaches back**

Run:
```bash
grep -nE 'fixedpoint|armv7-dsp-intrinsics|deluge-' crates/deluge-fft/Cargo.toml
```
Expected: only the `name = "deluge-fft"` line and the package metadata. No
dependency or dev-dependency on any other `deluge-*` crate.

- [ ] **Step 6: Update the test runner comment**

`tools/test.sh:41-42` currently reads:

```bash
# No --lib: also runs the cross-crate dsp_pipeline integration test.
cargo test --target "$QEMU" -p deluge-fft --features test-utils
```

That comment is now wrong. Replace those two lines with:

```bash
cargo test --target "$QEMU" -p deluge-fft --features test-utils
```

and change the `deluge-fixedpoint` line above it (currently
`cargo test --target "$QEMU" -p deluge-fixedpoint --lib`) to drop `--lib`, so
the relocated integration test actually runs:

```bash
# No --lib: also runs the cross-crate dsp_pipeline integration test, which
# moved here from deluge-fft (that crate becomes flare-fft and must not
# dev-depend on anything staying in the SDK).
cargo test --target "$QEMU" -p deluge-fixedpoint
```

- [ ] **Step 7: Run the full runner**

Run: `tools/test.sh`
Expected: everything passes, and `dsp_pipeline` appears in the
`deluge-fixedpoint` section of the output.

- [ ] **Step 8: Commit**

```bash
git add crates/deluge-fft/Cargo.toml crates/deluge-fixedpoint/Cargo.toml crates/deluge-fixedpoint/tests/dsp_pipeline.rs tools/test.sh
git commit -m "test: move the dsp_pipeline test to the fixedpoint side

deluge-fft becomes flare-fft in a separate repo (spec §1), where a
dev-dependency on fixedpoint and armv7-dsp-intrinsics would point back at the
SDK — the one thing the seam forbids. Both halves of the pipeline are reachable
from deluge-fixedpoint, so the test moves rather than dies.

Drops --lib from the fixedpoint runner line so the relocated integration test
is actually executed; deluge-fft's Cargo.toml now names no other deluge- crate."
```

---

### Task 11: Phase 1 exit gate

Confirm the phase actually delivered its purpose: a net that would catch a bad
poly refactor. This task writes no production code; it verifies and records.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-08-flare-audio-engine-extraction-design.md` (record the Phase 1 result)

**Interfaces:**
- Consumes: everything from Tasks 1-10.
- Produces: a recorded baseline that Phase 2's "bit-identical" gate is measured against.

- [ ] **Step 1: Full runner, clean tree**

Run:
```bash
git status --short
tools/test.sh 2>&1 | tail -40
```
Expected: clean tree, every invocation passing.

- [ ] **Step 2: Count the goldens and record their digests**

Run:
```bash
grep -c '#\[test\]' crates/deluge-audio-graph/src/golden.rs
grep -nE 'const [A-Z_]+_DIGEST: u64' crates/deluge-audio-graph/src/golden.rs
```
Expected: 8 tests; 7 digest constants (the sample-wise golden pins an array
rather than a digest). No constant is `0`.

- [ ] **Step 3: Prove the net actually catches a regression**

A golden suite nobody has seen fail is a suite nobody knows works. Introduce a
deliberate one-line perturbation in a kernel — for example, in
`crates/deluge-dsp-kernels/src/poly.rs`, change a filter coefficient or a gain
by a small amount — then run:

```bash
cargo test --target x86_64-unknown-linux-gnu -p deluge-audio-graph golden 2>&1 | grep -E '^test golden|^test result'
```
Expected: **multiple goldens fail.** Record which ones.

Then revert the perturbation (`git checkout crates/deluge-dsp-kernels/src/poly.rs`)
and confirm all goldens pass again.

If the perturbation did **not** break any golden, the net has a hole — report
which kernel path is unguarded rather than declaring the phase done.

- [ ] **Step 4: Record the result in the spec**

Append to the spec's `### Phase 1` subsection in §4:

```markdown
**Phase 1 complete (YYYY-MM-DD).** `tools/test.sh` now runs both configurations
(the `simd` feature was previously exercised by nothing). Goldens live in
`crates/deluge-audio-graph/src/golden.rs`: 8 tests, 7 pinned digests, covering
the saw→lpf→env baseline, the poly osc/VoiceSum path, PolyMoog, PolyMs20,
PolySyncOsc, staggered per-voice gating, the static wavetable sweep across mip
levels, and the master chain with two buses. All hold identically in the scalar
and `simd` configurations and on both the x86-64 and 32-bit ARM buckets — that
config- and target-invariance is what makes them a usable Phase 2/3 gate.
Verified catching a regression: a one-line perturbation to `poly.rs` failed
<N> goldens. `deluge-fft` no longer dev-depends on any crate staying in the SDK.
```

Fill in the date and the observed `<N>`.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-09-08-flare-audio-engine-extraction-design.md
git commit -m "docs: record the flare Phase 1 result

The net is verified, not just written: a deliberate one-line perturbation to
poly.rs fails multiple goldens, and every golden holds identically across the
scalar/simd configurations and the x86-64/ARM buckets. That invariance is the
property Phase 2's 'bit-identical' gate and Phase 3's 'default output must not
move' gate both rest on."
```

---

## What this plan does NOT cover

Phases 2-4 of the spec get their own plans, written when this one is done:

- **Phase 2** — the `git filter-repo` split, crate renames, the `flare` facade,
  and rewiring `deluge-sdk` to the git dependency.
- **Phase 3** — `AUX_OUTS`, the `MAX_BLOCK` feature, the `VOICES` chunk refactor,
  scoped updates, and the multi-engine test.
- **Phase 4** — the four-target CI matrix.

Phase 2's plan should not be written until Phase 1's exit gate (Task 11, Step 3)
has actually demonstrated the goldens catching a regression, because the whole
of Phase 2 is gated on "any digest change is a bug" — which is only a meaningful
gate if the digests are known to move when something breaks.
