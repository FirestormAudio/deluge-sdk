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

### G2 — No control rate — ✅ **DONE** (mono + poly; stereo refused)

`In` was only `K(f32)` or `A(&[f32])`, and `K` only ever originated from a
literal `Input::Const` — no *node* could produce one value per block, so an LFO
driving a filter cutoff ran at full audio rate.

**Resolved for width-1 nodes.** `Rate::{Audio, Control}` on `Node`, set via
`Engine::set_rate` / `Cmd::SetRate`. A control-rate node is evaluated once per
block into a 1-sample `OutView`, then its value is broadcast across the row;
consumers receive it as `In::K` and take their existing `as_const` fast path.

Two semantics that are easy to get wrong and are pinned by tests:

- **`BLOCK * dt`, not `dt`.** A node evaluated once per block covers `BLOCK`
  samples of wall time. Passing `self.dt` would make every time-based kernel
  (LFO phase, envelope stages, slew) run `BLOCK`× too slow — a silent, purely
  musical bug. `control_rate_advances_at_the_same_wall_clock_rate` and
  `control_rate_without_dt_scaling_would_be_visibly_slower` guard it.
- **Inputs are sampled at the block's first sample** (scsynth's `A2K`), so
  feeding an audio-rate signal into a control-rate node is a sample-and-hold,
  not an average.

The output row is still filled, because readers other than kernels — bus
writes, `node_output`, `fill_usb`, the poly-lane splat — index it directly.
Filling `BLOCK` floats is far cheaper than `BLOCK` kernel evaluations.

**Poly, since resolved.** A poly node's output is a **sample-major** tile —
lane `v` of sample `i` at `tile[i * VOICES + v]`, per `poly::voice_sum` and
every `Poly*::process`. Its `VOICES` arena rows are storage, *not* lanes, which
is worth stating because the `[[f32; BLOCK]; VOICES]` shape in the engine
actively suggests otherwise; `Engine::poly_tile_sample` exists so tests address
it correctly. Control rate asks the kernel for one sample — the tile's first
`VOICES` floats — and repeats them across the block.

Widths are handled apart: the input tile is always `VOICES` wide, while the
*output* is `width` wide, which for a collapse node like `VoiceSum` is 1. Both
are sample-major, so one flat `width`-sized repeat serves both.

**Still open: stereo.** A width-2 kind (`Pan`, `StereoVoiceSum`, the reverbs)
stores its two ports as independent row-major rows rather than one interleaved
tile, so the same repeat does not apply. `set_rate` refuses it rather than
guessing, and no stereo kind is a modulation source.

**Adjacent win not taken:** `Input::Const` still resolves to a filled row and
`In::A`. Handing those down as `In::K` would put every existing patch on the
kernels' const fast path too — a real speedup, but it changes the code path
every current patch takes and deserves its own change with the golden
characterisation test watched closely.

### G3 — No graph template / SynthDef — ❌ **RETIRED**

Every node is created individually by explicit `NodeId` via `Cmd::NewNode`, so
there is no way to define a patch once and instantiate it N times; scsynth's
`/d_recv` + `/s_new` split is what makes "load a preset" cheap there.

**Retired, not built.** "Instantiate a template N times" was the scsynth
framing. The glicol note argues a better one for this product — *edit a running
patch without a click* — and that shipped instead as GL2 (`cf180e9`): an epoch
mark-and-sweep where re-running the patch script **is** the diff, because node
ids are author-assigned and deterministic. Paired with GL6's named identity
scopes (`b7ed24f`), an edit mid-script no longer renumbers everything below it.

That covers the motivating use (presets, patch reload) without a template
format, a relocation pass, or a free-id allocator. If instantiate-N is ever
genuinely wanted, it falls out of the same id machinery. See
`docs/audio-graph-vs-glicol.md` §GL2.

### G4 — Eval order is creation order — ✅ **DONE**

**Nothing sorts.** `Arena::create` appends to the order list and `free` does a
stable compaction. Correct signal flow is entirely the author's obligation — to
create nodes in dependency order.

**G4a resolved.** The doc comments in `lib.rs`, `arena.rs`, and `engine.rs` used
to claim "topological" order, which read as a promise the engine sorts for you.
They now say creation order and spell out the consequence.

**G4b resolved.** `Arena::move_before` / `move_after` (scsynth `/n_before`,
`/n_after`), surfaced as `Cmd::MoveBefore` / `Cmd::MoveAfter` and as direct
`Engine` methods returning `bool`; `Engine::eval_order()` exposes the current
order for inspection. Reordering is a remove-then-insert on the existing
`order` list — every other node keeps its relative position.

The move is purely local to eval order: output slots, bus writes, stream
cursors and envelope history are all keyed by `NodeId`, so nothing else has to
move with it (pinned by `reordering_does_not_disturb_output_slots` and
`reordering_preserves_bus_routing_and_output`). Inserting into an existing
chain is now `NewNode` (lands at the end) then `MoveAfter` onto its upstream —
no teardown of anything downstream.

**Superseded by G11.** This section originally ended "still the author's
obligation: nothing sorts." That is no longer true. `Arena::sort` (G11,
`5bb968e`) runs Kahn's algorithm on mutation, so eval order *is* topological
and building a patch in dependency order is no longer the author's job —
G11 was the better answer to G4 and the glicol note explains why.

`MoveBefore` / `MoveAfter` survive as the deliberate override: the sort is
stable, so a move that does not contradict a dependency is preserved, and the
members of a feedback cycle keep their author-chosen order because that order
decides where the loop's one-block delay falls.

### G5 — Hard three-input / three-arg ceiling

`MAX_INPUTS = 3` (`node.rs:48`) and `MAX_ARGS = 3` (`cmd.rs:8`). This rules out,
as single nodes: multi-input mixers, `Select` / `SelectX`, multi-tap structures,
and breakpoint envelopes. The workaround is chains of `Add`, which consumes
eval-order entries and arena output rows.

### G6 — No control buses, no `/n_map` — ✅ **DONE**

Buses were stereo **audio** pairs; reading one as an input sums L+R, which is
meaningless for a control signal. There was no shared modulation signal many
nodes could subscribe to — fanning one modulator to twenty destinations cost
twenty explicit edges.

**Resolved.** `ctrl.rs` adds `CTRL_BUSES` mono control buses, deliberately the
opposite of an audio bus in every respect:

|          | audio bus                    | control bus                |
|----------|------------------------------|----------------------------|
| shape    | `[[f32; BLOCK]; BUSES]`      | one `f32`                  |
| lifetime | zeroed and re-summed / block | **persists until written** |
| read as  | `In::A`, L+R summed          | `In::K` (const fast path)   |

**Persistence is the point.** A host writes a MIDI CC once (`Cmd::SetCtrl`,
scsynth `/c_set`) and it stays until something writes it again; summing-and-
zeroing like an audio bus would wipe that on the next block. It also means a
control bus reads `0.0` before anything writes it rather than being undefined.

**One writer wins, rather than summing.** Two nodes routed to one control bus
is a patching mistake, not a mix; modulators combine through `Add`, where the
intent is explicit. `Cmd::CtrlWrite` records a standing route (scsynth
`Out.kr`), `Cmd::ClearCtrlWrite` removes it, and the bus then holds its last
value rather than snapping to zero.

**`/n_map` is the half that matters most here.** `Cmd::MapParam` re-applies a
parameter from its bus at the top of every block, so the whole `u8` parameter
space becomes modulatable *without spending one of the three scarce input
ports* — see §G5. Filter drive, oscillator feedback, reverb size and ADSR
sustain are parameters, not inputs, and until now nothing could modulate them
at all. While mapped, the bus is the authority and a direct `SetParam` is
overwritten on the next block, as in scsynth. `MAX_PARAM_MAPS = 32`, and a full
table reports `CmdError::MapTableFull`.

**Ordering within a block:** maps are applied at the *top* of `render_block`
(so a node renders with this block's mapped values), and standing routes are
collected at the *end* (so a bus carries the value its source just produced,
read by consumers next block). A control-bus read is therefore one block old —
the same rule `Input::Bus` already follows, which is why control-bus edges are
**not** dependencies for the G11 topological sort. One block is 1.3 ms at
48 kHz; inaudible for modulation, which is all this carries.

`CTRL_BUSES` is a plain const rather than a seventh `Engine` const-generic
parameter: at 4 bytes a bus the whole space is 128 bytes, which is not worth
another type parameter on an already six-wide signature.

### G7 — No scheduled or timestamped commands — ✅ **DONE**

`Engine::apply` lands a `Cmd` on whatever block it is called in, tying every
edit to whenever the control thread happened to run.

**Resolved.** `Engine::apply_at(at, cmd)` files a command against a sample
position on the engine's clock; `sched.rs` holds a fixed `SCHED_QUEUE` of
`(timestamp, Cmd)` which `render_block` drains. `Cmd::ClearSchedule` is
scsynth's `/clearSched`; `Cmd::Reset` clears it too, since the clock restarts
at zero and pending timestamps would fire a dead patch's commands into the new
one.

- **Block accurate**, like scsynth — the command fires at the start of the
  block *containing* its sample, so resolution is `BLOCK` (1.3 ms at 48 kHz,
  `BLOCK = 64`). Sample accuracy would mean splitting a block's render around
  the command; scsynth draws the line in the same place.
- **Overdue fires, never vanishes.** The drain predicate is `at <
  sample_clock` *after* the clock's `+= BLOCK`, which reads as "this block
  contains `at`" and sweeps up anything already past. Late is recoverable.
- **Drained before the G11 re-sort**, so a scheduled `NewNode` joins eval order
  in the block it lands in rather than a block late.
- **FIFO within a timestamp**, so a scheduled `NewNode` and the `SetInput`
  wiring it up cannot invert. Insertion places an entry after every equal-or-
  earlier one, which gets that with no sequence counter to overflow.
- **A full queue refuses and reports** `CmdError::ScheduleFull` rather than
  evicting something already accepted.

**Why it is not a `Cmd` variant.** `Cmd::ScheduleAt { at, cmd: Cmd }` cannot
exist — a `Cmd` nesting a `Cmd` is infinitely sized. Boxing would make `Cmd`
non-`Copy` (it is passed by value at ~14 sites) *and* put a `dealloc` in the
render path, since `deluge-alloc` runs every alloc and free inside
`critical_section::with` with interrupts off. So scheduling is expressed as a
property of delivery: `Host::audio_cmd_at` alongside `audio_cmd`.

**Not built:** an unbounded queue. `SCHED_QUEUE = 64` is a ceiling a dense
pattern could hit. The clean fix is a `Vec`-backed queue on the *control* side,
allocating off the audio thread and feeding this one just in time — host-side,
no engine change.

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
| ~~P2~~ | ~~G4b — node reordering commands~~ | high | low | ✅ **done** — `Move{Before,After}` |
| ~~P3~~ | ~~G2 — control rate~~ | high | med | ✅ **done** (mono; poly still open) |
| ~~P4~~ | ~~G3 — graph templates / SynthDef~~ | — | — | ❌ **retired** — superseded by GL2 (see glicol note) |
| ~~P6~~ | ~~G7 — scheduled commands~~ | med | med | ✅ **done** — `sched.rs`, `Engine::apply_at` |
| ~~P5~~ | ~~G6 — control buses + `/n_map`~~ | med | med | ✅ **done** — `ctrl.rs`; `/n_map` relieves §G5 |
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

**P2 · G4b — node reordering. ✅ done.** As predicted, cheap: eval order was
already an explicit `[u16; NODES]` list with a stable-compaction path, so the
move is a remove-then-insert on an array we already maintain. Shipped
`Arena::move_before` / `move_after`, `Cmd::MoveBefore` / `MoveAfter`, matching
`Engine` methods returning `bool`, and `Engine::eval_order()`.

Invalid moves (dead node, dead target, self-move) return `false` and change
nothing, in keeping with the crate's no-panic discipline. 12 new tests,
including the behavioural one that builds a chain in the wrong creation order,
shows the read is a block stale, and repairs it with a single `MoveAfter`.

**P3 · G2 — control rate. ✅ done (mono).** The cheap path held: **no kernel
signature changed**. `Rate` on `Node`, a 1-sample `OutView` plus a broadcast
fill, `In::K` to consumers, and the kernels' existing `as_const` fast path does
the rest.

Two things the four-bullet plan above missed, both found while implementing:
the `dt` must be scaled to `BLOCK * dt` or every time-based kernel runs `BLOCK`×
too slow, and poly kinds need their own design (voice-interleaved tiles), so
this pass is width-1 only. Both are written up under G2. 9 new tests.

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
moves off-stack into the arena. **G6's `/n_map` relieves much of the pressure**:
the parameter space is a whole `u8` and is now modulatable, so a destination
that is a *parameter* no longer competes for one of the three input ports.
Since `Add` chains cover the rest, this stays deferred — but revisit it at the same time as pinning `BLOCK`
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
