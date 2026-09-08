# Flare — Extracting the Audio Engine from the Deluge SDK — Design

**Date:** 2026-09-08
**Scope:** A new repo, `flare`, taking `deluge-dsp-kernels`, `deluge-audio-graph`,
`deluge-fft` and `mipgen` out of `deluge-sdk` and generalizing them into a
portable audio engine. `deluge-wren-core` and everything Deluge-specific stays
behind.

## Background

`crates/deluge-audio-graph` is a `no_std`, zero-allocation, block-rendering audio
graph: 80 node kinds evaluated in topological order into a pooled arena, ports
addressed `(node, port)`, stereo buses with fan-in, control buses, a `Cmd`
transport in and an `Event` queue out, sizes const-generic and sample rate
runtime. Underneath it, `deluge-dsp-kernels` holds 29.7k lines of oscillators,
filters, envelopes, reverbs, samplers and granular kernels, with a `core::simd`
`f32x8` fast path (NEON on ARM) behind a non-default `simd` feature and a scalar
oracle behind `not(simd)`.

Nothing about that is Deluge-specific. A survey of the crate found **zero**
occurrences of "Deluge" in its 13.4k lines of source. The coupling to the
product is four things and no more:

1. Crate naming (`deluge-*`).
2. `USB_CHANNELS = 8` and `Engine::fill_usb` — the Deluge's USB audio routing,
   which is really "N aux mono outs".
3. Two compile-time constants baked to the RZ/A1L's budget: `VOICES = 8`
   (`poly.rs:28`) and `MAX_BLOCK = 128` (`node.rs:1510`).
4. Its position inside a `no_std` workspace whose default target is
   `armv7a-none-eabihf`.

The engine is therefore already portable in substance and parochial only in
packaging. **Goal:** make it a standalone project usable on other embedded
targets, on desktop/std hosts, and on wasm, without giving up the NEON work or
disturbing the Deluge.

### Licensing — the finding that shapes the split

The initial premise for this work was that the DSP kernels were GPL,
Deluge-originated code that would have to stay behind and be rewritten. **They
are not.** The evidence, recorded here because the licensing position should be
documented rather than remembered:

- `crates/deluge-dsp-kernels/Cargo.toml` is `license.workspace = true`, i.e.
  `MIT OR Apache-2.0`.
- Every DSP plan doc carries an explicit constraint line. E.g.
  `docs/superpowers/plans/2026-07-10-sy-poly-filters.md:14` — *"Licensing: MIT /
  Apache-2.0 only. No GPL."*
- The two kernel files that mention GPL do so as **disclaimers**. `eq.rs:3` —
  *"Coefficients (RBJ 'Audio EQ Cookbook') recomputed per block. Not ported from
  any GPL source."* `reverb.rs:4` — *"Implemented from the public algorithm
  structure — not the GPL freeverb source."*
- `docs/superpowers/plans/2026-07-09-ef-delay.md:15` is explicit: *"Do NOT port
  spark's GPL-3.0 `deluge/fx/eq.rs` or `freeverb`."*
- The plate reverb was built from the public Dattorro 1997 paper, with constants
  cross-checked against an MIT-licensed implementation.
- `wavetables_generated.rs` is synthesized analytically by `mipgen`'s
  `gen_tables` binary (Fourier series for saw/square/sine/tri/organ/formant).
  No sampled Deluge ROM data.

The GPL code in `deluge-sdk` is **UI, not DSP**: `deluge-ui-toolkit`,
`deluge-fonts` and `deluge-grid-toolkit` (vendored from `spark`;
`grid-toolkit/src/color.rs:11` — *"Ported from the Deluge C++ `RGB` class"*).
None of them is in the audio graph's dependency tree, and `README.md:186` makes
that a deliberate invariant. They stay in `deluge-sdk`.

Flare is therefore `MIT OR Apache-2.0` throughout, and carries a
`PROVENANCE.md` recording the above.

## 1. Topology

A new repo, `flare`, holding a six-crate workspace: the five existing crates
renamed, plus a facade.

| flare crate | from | lines | purpose |
|---|---|---|---|
| `flare-kernels` | `crates/deluge-dsp-kernels` | 29.7k | portable `f32` DSP kernels |
| `flare-graph` | `crates/deluge-audio-graph` | 13.4k | the block-rendering engine |
| `flare-fft` | `crates/deluge-fft` | 3.5k | const-generic portable-SIMD FFT |
| `flare-mipgen` | `crates/mipgen` | 0.6k | band-limited wavetable mip pyramids |
| `flare-dsp-test` | `crates/deluge-dsp-test` | 0.7k | host-side DSP measurement harness |
| `flare` | *new* | ~100 | facade re-exporting the public surface |

**`deluge-dsp-test` corrected in (2026-09-08).** This design originally listed
four crates. `deluge-dsp-test` is a `[dev-dependencies]` entry of both
`deluge-dsp-kernels` and `mipgen`, so leaving it behind would make flare
dev-depend on the SDK — the one thing the seam forbids. It was missed on the
first survey because it sits in the workspace `exclude` list (`Cargo.toml:62`),
being std-only (`realfft`), rather than in `members`. Nothing that stays in
deluge-sdk uses it, so it moves cleanly and orphans nothing. Its own module doc
records the invariant that makes it safe to move: *"it never depends on the
kernel or graph crates."*

The facade re-exports what `deluge-audio-graph/src/lib.rs` re-exports today —
`Engine`, `Cmd`, `Host`, `Node`, `Kind`, `Rate`, `In`, `Input`, `NodeId`,
`BusId`, `CtrlBusId`, `OutputSrc`, `StereoFrame`, `Event`, `Pool`, `TableId`,
the voice allocators — so a consumer names one crate.

The four-crate split is preserved rather than collapsed because the boundaries
are already good ones: the FFT has non-audio uses and its own benches, `mipgen`
is a build-time table generator, and someone may want only the kernels.

### What stays in `deluge-sdk`

- `deluge-wren-core` — the Wren scripting surface. This is the Deluge's control
  language, not the engine, and it is the single substantial consumer of the
  engine API.
- `wren-firmware`, `firmwares/*`, and the rest of the SDK.
- `deluge-fixedpoint` and `armv7-dsp-intrinsics` — a fixed-point numeric stack
  that is ARMv7-specific and that `deluge-sdk` itself depends on
  (`deluge-sdk/Cargo.toml:32`). Pulling them into a portable engine would be
  backwards.
- The three GPL crates.

### The seam

One direction only: **flare never names a `deluge-*` crate.** `deluge-sdk`
consumes flare as a git dependency pinned by rev, with a `[patch]` path override
for local co-development:

```toml
# deluge-sdk/Cargo.toml
[workspace.dependencies]
flare       = { git = "https://github.com/FirestormAudio/flare", rev = "…" }
flare-graph = { git = "https://github.com/FirestormAudio/flare", rev = "…" }

# .cargo/config.toml, developer-local, not committed as the default
[patch."https://github.com/FirestormAudio/flare"]
flare = { path = "../flare/crates/flare" }
```

This mirrors the precedent set by the three-repo reorg
(`2026-07-19-repo-topology-reorg-design.md`): a repo split gets exactly one
explicit interface, and bidirectional path-dependencies are the thing being
prevented.

There is one back-dependency to break. `deluge-fft/tests/dsp_pipeline.rs` is a
cross-crate integration test (quantise → fixed-point gain → FFT) whose
dev-dependencies are `fixedpoint` and `armv7-dsp-intrinsics`, both of which stay
in `deluge-sdk`. It **relocates to `deluge-fixedpoint`**, where both halves
live. Nothing is lost — the test still runs, from the other side of the seam.

`firmwares/demo-firmware` depends on `deluge-fft` directly; it becomes an
ordinary `flare-fft` consumer.

## 2. Generalization

Three changes to the public model, plus two to the patch model (§3). Ordered
cheapest first, which is also the order they are implemented.

### 2a. Aux outputs

`USB_CHANNELS` is the count of routable mono output channels. On the Deluge they
happen to go out over USB; in flare they are aux sends. Rename only:

| today | flare |
|---|---|
| `ids::USB_CHANNELS` | `ids::AUX_OUTS` |
| `Engine::fill_usb` | `Engine::fill_aux` |
| `Engine.usb_out` (private) | `Engine.aux_out` |
| `Cmd::SetUsbOut` (`cmd.rs:72`) | `Cmd::SetAuxOut` |

`OutputSrc` is already generic and keeps its name. Roughly 13 sites
(`ids.rs:40`, `lib.rs:42`, `cmd.rs:70-74`, `engine.rs` ×9). `Cmd::SetAuxOut`
keeps the `{ channel: u8, src: OutputSrc }` shape, so `deluge-wren-core`'s
binding changes name only.

### 2b. `MAX_BLOCK` configurable

`MAX_BLOCK` is purely a ceiling. `BLOCK` is already a const-generic parameter on
`Engine`, asserted `BLOCK <= MAX_BLOCK` at `engine.rs:152`; `MAX_BLOCK` itself
sizes ten stack scratch arrays across four sites in `node.rs` (1384–1387,
1440–1441, 1461–1462, 1482–1483) where `BLOCK` is not in scope, and the offline
render scratch (`nrt.rs:65`).

It becomes a feature-selected const:

```toml
[features]
default   = ["block-128"]
block-64  = []
block-128 = []
block-256 = []
block-512 = []
```

Constrained to multiples of 8 so the sample-parallel `f32x8` code in `math.rs`
stays aligned; a `const _: () = assert!(MAX_BLOCK % 8 == 0);` pins it. The cost
of a larger ceiling is stack, which is exactly the knob a small target wants to
turn down.

### 2c. `VOICES` configurable in multiples of 8

This is the only structurally interesting change. `VOICES = 8` (`poly.rs:28`)
appears 293 times across the two crates, but those decompose into three groups
of very different cost:

1. **`[T; VOICES]` array sizing and `tile[i * VOICES + v]` interleaved
   indexing.** Scales for free; no change.
2. **The scalar config's per-voice state.** Already `[f32; VOICES]` / `[Ar;
   VOICES]`; scales for free.
3. **The `simd` config.** The only real work.

Group 3 is contained because the `_x8` helpers are **width-agnostic elementwise
lane primitives**, not voice-aware code. `wave_sample_x8` (`osc.rs:165`),
`naive_wave_x8`, `poly_blep_x8`, `floor_x8`, `pade_tanh_x8` (`filter.rs:191`),
`ms20_clip_x8` (`filter.rs:550`) are all `f32x8 → f32x8`. They do not know what
a voice is. `poly.rs` is the only place where "8 lanes" means "8 voices".
The 47 `f32x8` sites in `osc.rs`, 37 in `wavetable.rs`, 7 in `filter.rs` and 3
in `math.rs` are therefore **untouched**.

The work, all in `poly.rs`:

- The two `const _: () = assert!(VOICES == 8);` guards (`poly.rs:78`,
  `poly.rs:396`) become `assert!(VOICES % 8 == 0 && VOICES > 0)`.
- Introduce `pub const VOICE_CHUNKS: usize = VOICES / 8;`.
- The 72 `f32x8` sites gain an outer `for c in 0..VOICE_CHUNKS` loop, loading
  and storing lane `c * 8` of the interleaved tile.
- Four structs holding `f32x8` state gain a chunk dimension:

  | struct | field | today | flare |
  |---|---|---|---|
  | `PolyMoog` | `state` (`poly.rs:517`) | `[f32x8; POLES]` | `[[f32x8; VOICE_CHUNKS]; POLES]` |
  | `PolyMs20` | `dc_x`, `dc_y` (`poly.rs:598`, `:600`) | `f32x8` | `[f32x8; VOICE_CHUNKS]` |
  | `PolyMs20` | `ic1`, `ic2` | `f32x8` | `[f32x8; VOICE_CHUNKS]` |

  All six fields are `#[cfg(feature = "simd")]`-only; the scalar config holds
  `voices: [Ms20; VOICES]` and so already scales.
  | `PolySyncOsc` | `master_phase`, `slave_phase` (`poly.rs:723`, `:725`) | `f32x8` | `[f32x8; VOICE_CHUNKS]` |

  The free-function ladder helper at `poly.rs:473` takes `state: &mut [f32x8;
  STAGES]` — it is called per chunk and so keeps its signature.

Feature-selected:

```toml
default   = ["voices-8", "block-128", …]
voices-8  = []
voices-16 = []
voices-24 = []
```

NEON is **not** removed, weakened, or made conditional on `VOICES == 8`. The
`f32x8` fast path runs on every configuration; a wider voice count runs it more
times per sample rather than falling back to scalar.

## 3. Multiple simultaneous patches

### Current behaviour, stated precisely

Multi-timbral playback already works. Several independent subgraphs coexist in
one `Engine` arena, each writing to its own stereo bus; buses fan in and sum
(`bus.rs`) into the master selected by `set_root`, with `AUX_OUTS` mono sends
routable from any bus side or node port. Polyphony is **per poly node** —
`VOICES` lanes each — so two instruments get `VOICES` voices each, not `VOICES`
between them. The ceiling is `NODES` capacity and CPU.

What does not exist is any first-class notion of an instrument. `NodeId` is a
flat `u16` space in one arena, and the incremental update sweep is **global**
(`engine.rs:531`):

```rust
fn end_update(&mut self) {
    self.in_update = false;
    for idx in 0..NODES {
        if self.arena.node(id).is_some() && self.node_epoch[idx] != self.epoch {
            self.free_node(id);
```

Any live node not stamped with the current epoch is freed, so `BeginUpdate` /
`EndUpdate` requires re-emitting the **entire** graph. Instrument A cannot be
live-edited without re-emitting instrument B. For the Deluge that is fine — one
Wren script owns the whole graph. For a host loading and editing independently
authored patches, it is a blocker.

### 3a. Scoped updates

Add an optional scope to the update epoch, so a host can re-emit one instrument.

- A `scope: u8` per node, stored alongside `node_epoch` in a parallel
  `[u8; NODES]`. `0` = the default/global scope, preserving today's behaviour
  for every existing caller.
- `Cmd::BeginUpdateScoped { scope: u8 }` sets `in_update` and bumps the epoch as
  `Cmd::BeginUpdate` does, and additionally records `update_scope`.
- `Cmd::NewNode` stamps the node with the active scope as well as the epoch
  (extending `stamp`, `engine.rs:557`).
- `end_update` sweeps only nodes whose scope matches `update_scope`:

  ```rust
  if self.arena.node(id).is_some()
      && self.node_scope[idx] == self.update_scope
      && self.node_epoch[idx] != self.epoch
  ```

  An unscoped `Cmd::BeginUpdate` keeps the global sweep, so this is purely
  additive.

Everything else is unchanged: one arena, one topological sort, shared buses,
shared capacity, cross-instrument routing all still work. `SCOPES` is a plain
`u8` range, not a new const-generic — scopes are a labelling of existing nodes,
not a new allocation.

### 3b. Multi-engine, proven rather than assumed

`Engine` × N already works: there is **no global mutable state** in either crate
(the only statics are the immutable generated wavetable arrays), so N engines
can be instantiated and summed by a host today. It is simply untested. Flare
adds:

- An integration test instantiating two `Engine`s with different const-generic
  capacities, rendering both, summing them, and pinning the result as a digest.
- Documentation of the trade-off, so the choice is informed: multi-engine gives
  hard isolation and independent capacities but no cross-engine buses, N
  topological sorts, and N× worst-case preallocated memory. Scoped updates
  (§3a) give shared routing and one allocation but no isolation.

## 4. Migration sequence

Four phases, each independently verifiable and revertable.

### Phase 1 — Prepare in place (in `deluge-sdk`, before anything moves)

Characterisation coverage today is one patch: `golden_saw_lpf_env_first_block`
(`cmd.rs:416`, sample-wise) and `golden_saw_lpf_env_offline_digest`
(`cmd.rs:508`, 4096 frames as one digest), both verified to match on x86-64 and
on ARM. That is thin cover for a 47k-line move plus a poly SIMD refactor, and
§2c touches code no current golden exercises.

So, first: extend characterisation goldens to cover the poly path specifically —
`PolyOsc`, `PolyMoog`, `PolyMs20`, `PolySyncOsc`, the four structs whose state
gains a chunk dimension — plus per-voice gating (the allocator side of the
voice/lane mapping), the wavetable path across mip levels, and the master chain
with two buses. Each is an offline render pinned as an `nrt::digest`.

**Sampler and granular are deliberately excluded.** `sampler.rs` and
`granular.rs` contain zero `f32x8` sites — they are entirely scalar — so §2c,
which reworks only `f32x8` state, cannot reach them; their `[T; VOICES]` arrays
scale with the voice count for free. They keep their existing unit tests. If
Phase 3c's implementation turns out to touch them after all, they get goldens
then.

Prerequisite discovered while planning: **the `simd` configuration is currently
built by nothing.** `tools/test.sh` never passes `--features simd` and no CI
workflow does, so the kernels' scalar==simd equivalence tests only run when
invoked by hand. Phase 3c refactors that code; it cannot be gated by a
configuration nobody builds. Adding both configurations to the runner is
therefore the first task of Phase 1, not a nicety. (Verified 2026-09-08: both
configs pass today — kernels 288 scalar / 286 simd, graph 313 both.)

Also in this phase: relocate `deluge-fft/tests/dsp_pipeline.rs` to
`deluge-fixedpoint`.

This is the highest-value step in the plan. It is what makes phases 2 and 3
verifiable rather than hopeful.

**Gate:** new goldens green on x86-64 and on the device target, in both `simd`
and scalar configurations.

**Phase 1 complete (2026-09-08).** `tools/test.sh` now runs both configurations;
the `simd` feature had been built by nothing, leaving 14 scalar-oracle
equivalence tests — including `polymoog_matches_scalar_oracle_both_slopes` and
`polysync_matches_scalar_oracle_all_waves` — never executed in CI.

Goldens live in `crates/deluge-audio-graph/src/golden.rs`: 9 tests, 8 of them
digest-pinned (10 constants; see below), covering the saw→lpf→env baseline, the
poly osc/`VoiceSum` path, `PolyMoog`, `PolyMs20`, `PolySyncOsc`, staggered
per-voice gating, a static wavetable sweep across mip levels, and the master
chain (bus gain, bus send, DC-block → EQ → limiter) with two buses.

All are **target-invariant** — identical digests on x86-64 and on 32-bit ARM
under NEON. Most are **config-invariant** too, but two are not, and the
distinction is now documented in the module: some `f32x8` kernels are bit-exact
reimplementations of their scalar oracle, while `PolySync` and the wavetable
interpolation reorder the arithmetic and agree only to a tolerance (the kernels
say so in their own test names — `simd_matches_scalar_within_tol`, and sync's
`<= 1e-4`). Those two pin one constant per configuration rather than loosening
to a tolerance, so the gate keeps full strength in whichever config is built.
Both divergences are inaudible: identical peak, RMS agreeing to ~1e-6.

**Verified catching a regression, not merely written.** Changing one constant in
`fast_sin` from `0.225` to `0.226` fails **5 of 9 goldens** in the scalar
configuration. The 4 survivors are exactly those with no sine in their path
(Saw+Lpf, the Saw-shape sync patch, the Saw wavetable), so the failure set is
explainable rather than incidental. The same one-line change fails only **1 of
9** under `simd`, because the `f32x8` path uses a separate `fast_sin_x8` and
only the master EQ (a scalar kernel) is shared — which is the concrete argument
for running both configurations: a single-config CI would have missed four of
those five regressions.

`deluge-fft`'s `Cargo.toml` now names no other `deluge-*` crate; the
`dsp_pipeline` test moved to `deluge-fixedpoint` and, as a side effect of
dropping `--lib` from its runner line, now runs in both target buckets instead
of ARM only.

### Phase 2 — The move (pure rename, zero behaviour change)

- `git filter-repo` to split the four crate directories into `flare` with
  history preserved. Verify `git log --follow` resolves on representative files
  **before** anything is deleted from `deluge-sdk`; the SDK-side removal is the
  last commit of the phase, not the first.
- Rename crates, `[lib] name`s, and `use` paths; add the `flare` facade crate
  and the workspace manifest; add `PROVENANCE.md`, `LICENSE-MIT`,
  `LICENSE-APACHE`, `README.md`, CI.
- Rewire `deluge-sdk`: `deluge-wren-core`, `wren-firmware` and
  `firmwares/demo-firmware` take the git dependency.

**Gate:** every golden digest **bit-identical**; `wren-firmware` builds for
`armv7a-none-eabihf`; all 228 `deluge-wren-core` tests pass. Because this phase
is a pure rename, *any* digest change is a bug — which makes a large diff
trivially reviewable.

### Phase 3 — Generalize (in flare)

`3a` aux rename → `3b` `MAX_BLOCK` feature → `3c` `VOICES` chunking → `3d`
scoped updates → `3e` multi-engine test and docs. Cheapest first, so the risky
one lands against a green tree.

**Gate, and this is the crisp part:** at the default configuration
(`voices-8`, `block-128`) **every golden digest must be unchanged, bit for
bit.** The generalization is permitted to add configurations; it is not
permitted to move the default's output. At `voices-16` and `voices-24` the
existing scalar-oracle equivalence tests must hold lane-for-lane to ≤ 1e-4, per
the standing SIMD convention.

### Phase 4 — Prove the portability claim

A CI matrix, because otherwise "portable" is an assertion:

| target | why |
|---|---|
| `armv7a-none-eabihf` | the device; NEON `f32x8`; the status quo |
| `x86_64-unknown-linux-gnu` | host tests; `core::simd` → SSE/AVX |
| `wasm32-unknown-unknown` | the web simulator's target; `core::simd` → simd128 |
| `thumbv7em-none-eabihf` | Cortex-M; the real portability proof |

Each × `{default, simd}`, plus the voice/block feature configurations on the
host. The Cortex-M target is the one expected to find latent assumptions —
`MAX_BLOCK` stack scratch and the `PCAP`/`PCHUNK` wavetable pool are the likely
sites.

## 5. Testing

626 tests move with flare: 313 in `deluge-audio-graph`, 288 in
`deluge-dsp-kernels`, 25 in `deluge-fft` (all in-module `#[test]`s; neither
moving crate has a `tests/` directory). They move unchanged apart from import
renames.

The 228 `deluge-wren-core` tests stay in `deluge-sdk` and become the
**integration check that the rewire worked** — they exercise the engine through
the public API from the far side of the seam.

Standing conventions carried into flare unchanged:

- The scalar path is the correctness oracle; the `simd` path is null-tested
  against it lane-for-lane to ≤ 1e-4.
- `simd` is **default-off**, so the everyday `cargo build` *is* the scalar path.
  The fallback is load-bearing, not theoretical.
- Tests run per-crate, never `--workspace`, in both configurations.
- Host tests need an explicit `--target x86_64-unknown-linux-gnu` while the
  workspace default target is the embedded one.

## 6. Non-goals

Deliberately excluded, to keep this from sprawling:

- **No `std` or `alloc` feature.** The engine is const-generic and
  zero-allocation, which works fine on desktop as it stands.
- **No WAV writer, no CPAL example, no plugin (VST/CLAP/AU) wrapper.**
  `nrt::render_offline` already gives a host everything it needs; writing a WAV
  is thirty lines in the *consumer*. A plugin host is flare's second spec.
- **No const-generic `VOICES`.** Multiples of 8 via features, not a type
  parameter; a seventh const-generic on an already-wide `Engine` signature is
  not worth it, and the `f32x8` chunking gets the same result.
- **No new node kinds, no DSP changes.** Phase 3's gate is that default output
  does not move.
- **No Wren.** The scripting surface stays in `deluge-sdk`.
- **Not published to crates.io yet.** Git dependency pinned by rev. Publishing
  is a later decision, once the API has settled under a second consumer.

## 7. Risks

- **The poly chunk refactor (§2c) is the one genuinely risky change.** Mitigated
  by ordering it last, behind the extended goldens of Phase 1, with a
  bit-identical default-config requirement.
- **History preservation.** `filter-repo` across four directories that have moved
  historically can produce a messy graph. Verified before deletion, and deletion
  is last.
- **The two-repo dance.** The `[patch]` override makes local work pleasant but
  makes it easy to land an SDK change that only builds against an unpushed
  flare. CI must build `deluge-sdk` against the **pinned rev**, with the override
  disabled.
- **Facade sufficiency.** `deluge-wren-core` currently names `flare-kernels`
  types directly (`TableId`, `TableSrc` — `deluge-wren-core/Cargo.toml:45`). If
  the facade is the public story, wren-core should consume it rather than
  reaching past it; whether that is possible is the test of whether the facade
  is actually sufficient. If it is not, that is a finding, not a failure — the
  facade grows.
