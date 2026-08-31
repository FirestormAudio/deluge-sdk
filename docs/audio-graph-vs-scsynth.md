# `deluge-audio-graph` vs. scsynth — capability gap analysis

**Status:** reference note, not a plan. Written 2026-08-31 against
`crates/deluge-audio-graph` at commit `8e497f4`.

**Why this exists:** we evaluated adopting [Supersonic] (SuperCollider's scsynth
rearchitected for embedded/web/NIF hosts) as an alternative engine backend and
decided against replacing our own. That decision is only defensible if we're
honest about what scsynth's 25 years bought it that we don't have. This is that
list, plus the inverse — what our design does that scsynth structurally cannot.

Every claim below cites a line so it can be re-checked as the crate moves.

[Supersonic]: https://github.com/samaaron/supersonic

---

## The architectural difference in one sentence

scsynth is a general-purpose server — open UGen set, hierarchical node tree,
precompiled graph templates, arbitrary mono buses. `deluge-audio-graph` is a
fixed-capacity, closed-set, zero-allocation block renderer with **polyphony
built into the signal width**. Neither is a subset of the other.

---

## What we have that scsynth doesn't

### 1. Polyphony as a signal width

`Node::out_width` (`node.rs:301`) returns `VOICES` for 25 of the 80 kinds, and
`poly_in_count` (`node.rs:378`) marks which ports carry poly edges. The graph is
walked **once**, `VOICES` lanes wide.

scsynth's model is one `Synth` per voice: 8 voices means 8 graph instantiations,
8 node-tree walks, 8× the per-node dispatch and wire-buffer traffic. Ours is
what makes lane-parallel NEON viable and is the reason 8 voices are affordable
on a 400 MHz Cortex-A9. This is not retrofittable onto scsynth — its entire
model is mono wires.

### 2. Stereo is native

`StereoFrame` (`frame.rs`), stereo buses (`engine.rs:38`), constant-power `Pan`,
and `StereoVoiceSum` with per-lane pan. scsynth signals are strictly mono;
"stereo" there is a convention maintained by sclang, not the server.

### 3. Voice allocation lives in the engine crate

`VoiceAllocator` / `MonoAllocator` (`voice.rs`): oldest-LRU stealing,
release-tail awareness, unison with symmetric detune and stereo-width spread.
In SuperCollider all of this lives in sclang/patterns — off-server. Ours works
with no language runtime present at all, which matters when the control plane is
Wren or raw MIDI.

### 4. A curated musical DSP set scsynth core lacks

TB-303 ladder, Moog 2/4-pole, MS-20 Sallen-Key, Modal resonator, Dattorro plate,
FDN8 hall, Bitcrush, Decimate. scsynth core ships RLPF / Resonz / MoogFF /
FreeVerb; the rest is sc3-plugins territory, i.e. a separate dynamically-loaded
dependency.

### 5. Mip-mapped wavetables

`MipSet` / `LEVELS` / `level_offset` — a real band-limited pyramid with morph.
scsynth's `Osc` / `VOsc` read a raw buffer and alias at high pitch.

### 6. A master seam rather than master nodes

DC-block → EQ → limiter applied at the render boundary (`engine.rs`, fields
`master_dcblock` / `master_eq` / `master_limiter`), not as graph nodes. Cheaper,
and structurally impossible to mispatch.

### 7. No-panic, no-alloc discipline

Pool exhaustion returns `None` (`pool.rs`), arena exhaustion returns `false`
(`arena.rs`), a dangling `Input::Node` renders silence rather than panicking
(`engine.rs:308`). Total RAM is a compile-time constant — no fragmentation, no
runtime tuning. scsynth's `RTAlloc` failure drops a synth and logs.

### 8. SD streaming as a first-class node with a prefetch protocol

`Cmd::StreamFill` + per-voice `[fill_lo, fill_hi)` resident windows
(`stream.rs`). scsynth's `DiskIn` requires a `Buffer` plus the NRT thread.

---

## Gaps, ordered by how much they bite

### G1 — No engine→host event channel — ✅ **DONE**

`Host::audio_cmd` (`cmd.rs:116`) is one-way. There is no reply path anywhere in
`cmd.rs` / `engine.rs` / `lib.rs`. The engine never tells the control plane
anything: no `/n_end`, no `SendTrig`, no "this envelope finished."

**Concrete consequence.** `LaneState::Free` is documented as *"never used (never
reclaimed — the allocator has no time source)"* (`voice.rs:47`). Once all
`VOICES` lanes have played a note, every lane is permanently `Held` or
`Releasing`; `pick_lane` (`voice.rs:160`) falls through to stealing the
longest-released lane even when a genuinely silent one exists. It degrades
gracefully rather than breaking, but it will occasionally cut a long release
that didn't need cutting.

scsynth solves exactly this with `doneAction: 2` plus the `/n_end` notification.

**Resolved.** `event.rs` adds `Event::{Done, VoiceDone}` and a fixed-capacity
`EventQueue`; `Engine::collect_events` diffs each node's `Node::idle_mask`
against the previous block and enqueues rising edges only; the host drains via
`pop_event` / `drain_events`. `VoiceAllocator::on_event` reclaims a lane once
**every** configured gate reports for it, so a filter envelope finishing ahead
of the amp envelope no longer frees a sounding lane.

Two deliberate limits, both covered by tests:

- A `VoiceDone` for a `Held` lane does **not** free it — the key is still down,
  and freeing would let the next note steal a note the player is holding. True
  fire-and-forget (percussion, one-shots) wants the opposite and needs its own
  opt-in; that is follow-up work, not part of G1.
- Queue overflow drops the newest event and bumps `events_dropped()`. A dropped
  `VoiceDone` means a lane is not reclaimed that cycle — i.e. it degrades to the
  old stealing behaviour rather than misbehaving.

### G2 — No control rate

`In` is only `K(f32)` or `A(&[f32])` (`deluge-dsp-kernels/src/lib.rs:36`), and
`K` only ever originates from a literal `Input::Const`. There is no way for a
*node* to produce one value per block.

An LFO driving a filter cutoff therefore runs at full audio rate. In scsynth
that is `.kr` — one value per block, up to `BLOCK`× cheaper. On a modulation-heavy
patch this is the most likely place we are leaving CPU on the floor.

### G3 — No graph template / SynthDef

Every node is created individually by explicit `NodeId` via `Cmd::NewNode`.
There is no way to define a patch once and instantiate it N times. Anything that
should be a reusable instrument definition has to be re-emitted node-by-node
from the control plane.

scsynth's `/d_recv` + `/s_new` split is a better factoring and is what makes
"load a preset" cheap. A recorded `Cmd` sequence with `NodeId` relocation would
be a serviceable first version.

### G4 — Eval order is creation order, and cannot be changed

**Nothing sorts.** `Arena::create` appends to the order list and `free` does a
stable compaction. Correct signal flow is entirely the author's obligation — to
create nodes in dependency order.

*(G4a resolved: the doc comments in `lib.rs`, `arena.rs`, and `engine.rs` used
to claim "topological" order, which read as a promise the engine sorts for you.
They now say creation order and spell out the consequence. G4b — reordering
commands — is still open.)*

Consequences:

- You cannot insert a node into the middle of an existing chain.
- A freed-and-recreated `NodeId` jumps to the **end** of eval order, silently
  introducing a one-block delay in any path that reads it.
- There is no `/n_before` / `/n_after` / group-ordering equivalent.

For a live-patchable engine this will eventually hurt. At minimum the doc
comments should stop saying "topological."

### G5 — Hard three-input / three-arg ceiling

`MAX_INPUTS = 3` (`node.rs:48`) and `MAX_ARGS = 3` (`cmd.rs:8`). This rules out,
as single nodes: multi-input mixers, `Select` / `SelectX`, multi-tap structures,
and breakpoint envelopes. The workaround is chains of `Add`, which consumes
eval-order entries and arena output rows.

### G6 — No control buses, no `/n_map`

Buses are stereo **audio** pairs; reading one as an input sums L+R
(`engine.rs:313`), which is meaningless for a control signal. There is no shared,
named modulation signal that many nodes can subscribe to — fanning one modulator
to twenty destinations costs twenty explicit edges.

### G7 — No scheduled or timestamped commands

`Engine::apply` (`engine.rs:131`) lands a `Cmd` on whatever block it is called
in. scsynth's OSC bundle timestamps give block-accurate scheduling and immunity
to control-thread jitter. For a sequencer this matters more than it sounds.

### G8 — No groups

No node hierarchy, therefore no group free / pause / move, and no group-level
execution ordering.

### G9 — Closed UGen set

`Kind` (`node.rs:51`) is an 80-variant enum; the `Custom(dyn Ugen)` escape hatch
is explicitly *"reserved for a future open set (not built in P0)"* (`node.rs:4`).
Wren authors can patch but cannot write DSP.

**This is arguably the right call** — static dispatch is a large part of why the
engine is fast, and we ship a closed product. Listed for completeness, not as a
defect.

### G10 — Assorted smaller gaps

- No NRT / offline render.
- No recording (`DiskOut` equivalent).
- No buffer generators (`/b_gen`: `sine1`, `cheby`, wavetable normalization).
- `MAX_BLOCK = 128` cap (`node.rs:1346`).
- Fixed graph capacity: `NODES` / `OUTS` are const-generic, allocation is
  first-fit, exhaustion is silent (returns `false`).

---

## Assessment

None of G1–G10 indicates a wrong design. They are features scsynth grew because
it is a *general* server, and every one of them is **additive** — none requires
changing the poly-width core that is our actual structural advantage.

### The sequencing fact that matters

**There is no production `Engine<…>` instantiation in the repo.** Every one of
the ~20 instantiations is inside a `#[cfg(test)]` block, and
`deluge-wren-core`'s audio layer is engine-agnostic — it emits `Cmd`s and lets
the host own the `Engine` (`audio.rs:10-12` only assert capacity minimums).

So the const-generic sizing, the stack budget, and the `Cmd` surface are all
still **unfrozen**. Anything on this list is cheaper to do now than after the
first shipping app pins the API. That argues for doing the structural items
(G1, G4b, G2) before wiring the engine into a real app, not after.

---

## Priority order

Ranked by value ÷ effort, with dependencies respected. "Effort" is relative,
not calendar time.

| # | Gap | Value | Effort | Notes |
|---|-----|-------|--------|-------|
| ~~P0~~ | ~~G4a — fix misleading doc comments~~ | med | trivial | ✅ **done** |
| ~~P1~~ | ~~G1 — engine→host event channel~~ | **high** | **low** | ✅ **done** — `event.rs` |
| **P2** | G4b — node reordering commands | high | low | **next** — order list is already explicit |
| **P3** | G2 — control rate | high | med | Exploits existing `as_const()` paths |
| **P4** | G3 — graph templates / SynthDef | high | med-high | Gateway to the OSC front-end |
| **P5** | G6 — control buses | med | med | **Blocked by P3 (G2)** |
| **P6** | G7 — scheduled commands | med | med | Independent; matters for sequencing |
| **P7** | G5 — wider node inputs | low | med | Stack-budget constrained — see below |
| **P8** | G10 — assorted | low | low each | Opportunistic |
| — | G8 — groups | low | high | Defer; YAGNI for a fixed instrument |
| — | G9 — open UGen set | — | high | Deliberately not doing |

### Tier 0 — ✅ done

**P0 · G4a — documentation fix.** The module docs in `lib.rs`, `arena.rs`, and
`engine.rs` said "topological order," which reads as a promise that the engine
sorts the graph. They now say creation order and state the consequence: a node
reading a source created after it sees that source's *previous* block.

### Tier 1 — high value, contained, do before pinning the API

**P1 · G1 — engine→host event channel. ✅ done.** The kernel state already
existed (`Stage::Idle`, and `PolyAr`/`PolyAdsr` as `[Ar; VOICES]`), so the work
was plumbing it out. Shipped:

1. `Ar::is_idle` / `Adsr::is_idle`; `PolyAr::idle_mask` / `PolyAdsr::idle_mask`.
2. `event.rs` — `Event::{Done, VoiceDone}` plus a fixed `EventQueue` that drops
   and counts on overflow instead of panicking.
3. `Node::idle_mask` / `Node::is_poly_env`; `Engine::collect_events` diffs
   against `prev_idle` at the end of `render_block` and emits rising edges only.
   `seed_prev_idle` on create stops a fresh envelope announcing a release it
   never played. Host drains with `pop_event` / `drain_events`.
4. `VoiceAllocator::on_event` + a per-lane `lane_done` gate bitmask; a lane
   returns to `Free` only once every configured gate has reported.

15 new tests. See the G1 entry above for the two deliberate limits (held lanes
are not reclaimed; overflow degrades to the old stealing behaviour).

**P2 · G4b — node reordering. ← next.** Cheaper than it looks: eval order is already an
explicit `[u16; NODES]` list with a stable-compaction path (`arena.rs:74-78`),
so insertion is a memmove within an array we already maintain. Add
`Cmd::MoveBefore { node, target }` / `MoveAfter`. This is what makes live
patching safe — today, inserting an effect mid-chain means tearing down and
rebuilding everything downstream, and a freed-then-recreated id silently gains a
one-block delay.

**P3 · G2 — control rate.** The cheap path avoids touching any kernel signature:

1. Mark a node as control-rate (a flag on `Node`, or a `Kind` property).
2. The engine renders it with a 1-sample block instead of `BLOCK`.
3. The input-resolve step (`engine.rs:296-315`) hands downstream consumers
   `In::K(v)` instead of `In::A(row)`.
4. Kernels take their **existing** `as_const()` fast path
   (`deluge-dsp-kernels/src/lib.rs:52`) with no changes.

Do this before G6, and ideally before the first CPU-budget measurement on
hardware — a modulation-heavy patch is where the current audio-rate-everything
model costs the most.

### Tier 2 — structural, after the API settles

**P4 · G3 — graph templates.** First version: a recorded `Cmd` sequence plus
`NodeId` relocation and a free-id allocator. Pairs directly with exposing an
scsynth-compatible OSC surface (`/d_recv`, `/s_new`, `/n_set`, `/g_new`) as an
*alternative control front-end* onto our existing graph — which is how we get
SuperCollider-ecosystem interop without adopting scsynth's engine, its GPL, or
its C++ runtime.

**P5 · G6 — control buses.** *Blocked by P3.* A control bus without a control
rate is just an audio bus, so this has no value until G2 lands. Also needs a
mono/control bus type distinct from the current stereo audio pair, since
reading a bus currently sums L+R (`engine.rs:313`).

**P6 · G7 — scheduled commands.** Timestamp on `Cmd`, a small priority queue,
and a sample clock on the engine. Independent of everything else. Priority rises
sharply the moment a real sequencer drives the engine.

### Tier 3 — deferred

**P7 · G5 — wider node inputs.** Not the const bump it appears to be. The
per-call scratch in `render_block` is stack-resident:

```
poly_scratch = 3 × VOICES × BLOCK × 4 B      scratch = MAX_INPUTS × BLOCK × 4 B
```

At `VOICES = 8`, `BLOCK = MAX_BLOCK = 128` that is **12,288 B + 1,536 B ≈ 13.8 KB
of the 32 KB `PROGRAM_STACK_SIZE`** (`memory.x`). Note the `3` in `poly_scratch`
is hardcoded, *not* `MAX_INPUTS` — generalizing it to `MAX_INPUTS = 6` at
`BLOCK = 128` would need ~24.6 KB and blow the stack.

Any widening must be paired with a decision on `BLOCK` and on whether scratch
moves off-stack into the arena. Since `Add` chains are an adequate workaround
today, this stays deferred — but revisit it at the same time as pinning `BLOCK`
for the first production instantiation.

**P8 · G10 — assorted.** NRT render, recording, `/b_gen`-style buffer
generators, the `MAX_BLOCK` cap, non-silent capacity exhaustion. Pick these up
opportunistically when adjacent work is already open.

### Not planned

**G8 — groups.** High effort, and a hierarchical node tree buys little for a
fixed-topology embedded instrument. Revisit only if arbitrary user patching
becomes a product goal.

**G9 — open UGen set.** Deliberately declined. Static dispatch over the closed
80-variant `Kind` enum is a significant part of why the engine is fast on an A9,
and we ship a closed product. The `Custom(dyn Ugen)` hatch (`node.rs:4`) stays
reserved, not built.
