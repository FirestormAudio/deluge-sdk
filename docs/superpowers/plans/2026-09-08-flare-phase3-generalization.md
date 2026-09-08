# Flare Phase 3 — Generalization — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make flare configurable where it is currently hard-coded to one
product's budget — aux outputs, block ceiling, voice count — and let a host
live-edit one patch without re-emitting the others, **without moving the default
configuration's output by a single bit**.

**Architecture:** Cheapest change first, riskiest last, against a tree that stays
green throughout. `AUX_OUTS` and `MAX_BLOCK` are renames and a feature-selected
const. `VOICES` becomes a multiple of 8 by wrapping each `f32x8` site in a chunk
loop — NEON is kept and run more times per sample, never traded for scalar.
Scoped updates and the multi-engine story are additive.

**Tech Stack:** Rust nightly, `core::simd`, `no_std`, two Cargo workspaces
(`~/GitHub/flare` and `~/GitHub/deluge-sdk`), `cargo test` across host and
QEMU-ARM buckets.

**Spec:** `docs/superpowers/specs/2026-09-08-flare-audio-engine-extraction-design.md`
(this plan implements Phase 3, spec §2 and §3)

**Depends on:** Phases 1 and 2, complete and merged. Phase 1's goldens are this
plan's gate; Phase 2 put them in `~/GitHub/flare/crates/flare-graph/src/golden.rs`.

**Note on repos:** almost all the work is in `~/GitHub/flare`. Task 1 has
fallout in `~/GitHub/deluge-sdk` (the Wren command codec). Both repos must be
green at the end of every task that touches both.

## Global Constraints

- **The default configuration's output must not move.** At `voices-8` and
  `block-128`, every golden digest stays **byte-identical**. This phase is
  permitted to add configurations; it is not permitted to change the default's
  audio. A changed default digest is a bug, never a value to re-pin.
- **NEON is kept.** No `f32x8` path is deleted, weakened, or made conditional on
  `VOICES == 8`. A wider voice count runs the same fast path more times per
  sample; it never falls back to scalar.
- **The scalar path is the correctness oracle**, and the `simd` path is
  null-tested against it lane-for-lane to ≤ 1e-4. That equivalence must hold at
  **every** supported voice count, not only at 8.
- **Licensing:** MIT / Apache-2.0 only. No GPL.
- **`no_std`, no heap, no panics in DSP paths.** Bounded loops.
- **Both configurations, both buckets.** `flare/tools/test.sh` runs scalar and
  `simd` across host and QEMU ARM plus a bare-metal build guard. It is the
  acceptance command for every task.
- **`~/.local/bin` is not on `PATH`** on this machine; prefix
  `export PATH="$HOME/.local/bin:$PATH"` if a task needs `git filter-repo`.
- **`arm-none-eabi-gcc` is not installed**, so `wren-firmware` cannot be linked
  here. Pre-existing, not a regression. The bare-metal build guard in
  `flare/tools/test.sh` covers what matters for flare.

## Baseline (verified 2026-09-08, after Phase 2)

| fact | value |
|---|---|
| goldens | 9 tests, 8 digest-pinned, 10 constants, `flare-graph/src/golden.rs` |
| digest-divergent kinds | `PolySync`, wavetable — one constant per config, documented in the module |
| `flare-graph` tests | 320 + 1 doctest |
| `flare-kernels` tests | 278 scalar / 286 simd (host) |
| regression proof | `fast_sin` `0.225`→`0.226` fails **5 of 9** goldens scalar, **1 of 9** simd |
| aux-out surface | 38 sites: `ids.rs` 1, `lib.rs` 1, `cmd.rs` 3, `engine.rs` 33 |
| `MAX_BLOCK` surface | `node.rs:1510` (definition) + 10 scratch arrays at 4 sites + `engine.rs:152` + `nrt.rs:65` |
| `VOICES == 8` asserts | `poly.rs:78`, `poly.rs:396` |
| `f32x8` sites in `poly.rs` | 72 |
| poly kernels with a SIMD path | `PolyOsc` (86), `PolySvf` (403), `PolyMoog<POLES>` (512), `PolyMs20` (587), `PolySync` (719) |
| poly kernels that are scalar-only | `PolyCtrl`, `PolyAr`, `PolyAdsr`, `PolySlew`, `PolyNoise`, `PolyMtof` — these scale with `VOICES` for free |

## How a wider `VOICES` is verified

This is the part worth getting right before writing any code, because the
goldens **cannot** verify it: at `voices-16` a patch sums sixteen voices instead
of eight, so its digest legitimately differs. Three checks together do the job.

1. **The default must not move.** At `voices-8`, `VOICE_CHUNKS == 1`, the chunk
   loop runs once with `base == 0`, and every expression is identical to today's.
   All 10 golden digests must be byte-identical. This is the main gate, and it
   catches any accidental reordering.
2. **The scalar oracle, at every width.** The scalar paths are plain `for v in
   0..VOICES` loops that scale correctly by construction. The kernels' existing
   `*_matches_scalar_oracle` / `*_matches_scalar` tests compare simd against
   scalar lane-for-lane at ≤ 1e-4. Running those at `voices-16` is what proves
   the chunked SIMD is right, and it is a much stronger check than a digest.
3. **A chunk-boundary test**, new in Task 8. Voices 7 and 8 straddle the first
   chunk boundary; a bug that reads the wrong chunk shows up there first and
   nowhere else. Nothing existing covers it, because today there is only one
   chunk.

## File Structure

| file | change |
|---|---|
| `flare-graph/src/ids.rs` | `USB_CHANNELS` → `AUX_OUTS` |
| `flare-graph/src/cmd.rs` | `Cmd::SetUsbOut` → `Cmd::SetAuxOut` |
| `flare-graph/src/engine.rs` | `usb_out` → `aux_out`, `fill_usb` → `fill_aux`; scoped-update fields |
| `flare-graph/src/lib.rs` | re-export rename |
| `flare-graph/src/node.rs` | `MAX_BLOCK` becomes feature-selected |
| `flare-graph/Cargo.toml` | `block-*` features |
| `flare-kernels/src/poly.rs` | `VOICE_CHUNKS`, relaxed asserts, chunk loops |
| `flare-kernels/Cargo.toml` | `voices-*` features |
| `flare/Cargo.toml`, `flare/src/lib.rs` | forward the new features, re-export renames |
| `flare-graph/src/golden.rs` | the chunk-boundary golden (Task 8) |
| `flare/tools/test.sh` | add the non-default voice/block configurations |
| `deluge-sdk/crates/deluge-wren-core/src/codec.rs` | `Cmd::SetAuxOut` (Task 1) |

---

### Task 1: Aux outputs (spec §2a)

`USB_CHANNELS` counts routable mono output channels. On the Deluge they happen
to leave over USB; in flare they are aux sends. A rename, plus fallout in the
Wren command codec.

**Files:**
- Modify: `flare-graph/src/{ids.rs,cmd.rs,engine.rs,lib.rs}`
- Modify: `flare/src/lib.rs`
- Modify: `deluge-sdk/crates/deluge-wren-core/src/codec.rs` and any binding that names the command

**Interfaces:**
- Consumes: nothing.
- Produces: `flare::AUX_OUTS`, `Cmd::SetAuxOut { channel: u8, src: OutputSrc }`,
  `Engine::fill_aux`. `OutputSrc` is unchanged.

- [ ] **Step 1: Record the baseline digests**

```bash
cd ~/GitHub/flare
grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs > /tmp/flare-digests-phase3.txt
cat /tmp/flare-digests-phase3.txt
```
Expected: 10 constants. Every later task diffs against this file.

- [ ] **Step 2: Rename in flare**

```bash
cd ~/GitHub/flare
grep -rl 'USB_CHANNELS\|usb_out\|fill_usb\|SetUsbOut' crates --include='*.rs' \
  | xargs sed -i \
      -e 's/USB_CHANNELS/AUX_OUTS/g' \
      -e 's/SetUsbOut/SetAuxOut/g' \
      -e 's/\bfill_usb\b/fill_aux/g' \
      -e 's/\busb_out\b/aux_out/g'
grep -rn 'usb\|USB' crates --include='*.rs'
```

Read every remaining hit. Doc comments will still say "USB output channel"; they
must be rewritten to describe what the thing now is. `cmd.rs`'s doc for the
command currently reads *"Route USB output channel `channel` (0..USB_CHANNELS)
from a mono source"* — make it say aux, and say why the name changed:

```rust
    /// Route auxiliary output channel `channel` (`0..AUX_OUTS`) from a mono
    /// source: a bus side or a node port. Read by [`Engine::fill_aux`].
    ///
    /// "Auxiliary" rather than any particular transport — where these channels
    /// physically go (USB, a multi-channel DAC, a file) is the host's business,
    /// not the engine's.
    SetAuxOut {
        channel: u8,
        src: OutputSrc,
    },
```

- [ ] **Step 3: Update `ids.rs`'s constant doc**

```rust
/// The number of routable auxiliary mono output channels.
///
/// A fixed count rather than a const-generic parameter: at one `OutputSrc` each
/// the whole table costs a handful of bytes, which is not worth another type
/// parameter on an already-wide `Engine` signature.
pub const AUX_OUTS: usize = 8;
```

- [ ] **Step 4: Verify flare — the gate**

```bash
cd ~/GitHub/flare
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
tools/test.sh 2>&1 | tail -2
```
Expected: `DIGESTS UNCHANGED` and "All tests passed." A rename cannot change
audio; if a digest moved, something other than a name was edited.

- [ ] **Step 5: Fix the deluge-sdk fallout**

`deluge-wren-core/src/codec.rs` names `Cmd::SetUsbOut` at lines 259, 413, 565,
572, 694 and in a comment at 650.

```bash
cd ~/GitHub/deluge-sdk
grep -rl 'SetUsbOut\|USB_CHANNELS\|fill_usb' --include='*.rs' crates wren-firmware tools \
  | xargs sed -i -e 's/SetUsbOut/SetAuxOut/g' -e 's/USB_CHANNELS/AUX_OUTS/g' -e 's/\bfill_usb\b/fill_aux/g'
```

**The wire opcode is a separate matter.** `codec.rs` has a `SET_USB_OUT`
discriminant. Rename the *constant* if you like, but its **numeric value must
not change** — the web simulator decodes the same bytes. Check:

```bash
grep -n 'SET_USB_OUT\|SET_AUX_OUT' crates/deluge-wren-core/src/codec.rs
```
and confirm the number beside it is untouched. A changed opcode silently breaks
the sim, and no test in this repo would catch it.

- [ ] **Step 6: Verify deluge-sdk**

```bash
cd ~/GitHub/deluge-sdk
tools/test.sh 2>&1 | tail -2
```
Expected: "All tests passed", including the codec round-trip tests, which are
what prove the wire format survived.

- [ ] **Step 7: Commit both repos**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "refactor: aux outputs, not USB outputs

USB_CHANNELS -> AUX_OUTS, Cmd::SetUsbOut -> Cmd::SetAuxOut, Engine::fill_usb ->
fill_aux. Where these channels physically go — USB, a multi-channel DAC, a file
— is the host's business; the engine only knows it has N routable mono sends.
OutputSrc is unchanged.

A rename: all 10 golden digests unchanged."
```

```bash
cd ~/GitHub/deluge-sdk && git add -A && git commit -m "refactor: follow flare's aux-output rename

Cmd::SetUsbOut is Cmd::SetAuxOut in flare. The wire opcode's numeric value is
deliberately untouched — the web simulator decodes the same bytes, and nothing
in this repo's tests would catch a changed discriminant."
```

---

### Task 2: `MAX_BLOCK` configurable (spec §2b)

`MAX_BLOCK` is purely a ceiling. `BLOCK` is already a const-generic parameter on
`Engine`, asserted `BLOCK <= MAX_BLOCK`; `MAX_BLOCK` itself sizes ten stack
scratch arrays where `BLOCK` is not in scope.

**Files:**
- Modify: `flare-graph/src/node.rs:1510`, `flare-graph/Cargo.toml`, `flare/Cargo.toml`

**Interfaces:**
- Consumes: nothing.
- Produces: features `block-64` / `block-128` (default) / `block-256` /
  `block-512` on `flare-graph`, forwarded by `flare`.

- [ ] **Step 1: Write the failing test**

Add to `crates/flare-graph/src/node.rs`'s test module:

```rust
#[test]
fn max_block_is_a_multiple_of_eight_and_at_least_one_block() {
    // The sample-parallel `f32x8` code in the kernels' `math` module steps
    // blocks eight samples at a time, so a ceiling that is not a multiple of 8
    // would leave a ragged tail with no scalar fallback behind it.
    assert_eq!(MAX_BLOCK % 8, 0, "MAX_BLOCK must be a multiple of 8");
    assert!(MAX_BLOCK >= 8);
}
```

- [ ] **Step 2: Run it**

```bash
cd ~/GitHub/flare && cargo test -p flare-graph max_block_is_a_multiple
```
Expected: PASS at the current 128. It is a guard for the feature work, not a
red-first test — say so rather than pretending otherwise.

- [ ] **Step 3: Make the constant feature-selected**

Replace `pub const MAX_BLOCK: usize = 128;` in `node.rs` with:

```rust
/// The largest `BLOCK` any [`crate::Engine`] may be instantiated with, and the
/// size of the stack scratch buffers in this module (where `BLOCK` is not in
/// scope). Asserted against in `Engine::new`.
///
/// Feature-selected rather than const-generic: it is a ceiling, not a
/// parameter, and threading a seventh const-generic through an already-wide
/// `Engine` signature to express "how much stack may a node use" would be a bad
/// trade. Raising it costs stack and nothing else, which is exactly the knob a
/// small target wants to turn *down*.
// Written largest-first with explicit `not(...)` guards so the arms are
// mutually exclusive. Cargo features are additive — two crates in one graph can
// each enable a different one — and bare `#[cfg(feature = ...)]` arms would then
// both compile, giving a duplicate definition. This way the largest enabled
// ceiling wins, which is the safe direction: too much stack is a cost, too
// little is an assert failure at `Engine::new`.
#[cfg(feature = "block-512")]
pub const MAX_BLOCK: usize = 512;
#[cfg(all(feature = "block-256", not(feature = "block-512")))]
pub const MAX_BLOCK: usize = 256;
#[cfg(all(feature = "block-64", not(any(feature = "block-256", feature = "block-512"))))]
pub const MAX_BLOCK: usize = 64;
#[cfg(not(any(feature = "block-64", feature = "block-256", feature = "block-512")))]
pub const MAX_BLOCK: usize = 128;

// Keep the sample-parallel f32x8 code in the kernels' `math` module aligned.
const _: () = assert!(MAX_BLOCK % 8 == 0);
```

The default is the `not(any(...))` arm rather than a `block-128` feature, so a
consumer who enables nothing gets 128. The `not(...)` guards are what make the
arms mutually exclusive: without them, two features enabled anywhere in the
dependency graph would compile two `MAX_BLOCK` definitions and fail. Add a test
that the constant exists and is sane under every combination if you want that
guaranteed rather than reasoned about.

- [ ] **Step 4: Declare the features**

In `crates/flare-graph/Cargo.toml`:

```toml
[features]
default = []
# Forward SIMD to the kernels crate.
simd = ["flare-kernels/simd"]
## The stack-scratch ceiling in `node.rs` and the largest `BLOCK` an `Engine`
## may use. 128 unless one of these is set. Bigger costs stack, nothing else.
block-64 = []
block-256 = []
block-512 = []
```

and forward them from `crates/flare/Cargo.toml`:

```toml
block-64 = ["flare-graph/block-64"]
block-256 = ["flare-graph/block-256"]
block-512 = ["flare-graph/block-512"]
```

- [ ] **Step 5: Verify every block configuration builds and the default is unchanged**

```bash
cd ~/GitHub/flare
for f in "" "--features block-64" "--features block-256" "--features block-512"; do
  echo "== ${f:-default}"; cargo test -p flare-graph $f 2>&1 | grep -E '^test result: ok|^error' | head -2
done
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
```

Expected: all four configurations pass and the digests are unchanged.

**Note `block-64` will fail any golden using `BLOCK = 64`… it will not** — the
goldens use `Engine<64, …>`, and `64 <= 64` holds. But if a future golden used a
larger `BLOCK`, `block-64` would trip `Engine::new`'s assert. That is the assert
doing its job; do not weaken it.

- [ ] **Step 6: Add the configurations to the runner**

In `flare/tools/test.sh`, after the host simd block:

```bash
echo "==> Host bucket, non-default block ceilings"
for f in block-64 block-256 block-512; do
  cargo test -p flare-graph --features "$f"
done
```

- [ ] **Step 7: Full runner, then commit**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | tail -2
git add -A && git commit -m "feat: make the block ceiling configurable

MAX_BLOCK sizes ten stack scratch arrays in node.rs and caps the BLOCK
const-generic. It is a ceiling, not a parameter, so it is feature-selected
rather than threaded through Engine's already-wide signature as a seventh
const-generic: raising it costs stack and nothing else, which is exactly the
knob a small target wants to turn down.

The default is the not(any(...)) arm rather than a block-128 feature, so
features stay additive — two enabled at once picks the larger instead of
failing to compile.

A const assert keeps it a multiple of 8, since the sample-parallel f32x8 code
in the kernels steps blocks eight at a time. All 10 golden digests unchanged
at the default."
```

---

### Task 3: `VOICE_CHUNKS`, and relaxing the width asserts (spec §2c)

Pure infrastructure. `VOICES` stays 8, so `VOICE_CHUNKS == 1` and nothing
changes yet — which is the point: it isolates the scaffolding from the risky
edits that follow.

**Files:**
- Modify: `flare-kernels/src/poly.rs:28` (add `VOICE_CHUNKS`), `:78`, `:396`

**Interfaces:**
- Consumes: nothing.
- Produces: `flare_kernels::poly::VOICE_CHUNKS`, and asserts that permit any
  positive multiple of 8. Tasks 4-7 use `VOICE_CHUNKS`.

- [ ] **Step 1: Add the constant beside `VOICES`**

In `crates/flare-kernels/src/poly.rs`, after `pub const VOICES: usize = 8;`:

```rust
/// `f32x8` chunks per poly node: `VOICES / 8`.
///
/// The SIMD poly kernels process voices eight at a time because that is one
/// NEON register. `VOICES` is constrained to a multiple of 8 so a wider voice
/// count runs that same fast path more times per sample rather than falling
/// back to scalar — the vector width and the voice count are deliberately not
/// the same number.
pub const VOICE_CHUNKS: usize = VOICES / 8;
```

- [ ] **Step 2: Relax both asserts**

Replace `poly.rs:78` and `poly.rs:396`:

```rust
// The f32x8 poly path processes voices eight at a time — one NEON register —
// and iterates `VOICE_CHUNKS` times per sample. Any positive multiple of 8
// works; anything else would leave a ragged tail with no scalar fallback.
#[cfg(feature = "simd")]
const _: () = assert!(VOICES % 8 == 0 && VOICES > 0);
```

Also update the comment above the first one, which currently says *"Changing
VOICES requires revisiting the SIMD width (e.g. f32x16 or 2× f32x8)"* — that is
exactly what this phase does, so it should now say so.

- [ ] **Step 3: Verify nothing moved**

```bash
cd ~/GitHub/flare
tools/test.sh 2>&1 | tail -2
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
```
Expected: green, digests unchanged. Nothing consumes `VOICE_CHUNKS` yet.

- [ ] **Step 4: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "refactor: introduce VOICE_CHUNKS and relax the width asserts

VOICES stays 8 and VOICE_CHUNKS is 1, so nothing changes yet — which is the
point: the scaffolding lands separately from the kernel edits that use it.

The asserts now permit any positive multiple of 8 rather than exactly 8. The
vector width and the voice count are deliberately different numbers: eight is
one NEON register, and a wider voice count should run that same fast path more
times per sample rather than fall back to scalar."
```

---

### Task 4: Chunk `PolyOsc` — the template

The simplest of the five, and the pattern every later kernel copies. Its state
is already `[f32; VOICES]`; only the SIMD path's `from_array` needs widening.

**Files:**
- Modify: `flare-kernels/src/poly.rs`, `PolyOsc::process` (SIMD arm, ~127-183)

**Interfaces:**
- Consumes: `VOICE_CHUNKS`.
- Produces: the chunking idiom Tasks 5-6 follow.

- [ ] **Step 1: Rewrite the SIMD arm as a chunk loop**

The current arm hoists state into registers, runs the sample loop, then writes
state back. Keep that shape and put the chunk loop **outside** the sample loop,
so per-chunk state stays register-resident across the whole block rather than
being reloaded every sample:

```rust
        #[cfg(feature = "simd")]
        {
            use core::simd::prelude::*;
            let n = out.len() / VOICES;
            let one = f32x8::splat(1.0);
            let dtv = f32x8::splat(dt);
            let half = f32x8::splat(0.5);
            let fbv = f32x8::splat(self.feedback);
            // Voices are independent, so each chunk of 8 can run the whole
            // block before the next starts. That keeps its phase/feedback state
            // in registers for the duration instead of reloading it per sample.
            for c in 0..VOICE_CHUNKS {
                let base = c * 8;
                let mut ph = f32x8::from_slice(&self.phase[base..]);
                let mut last = f32x8::from_slice(&self.last[base..]);
                let mut last2 = f32x8::from_slice(&self.last2[base..]);
                for i in 0..n {
                    let o = i * VOICES + base;
                    let f = f32x8::from_slice(&pitch[o..]);
                    let w = f32x8::from_slice(&width[o..]);
                    let pm_v = f32x8::from_slice(&pm[o..]);
                    let dtp = f * dtv;
                    let mut p = ph + dtp;
                    let t: f32x8 = p.cast::<i32>().cast::<f32>();
                    let fl = t.simd_gt(p).select(t - one, t);
                    p -= fl;
                    ph = p;
                    let mut rp = p + pm_v + fbv * half * (last + last2);
                    let tr: f32x8 = rp.cast::<i32>().cast::<f32>();
                    let flr = tr.simd_gt(rp).select(tr - one, tr);
                    rp -= flr;
                    let y = wave_sample_x8(self.shape, rp, dtp, w);
                    y.copy_to_slice(&mut out[o..]);
                    last2 = last;
                    last = y;
                }
                ph.copy_to_slice(&mut self.phase[base..]);
                last.copy_to_slice(&mut self.last[base..]);
                last2.copy_to_slice(&mut self.last2[base..]);
            }
        }
```

Two mechanical points that matter. `from_array(self.phase)` becomes
`from_slice(&self.phase[base..])`, which takes the first 8 elements of the slice
and so works at any `VOICES`. And every tile index gains `+ base`, which is what
makes chunk `c` touch lanes `8c..8c+8` and no others.

- [ ] **Step 2: The gate — default output unmoved**

```bash
cd ~/GitHub/flare
cargo test -p flare-graph --features simd golden 2>&1 | grep -E '^test result'
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
```
Expected: 9 passed, digests unchanged. At `VOICES == 8` the loop runs once with
`base == 0`, so every expression is identical to before. **A failure here means
the rewrite changed an expression, not just its indexing.**

- [ ] **Step 3: Full runner**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | tail -2
```
Expected: "All tests passed."

- [ ] **Step 4: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "refactor(poly): chunk PolyOsc's SIMD path over VOICE_CHUNKS

The template the other four SIMD poly kernels follow. from_array(self.phase)
becomes from_slice(&self.phase[base..]) so it works at any VOICES, and every
tile index gains + base so chunk c touches lanes 8c..8c+8 and no others.

The chunk loop sits outside the sample loop rather than inside it: voices are
independent, so each chunk can run the whole block with its phase and feedback
state register-resident, instead of reloading state every sample.

At VOICES == 8 the loop runs once with base == 0 and every expression is
unchanged, which is why all 10 golden digests still match."
```

---

### Task 5: Chunk `PolySvf` and `PolyMoog`

Both hold **SoA `f32x8` state arrays**, so their state gains a chunk dimension.
`PolyMoog`'s is the invasive one: `[f32x8; POLES]` becomes
`[[f32x8; VOICE_CHUNKS]; POLES]`.

**Files:**
- Modify: `flare-kernels/src/poly.rs` — `PolySvf` (403-460), the free ladder
  helper (~470-510), `PolyMoog<POLES>` (512-585)

**Interfaces:**
- Consumes: `VOICE_CHUNKS`, the Task 4 idiom.
- Produces: nothing new.

- [ ] **Step 1: Read both kernels before editing**

```bash
cd ~/GitHub/flare && sed -n '395,585p' crates/flare-kernels/src/poly.rs
```
Note in particular the free helper at ~470 taking `state: &mut [f32x8; STAGES]`.
It is called **per chunk**, so its signature does not change — only its caller
passes a different sub-array.

- [ ] **Step 2: Give the state a chunk dimension**

`PolySvf`'s `#[cfg(feature = "simd")]` state fields and `PolyMoog`'s
`state: [core::simd::f32x8; POLES]` become chunk-indexed:

```rust
    #[cfg(feature = "simd")]
    state: [[core::simd::f32x8; VOICE_CHUNKS]; POLES],
```

and their `new()` initialisers become
`[[core::simd::f32x8::splat(0.0); VOICE_CHUNKS]; POLES]`.

- [ ] **Step 3: Wrap both process bodies in the chunk loop**

Same shape as Task 4: `for c in 0..VOICE_CHUNKS { let base = c * 8; … }`, every
tile index `i * VOICES` becoming `i * VOICES + base`, and every state access
gaining `[c]`. For `PolyMoog`, the ladder helper is called with
`&mut per_pole_state_for_chunk`, which needs a small transpose — build a
`[f32x8; POLES]` for chunk `c`, run the helper, write it back — or restructure
the array as `[[f32x8; POLES]; VOICE_CHUNKS]` so the chunk's poles are already
contiguous. **Prefer the latter**: it keeps the helper call free of copying and
makes the chunk the outer index everywhere, matching Task 4.

- [ ] **Step 4: The gate**

```bash
cd ~/GitHub/flare
cargo test -p flare-graph --features simd golden 2>&1 | grep -E '^test golden|^test result'
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
cargo test -p flare-kernels --features simd polymoog 2>&1 | grep -E '^test result'
cargo test -p flare-kernels --features simd polysvf 2>&1 | grep -E '^test result'
```
Expected: `golden_poly_moog_lp4` passes, digests unchanged, and both scalar-oracle
equivalence tests pass. The oracle tests are the sharper check here — a ladder
that reads the wrong chunk diverges rather than drifting.

- [ ] **Step 5: Full runner, then commit**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | tail -2
git add -A && git commit -m "refactor(poly): chunk PolySvf and PolyMoog

Both hold SoA f32x8 state, so it gains a chunk dimension. PolyMoog's is stored
[[f32x8; POLES]; VOICE_CHUNKS] rather than the transpose, so a chunk's poles
stay contiguous and the free ladder helper keeps its &mut [f32x8; STAGES]
signature with no copying at the call site.

Ladder state feeds back per sample, so a lane reading the wrong chunk would
diverge rather than drift — which is why the scalar-oracle equivalence tests
matter more here than the digests. Both pass, and the digests are unchanged."
```

---

### Task 6: Chunk `PolyMs20` and `PolySync`

Both hold **scalar `f32x8` fields** rather than arrays, so each becomes
`[f32x8; VOICE_CHUNKS]`. `PolySync` is also the kind whose digest is
config-divergent, so read `golden.rs`'s note before touching it.

**Files:**
- Modify: `flare-kernels/src/poly.rs` — `PolyMs20` (587-717), `PolySync` (719-816)

**Interfaces:**
- Consumes: `VOICE_CHUNKS`, the Task 4 idiom.
- Produces: nothing new.

- [ ] **Step 1: Widen the state fields**

`PolyMs20`'s `ic1`, `ic2`, `dc_x`, `dc_y` and `PolySync`'s `master_phase`,
`slave_phase` all become `[core::simd::f32x8; VOICE_CHUNKS]`, initialised
`[f32x8::splat(0.0); VOICE_CHUNKS]`.

- [ ] **Step 2: Wrap both process bodies**

Same idiom. Note `PolyMs20` also carries a scalar `cached_dt` and `dc_a` — those
are **shared across chunks**, not per-chunk, so they stay scalar. Recomputing
coefficients inside the chunk loop would be wasted work; hoist the coefficient
computation above it.

- [ ] **Step 3: The gate**

```bash
cd ~/GitHub/flare
cargo test -p flare-graph --features simd golden 2>&1 | grep -E '^test golden|^test result'
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
cargo test -p flare-kernels --features simd polysync 2>&1 | grep -E '^test result'
```
Expected: `golden_poly_ms20_lp` and `golden_poly_sync_saw` pass, digests
unchanged.

`golden_poly_sync_saw` pins **one digest per configuration** because `PolySync`'s
scalar and SIMD paths reorder arithmetic (see `golden.rs`'s module doc). Both
constants must still hold. If only the simd one moves, the chunking changed an
expression; if both move, something outside `PolySync` did.

- [ ] **Step 4: Confirm every `f32x8` site is now chunked**

```bash
cd ~/GitHub/flare
grep -n 'f32x8' crates/flare-kernels/src/poly.rs | grep -vE 'VOICE_CHUNKS|base|splat|f32x8::from_slice|copy_to_slice|-> f32x8|: f32x8|use core::simd'
```
Read every remaining hit. Any `from_array(` on a `[f32; VOICES]`, or any tile
index without `+ base`, is a site the chunk loop missed and will silently
process only the first eight voices.

- [ ] **Step 5: Full runner, then commit**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | tail -2
git add -A && git commit -m "refactor(poly): chunk PolyMs20 and PolySync

Their scalar f32x8 fields become [f32x8; VOICE_CHUNKS]. PolyMs20's cached_dt
and dc_a stay scalar — they are shared across chunks, so the coefficient
computation is hoisted above the chunk loop rather than repeated in it.

PolySync's golden pins one digest per configuration because its scalar and SIMD
paths reorder arithmetic; both constants still hold, which is the check that
the chunking changed indexing and not expressions."
```

---

### Task 7: The `voices-*` features

The scaffolding and the kernels are ready; this makes a wider voice count
selectable.

**Files:**
- Modify: `flare-kernels/src/poly.rs:28`, `flare-kernels/Cargo.toml`,
  `flare-graph/Cargo.toml`, `flare/Cargo.toml`

**Interfaces:**
- Consumes: Tasks 3-6.
- Produces: features `voices-16` / `voices-24` (default 8), forwarded through
  `flare-graph` and `flare`.

- [ ] **Step 1: Make `VOICES` feature-selected**

```rust
/// Voices processed in parallel per poly node. Fixed at compile time.
///
/// Constrained to a multiple of 8 — see [`VOICE_CHUNKS`]. Wider costs CPU and
/// per-node state linearly; it does not change the number of graph walks,
/// because polyphony here is a property of the signal's width rather than of
/// the graph's shape.
// Largest-first with explicit `not(...)` guards, for the same reason as
// `MAX_BLOCK`: Cargo features are additive, so bare `#[cfg(feature = ...)]`
// arms would both compile if two were enabled anywhere in the graph.
#[cfg(feature = "voices-24")]
pub const VOICES: usize = 24;
#[cfg(all(feature = "voices-16", not(feature = "voices-24")))]
pub const VOICES: usize = 16;
#[cfg(not(any(feature = "voices-16", feature = "voices-24")))]
pub const VOICES: usize = 8;
```

- [ ] **Step 2: Declare and forward the features**

`flare-kernels/Cargo.toml`:

```toml
## Voices per poly node. 8 unless one of these is set; must be a multiple of 8,
## since the SIMD path processes one NEON register (eight lanes) at a time.
voices-16 = []
voices-24 = []
```

`flare-graph/Cargo.toml`:

```toml
voices-16 = ["flare-kernels/voices-16"]
voices-24 = ["flare-kernels/voices-24"]
```

`flare/Cargo.toml`:

```toml
voices-16 = ["flare-graph/voices-16", "flare-kernels/voices-16"]
voices-24 = ["flare-graph/voices-24", "flare-kernels/voices-24"]
```

- [ ] **Step 3: The default must not move**

```bash
cd ~/GitHub/flare
tools/test.sh 2>&1 | tail -2
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
```

- [ ] **Step 4: The wider configurations must build and pass their oracle**

```bash
cd ~/GitHub/flare
for v in voices-16 voices-24; do
  for cfg in "" "--features simd"; do
    echo "== $v ${cfg:-scalar}"
    cargo test -p flare-kernels --features "$v${cfg:+,simd}" 2>&1 | grep -E '^test result|^error' | head -3
  done
done
```

Expected: all pass. **The scalar-oracle equivalence tests are the real check
here** — the goldens cannot help, because sixteen voices legitimately sum to
different audio. If `polymoog_matches_scalar_oracle_both_slopes` or
`polysync_matches_scalar_oracle_all_waves` fails at `voices-16`, the chunking is
wrong and the goldens would never have told you.

- [ ] **Step 5: The graph and facade at wider widths**

```bash
cd ~/GitHub/flare
for v in voices-16 voices-24; do
  cargo test -p flare-graph --features "$v" 2>&1 | grep -E '^test result: FAILED|^error' | head -3
  cargo test -p flare-graph --features "$v,simd" 2>&1 | grep -E '^test result: FAILED|^error' | head -3
done
```

**Expect the goldens to FAIL here, and that is correct** — a 16-voice render of
a poly patch is different audio. Confirm the failures are *only* golden tests
and that every non-golden test passes:

```bash
cargo test -p flare-graph --features voices-16 2>&1 | grep '^test .* FAILED' | sort
```
Expected: only `golden::golden_poly_*` names. A non-golden failure at
`voices-16` is a real bug. Handle the goldens in Task 8.

- [ ] **Step 6: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "feat: make VOICES selectable in multiples of 8

voices-16 and voices-24 alongside the default 8, forwarded through flare-graph
and the facade. Multiples of 8 because the SIMD path processes one NEON
register at a time and iterates VOICE_CHUNKS times per sample — a wider voice
count runs the fast path more often rather than falling back to scalar.

The default's output is unchanged, all 10 digests byte-identical. At the wider
widths the scalar-oracle equivalence tests are what verify the chunking: the
goldens cannot, because sixteen voices legitimately sum to different audio."
```

---

### Task 8: The chunk-boundary golden

Nothing existing distinguishes "voice 8" from "the first lane of chunk 1",
because until now there was only one chunk. This is the test that does.

**Files:**
- Modify: `flare-graph/src/golden.rs`

**Interfaces:**
- Consumes: Tasks 3-7.
- Produces: `golden_voices_are_independent_across_chunk_boundary`.

- [ ] **Step 1: Write a test that is meaningful at every width**

A digest cannot be used here — it differs per width. Assert a *property*
instead: driving exactly one voice must produce the same output whichever voice
it is, because voices are independent.

Append to `crates/flare-graph/src/golden.rs`:

```rust
/// Voices must be independent, including across an `f32x8` chunk boundary.
///
/// Not a digest: this must hold at every `VOICES` setting, and a digest differs
/// per width. It asserts the property instead — drive exactly one voice, and
/// the rendered output must be the same whichever voice you picked. A chunking
/// bug that makes lane 8 read chunk 0's state shows up here and, until
/// `VOICES > 8` existed, nowhere at all.
#[test]
fn golden_voices_are_independent_across_chunk_boundary() {
    fn render_one_voice(v: usize) -> [StereoFrame; 256] {
        let mut e = G::new(48_000.0);
        e.create(NodeId(0), Kind::PolyCtrl);
        // Silence every voice but `v`. 0 Hz holds phase, so an unselected lane
        // contributes a constant rather than a tone; the selected lane is the
        // only thing that can vary between runs.
        for i in 0..crate::VOICES {
            e.apply(Cmd::SetParam {
                node: NodeId(0),
                param: i as u8,
                value: if i == v { 220.0 } else { 0.0 },
            });
        }
        e.create(NodeId(1), Kind::PolyOsc);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node { node: NodeId(0), port: 0 };
        e.create(NodeId(2), Kind::VoiceSum);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node { node: NodeId(1), port: 0 };
        e.apply(Cmd::SetParam { node: NodeId(2), param: 0, value: 0.1 });
        e.apply(Cmd::BusWrite {
            src: Input::Node { node: NodeId(2), port: 0 },
            bus: BusId(0),
        });
        e.apply(Cmd::SetRoot { bus: BusId(0) });
        let mut out = [StereoFrame::default(); 256];
        e.render_offline(&mut out);
        out
    }

    let reference = crate::nrt::digest(&render_one_voice(0));
    for v in 1..crate::VOICES {
        assert_eq!(
            crate::nrt::digest(&render_one_voice(v)),
            reference,
            "voice {v} rendered differently from voice 0; \
             lanes are not independent (chunk boundary is at every multiple of 8)"
        );
    }
}
```

- [ ] **Step 2: Run it at the default width**

```bash
cd ~/GitHub/flare
cargo test -p flare-graph golden_voices_are_independent 2>&1 | grep -E '^test result'
cargo test -p flare-graph --features simd golden_voices_are_independent 2>&1 | grep -E '^test result'
```
Expected: PASS in both. At `VOICES == 8` this exercises one chunk and proves the
test itself is sound.

- [ ] **Step 3: Run it at the widths that actually have a boundary**

```bash
cd ~/GitHub/flare
for v in voices-16 voices-24; do
  for cfg in "" ",simd"; do
    printf '%-12s %-6s ' "$v" "${cfg:+simd}"
    cargo test -p flare-graph --features "$v$cfg" golden_voices_are_independent 2>&1 | grep -E '^test result'
  done
done
```
Expected: PASS in all four. **This is the single most valuable check in Phase 3**
— it is the only thing that directly tests the bug class the chunking
introduces.

- [ ] **Step 4: Prove the test can fail**

A test that has never failed is a test nobody knows works. Temporarily break one
chunk's base offset in `PolyOsc` — change `let base = c * 8;` to `let base = 0;`
— and run at `voices-16`:

```bash
cd ~/GitHub/flare
cargo test -p flare-graph --features voices-16,simd golden_voices_are_independent 2>&1 | grep -E '^test result'
```
Expected: **FAILED.** Then revert and confirm it passes again. If it still
passes with `base = 0`, the test is not reaching the chunk logic and needs
rethinking before it is trusted.

- [ ] **Step 5: Handle the goldens at wider widths**

The poly digest goldens fail at `voices-16` by design. Gate them so the suite is
honest rather than noisy — add above each poly digest golden:

```rust
// Digest goldens pin the default voice count: a wider VOICES sums more voices
// and legitimately renders different audio. Width-independent behaviour is
// covered by `golden_voices_are_independent_across_chunk_boundary` and by the
// kernels' scalar-oracle equivalence tests.
#[cfg(not(any(feature = "voices-16", feature = "voices-24")))]
```

- [ ] **Step 6: Verify every configuration is now green**

```bash
cd ~/GitHub/flare
for feats in "" "simd" "voices-16" "voices-16,simd" "voices-24" "voices-24,simd"; do
  printf '%-20s ' "${feats:-default}"
  n=$(cargo test -p flare-graph ${feats:+--features "$feats"} 2>&1 | grep -c '^test result: FAILED')
  [ "$n" = 0 ] && echo "clean" || echo "$n FAILING"
done
tools/test.sh 2>&1 | tail -2
```
Expected: no FAILED lines anywhere, and the runner green.

- [ ] **Step 7: Add the wider widths to the runner**

In `flare/tools/test.sh`, after the block-ceiling loop:

```bash
echo "==> Host bucket, wider voice counts"
for v in voices-16 voices-24; do
  cargo test -p flare-kernels --features "$v"
  cargo test -p flare-kernels --features "$v,simd"
  cargo test -p flare-graph   --features "$v,simd"
done
```

- [ ] **Step 8: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "test: assert voices stay independent across a chunk boundary

Nothing distinguished 'voice 8' from 'the first lane of chunk 1', because until
this phase there was only one chunk. This asserts a property rather than a
digest, so it holds at every VOICES setting: drive exactly one voice and the
render must be identical whichever voice you picked.

Verified able to fail rather than assumed to be: forcing base = 0 in PolyOsc's
chunk loop fails it at voices-16.

The poly digest goldens are gated to the default width — sixteen voices
legitimately sum to different audio, so pinning them there would assert
nothing useful."
```

---

### Task 9: Scoped updates (spec §3a)

`end_update` sweeps globally: any live node not stamped with the current epoch is
freed, so `BeginUpdate`/`EndUpdate` requires re-emitting the **whole** graph.
Instrument A cannot be live-edited without re-emitting instrument B. This makes
the sweep scopeable, additively.

**Files:**
- Modify: `flare-graph/src/engine.rs` (fields ~119-123, `end_update` 531,
  `stamp` 557, `apply` arms ~818, `Reset` ~856), `flare-graph/src/cmd.rs`

**Interfaces:**
- Consumes: nothing from Phase 3.
- Produces: `Cmd::BeginUpdateScoped { scope: u8 }`. `Cmd::BeginUpdate` is
  unchanged and still sweeps globally.

- [ ] **Step 1: Write the failing test**

Add to `engine.rs`'s test module:

```rust
#[test]
fn a_scoped_update_leaves_other_scopes_alone() {
    let mut e = E::new(48_000.0);
    // Instrument A in scope 1, instrument B in scope 2.
    e.apply(Cmd::BeginUpdateScoped { scope: 1 });
    e.create(NodeId(0), Kind::Saw);
    e.apply(Cmd::EndUpdate);
    e.apply(Cmd::BeginUpdateScoped { scope: 2 });
    e.create(NodeId(1), Kind::Square);
    e.apply(Cmd::EndUpdate);
    assert!(e.arena.node(NodeId(0)).is_some(), "A survived its own update");
    assert!(e.arena.node(NodeId(1)).is_some(), "B survived its own update");

    // Re-emit only scope 1. B must be untouched — this is the whole point.
    e.apply(Cmd::BeginUpdateScoped { scope: 1 });
    e.create(NodeId(0), Kind::Saw);
    e.apply(Cmd::EndUpdate);
    assert!(e.arena.node(NodeId(0)).is_some(), "A re-emitted");
    assert!(
        e.arena.node(NodeId(1)).is_some(),
        "B was swept by an update it had nothing to do with"
    );
}

#[test]
fn an_unscoped_update_still_sweeps_everything() {
    // The pre-existing contract, unchanged: BeginUpdate re-emits the whole
    // graph, so anything not re-emitted is freed.
    let mut e = E::new(48_000.0);
    e.create(NodeId(0), Kind::Saw);
    e.create(NodeId(1), Kind::Square);
    e.apply(Cmd::BeginUpdate);
    e.create(NodeId(0), Kind::Saw);
    e.apply(Cmd::EndUpdate);
    assert!(e.arena.node(NodeId(0)).is_some());
    assert!(e.arena.node(NodeId(1)).is_none(), "unscoped sweep still global");
}
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cd ~/GitHub/flare && cargo test -p flare-graph scoped_update 2>&1 | grep -E 'error|^test result'
```
Expected: a compile error — `Cmd::BeginUpdateScoped` does not exist.

- [ ] **Step 3: Add the command**

In `cmd.rs`, beside `BeginUpdate`:

```rust
    /// Begin an update confined to one scope (spec §3a).
    ///
    /// Like [`Cmd::BeginUpdate`], but [`Cmd::EndUpdate`] then sweeps only nodes
    /// belonging to `scope`. That lets a host live-edit one instrument's patch
    /// without re-emitting every other instrument, which an unscoped update
    /// requires. Scope `0` is the default every node gets, so an unscoped
    /// update remains exactly what it was.
    BeginUpdateScoped {
        scope: u8,
    },
```

- [ ] **Step 4: Add the scope state and the scoped sweep**

In `engine.rs`, beside `node_epoch: [u8; NODES]`:

```rust
    // `node_scope[i]` is the scope node `i` was created in; 0 unless a
    // `BeginUpdateScoped` was active. A parallel byte array rather than a
    // seventh const-generic: scopes label existing nodes, they do not allocate.
    node_scope: [u8; NODES],
    // The scope of the update in progress, and whether it is scoped at all.
    update_scope: Option<u8>,
```

`stamp` records the active scope alongside the epoch; `end_update` skips nodes
outside the active scope:

```rust
    fn end_update(&mut self) {
        self.in_update = false;
        let scope = self.update_scope.take();
        for idx in 0..NODES {
            let id = NodeId(idx as u16);
            if let Some(s) = scope {
                if self.node_scope[idx] != s {
                    continue;
                }
            }
            if self.arena.node(id).is_some() && self.node_epoch[idx] != self.epoch {
                self.free_node(id);
                self.events.push(crate::event::Event::Freed { node: id });
            }
        }
    }
```

`Cmd::Reset` must clear `node_scope` alongside `node_epoch`.

- [ ] **Step 5: Run the tests**

```bash
cd ~/GitHub/flare && cargo test -p flare-graph update 2>&1 | grep -E '^test result'
```
Expected: both new tests pass, and every pre-existing update test still passes —
that second part is what proves this was additive.

- [ ] **Step 6: The gate, and the full runner**

```bash
cd ~/GitHub/flare
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "DIGESTS UNCHANGED"
tools/test.sh 2>&1 | tail -2
```

- [ ] **Step 7: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "feat: scoped patch updates

end_update swept globally, so BeginUpdate/EndUpdate required re-emitting the
entire graph — one instrument could not be live-edited without re-emitting
every other one. For a single script owning the whole graph that is fine; for a
host loading independently authored patches it is a blocker.

Cmd::BeginUpdateScoped { scope } confines the sweep. Scope 0 is the default
every node gets and Cmd::BeginUpdate is untouched, so this is purely additive —
the test that an unscoped update still sweeps everything is as load-bearing as
the one that a scoped update does not.

Node scopes are a parallel [u8; NODES] rather than a seventh const-generic:
they label existing nodes, they do not allocate."
```

---

### Task 10: Multi-engine, proven (spec §3b) and the Phase 3 exit gate

`Engine` × N already works — there is no global mutable state anywhere in either
crate — but nothing exercises it, so nobody knows.

**Files:**
- Create: `flare/tests/multi_engine.rs`
- Modify: `flare/src/lib.rs` (document the trade-off), the spec

**Interfaces:**
- Consumes: everything above.
- Produces: the recorded Phase 3 result.

- [ ] **Step 1: Write the multi-engine test**

Create `crates/flare/tests/multi_engine.rs`:

```rust
//! Two independent engines, summed by the host.
//!
//! There is no global mutable state in flare — the only statics are the
//! immutable generated wavetables — so `Engine` × N works today. Nothing
//! exercised it, which meant nobody knew. This does.
//!
//! It is the alternative to `Cmd::BeginUpdateScoped`: multi-engine gives hard
//! isolation and independent capacities, at the cost of N topological sorts, N×
//! worst-case preallocated memory, and no cross-engine buses. Scoped updates
//! give shared routing and one allocation, without isolation.

use flare::{BusId, Cmd, Engine, Input, Kind, NodeId, StereoFrame};

/// Deliberately different const-generic capacities, to prove the engines are
/// genuinely independent instantiations rather than one shape used twice.
type Small = Engine<64, 8, 16, 2, 45056, 2048>;
type Large = Engine<64, 16, 64, 4, 45056, 2048>;

/// Build the same saw patch in whichever engine. A macro rather than a trait or
/// a generic function because `Engine`'s const-generic parameters differ
/// between the two types and there is no trait to bound them by — three lines
/// of repetition would do just as well, and this is the shorter of the two.
macro_rules! saw_patch {
    ($e:expr, $hz:expr) => {{
        $e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Saw,
            args: [Input::Const($hz), Input::Const(0.0), Input::Const(0.0)],
        });
        $e.apply(Cmd::BusWrite {
            src: Input::Node { node: NodeId(0), port: 0 },
            bus: BusId(0),
        });
        $e.apply(Cmd::SetRoot { bus: BusId(0) });
    }};
}

#[test]
fn two_engines_render_independently_and_sum() {
    let mut a = Small::new(48_000.0);
    let mut b = Large::new(48_000.0);
    saw_patch!(a, 110.0);
    saw_patch!(b, 220.0);

    let mut out_a = [StereoFrame::default(); 512];
    let mut out_b = [StereoFrame::default(); 512];
    a.render_offline(&mut out_a);
    b.render_offline(&mut out_b);

    assert!(flare::nrt::is_clean(&out_a) && flare::nrt::is_clean(&out_b));
    assert!(flare::nrt::peak(&out_a) > 1e-3 && flare::nrt::peak(&out_b) > 1e-3);
    assert_ne!(
        flare::nrt::digest(&out_a),
        flare::nrt::digest(&out_b),
        "different patches must render differently"
    );

    // The host's job: sum them.
    let mixed: [StereoFrame; 512] = core::array::from_fn(|i| StereoFrame {
        l: (out_a[i].l + out_b[i].l) * 0.5,
        r: (out_a[i].r + out_b[i].r) * 0.5,
    });
    assert!(flare::nrt::is_clean(&mixed));
    assert!(flare::nrt::peak(&mixed) > 1e-3);
}

#[test]
fn an_engine_is_unaffected_by_another_engines_edits() {
    // Pin what a lone engine renders...
    let mut reference = Large::new(48_000.0);
    saw_patch!(reference, 110.0);
    let mut out = [StereoFrame::default(); 256];
    reference.render_offline(&mut out);
    let pinned = flare::nrt::digest(&out);

    // ...then render the same patch with a second engine alive beside it,
    // being reset and rebuilt throughout. If any state were shared, `a` would
    // notice.
    let mut a = Large::new(48_000.0);
    let mut b = Large::new(48_000.0);
    saw_patch!(a, 110.0);
    saw_patch!(b, 880.0);
    let mut scratch = [StereoFrame::default(); 256];
    b.render_offline(&mut scratch);
    b.apply(Cmd::Reset);
    saw_patch!(b, 55.0);
    b.render_offline(&mut scratch);

    let mut after = [StereoFrame::default(); 256];
    a.render_offline(&mut after);
    assert_eq!(
        flare::nrt::digest(&after),
        pinned,
        "another engine's rebuild changed this one's output; state is shared"
    );
}
```

- [ ] **Step 2: Run it**

```bash
cd ~/GitHub/flare && cargo test -p flare --test multi_engine 2>&1 | grep -E '^test |^test result|^error'
```
Expected: 2 passed.

- [ ] **Step 3: Document the choice in the facade**

Add to `crates/flare/src/lib.rs`'s module doc:

```rust
//! # Several patches at once
//!
//! Two ways, with a real trade-off between them.
//!
//! **One engine, many subgraphs.** Independent subgraphs coexist in one arena,
//! each writing its own bus; buses fan in and sum to the root. Polyphony is per
//! poly node, so two instruments get `VOICES` voices *each*, not `VOICES`
//! between them. Use [`Cmd::BeginUpdateScoped`] to live-edit one without
//! re-emitting the others. Shared routing, one allocation, one topological
//! sort — but no isolation: a runaway patch can exhaust the shared arena.
//!
//! **Many engines.** `Engine` values share no state (the only statics here are
//! immutable wavetables), so N of them can run and be summed by the host. Hard
//! isolation and independent capacities, at the cost of N topological sorts,
//! N× worst-case preallocated memory, and no cross-engine buses.
```

- [ ] **Step 4: The Phase 3 exit gate**

```bash
cd ~/GitHub/flare
echo "[1] default digests unchanged:"
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase3.txt && echo "    PASS"
echo "[2] flare runner:"; tools/test.sh >/dev/null 2>&1 && echo "    PASS"
echo "[3] deluge-sdk runner:"; (cd ~/GitHub/deluge-sdk && tools/test.sh >/dev/null 2>&1) && echo "    PASS"
echo "[4] chunk-boundary test at every width:"
for v in "" voices-16 voices-24; do
  cargo test -p flare-graph ${v:+--features $v,simd} golden_voices_are_independent >/dev/null 2>&1 && echo "    ${v:-default} PASS"
done
echo "[5] regression net still catches:"
sed -i '72s/0\.225/0.226/' crates/flare-kernels/src/lib.rs
cargo test -p flare-graph golden 2>&1 | grep '^test result'
git checkout crates/flare-kernels/src/lib.rs
```
Expected: [1]-[4] all PASS, and [5] shows 5 failures as in Phases 1 and 2.

- [ ] **Step 5: Record the result in the spec**

Append to the spec's `### Phase 3` subsection: which configurations now exist,
that the default's digests are unchanged, the chunk-boundary test's result at
each width, and — importantly — any place where the chunking turned out to be
harder than this plan assumed, since Phase 4 will build on it.

- [ ] **Step 6: Commit both repos**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "test: prove Engine x N works, and document the choice

There is no global mutable state in flare, so multiple engines have always
worked; nothing exercised it, which meant nobody knew. Two engines with
different const-generic capacities now render independently and sum, and a
second test shows one engine's full rebuild leaves another's output bit-identical.

The facade documents the trade-off rather than implying one right answer:
one engine with scoped updates gives shared routing and a single allocation;
many engines give hard isolation at N topological sorts and N x the memory."
```

---

## What this plan does NOT cover

- **Phase 4** — the four-target CI matrix (`armv7a-none-eabihf`, host, wasm32,
  thumbv7em). Phase 3 adds configurations; Phase 4 is where they get run
  everywhere.
- **Publishing flare.** Still a local repo with no remote.
- **`f32x16` or wider vectors.** `VOICE_CHUNKS` iterates `f32x8` because that is
  one NEON register. A machine with wider vectors would want a different chunk
  width, which is a separate change with its own equivalence testing.
- **A `voices-32` or larger.** The features stop at 24 because nothing has asked
  for more and each one is a configuration that must be tested. Adding one is a
  two-line change once something needs it.
- **Making `AUX_OUTS` configurable.** It is a rename this phase, not a knob.
  Nothing has needed a different count.
