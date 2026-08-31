# `deluge-audio-graph` vs. glicol_synth — capability gap analysis

**Status:** reference note. Written as an evaluation; the items it raised have
since been built (see the priority table at the end). Companion to
[`audio-graph-vs-scsynth.md`](audio-graph-vs-scsynth.md). Written 2026-08-31
against `crates/deluge-audio-graph` at commit `79201c3` and
[glicol] `0317db2` (`rs/synth`, workspace version `0.14.0-dev`, last commit
2025-01-23).

**Why this exists:** the scsynth comparison measured us against a 25-year-old
C++ server. glicol is the opposite control: a *contemporary Rust* graph engine
with const-generic block size, embedded/wasm ambitions, and a live-coding
language on top — i.e. the closest thing to a peer this crate has. If our design
is wrong in ways the scsynth comparison couldn't see, it should show up here.

It mostly doesn't. But glicol has three things we don't, and one of them
(sink-driven evaluation) is a better answer to G4 than the one we shipped.

Gap IDs below are `GL*`; where a gap is the same one the scsynth note already
raised, the `G*` id is given so the two documents stay reconcilable.

[glicol]: https://github.com/chaosprint/glicol/tree/main/rs/synth

---

## The architectural difference in one sentence

glicol_synth is a **`std`, heap-allocating, dynamically-dispatched** graph
(`petgraph::StableGraph` of `Box<dyn Node>`, `graph.rs:49-53`,
`context.rs:88-90`) whose great strength is that the graph is **derived from
source text and re-derived on every edit**; `deluge-audio-graph` is a
`no_std`, no-alloc, statically-dispatched fixed-capacity renderer whose great
strength is that it can state its worst case. glicol optimises for *the patch
changing*; we optimise for *the patch running*.

| | glicol_synth | deluge-audio-graph |
|---|---|---|
| Node set | open — `Box<dyn Node<N>>`, user-implementable | closed — 80-variant `Kind` enum (`node.rs:51`) |
| Dispatch | vtable per node per block | static match on `Kind` |
| Input delivery | `HashMap<usize, Input<N>>` rebuilt per node per block (`graph.rs:146-157`) | copy into stack scratch by port index (`engine.rs:370-384`) |
| Eval order | DFS post-order from the sink, recomputed every block (`graph.rs:143-145`, in `process`, `graph.rs:133`) | creation order + explicit `Move{Before,After}` (`arena.rs:47`) |
| Buffers | `Vec<Buffer<N>>` per node, heap | flat output arena, `NODES × OUTS` rows |
| Capacity | `max_nodes: 1024` default, grows (`context.rs:28`) | const generic, exhaustion returns `false` |
| Alloc in `process` | yes (see GL-vs below) | none |
| Panics in `process` | yes (`graph.rs:154`, `163`) | none by construction |
| Polyphony | none — ad-hoc `Vec` of active playbacks inside two nodes | `VOICES`-wide signals + `VoiceAllocator` (`voice.rs:56`) |
| SIMD | none | NEON via `armv7-dsp-intrinsics` |
| Musical time | engine-level BPM + bar/sec/ms param units | none |
| Target | desktop / wasm AudioWorklet | bare-metal Cortex-A9 |

---

## What glicol has that we don't

### GL1 — Sink-driven topological evaluation, with dead-branch culling — ✅ **DONE**

`process` resets a `DfsPostOrder` over the **reversed** graph from the
destination node and walks it (`graph.rs:143-145`, in `process`, `graph.rs:133`). Two consequences we don't
get:

1. **Order is always correct.** You cannot build a stale-by-one-block read,
   because dependencies are visited before dependents by construction. Our G4b
   work gave us the *tools* to fix a bad order (`Cmd::MoveBefore` /
   `MoveAfter`); glicol never has a bad order to fix.
2. **Nodes that don't reach the output cost nothing.** A disconnected chain is
   simply not visited. Our `render_block` walks every live node in
   `eval_order()` (`engine.rs:355-357`) whether or not it feeds the root bus.

The price glicol pays is real: a visit-map reset and a full traversal every
block, plus a `HashMap` clear-and-refill per node (`graph.rs:146-157`). That
cost is why we shouldn't copy the mechanism — but the *guarantee* is
reachable without it. **Sort on mutation, not per block:** topologically sort
the arena's `order` array when an edge changes (`SetInput`, `NewNode`, `Free`),
cache the result, and keep the render loop exactly as it is. Same guarantee,
zero per-block cost, and `Move{Before,After}` becomes an override for the
deliberate feedback case rather than the primary ordering mechanism.

Reachability culling is the same pass: mark from the root bus backwards while
sorting, skip unmarked nodes in the render loop.

**Resolved** (`arena.rs`, `Arena::sort`). Kahn's algorithm in place, no
allocation, run from `render_block` when the graph is dirty — so a whole patch
build costs one sort, not one per command. Stable: it always takes the earliest
ready node in the current order, so parallel chains keep their authored order
and a `move_before`/`move_after` that respects dependencies survives. Cycles
keep their existing relative order (their order is what decides where the loop's
one-block delay falls); self-edges and `Input::Bus` reads are not treated as
dependencies, both being one block delayed by design.

`Move{Before,After}` is now an override rather than the primary mechanism: it
applies immediately and holds until the graph's shape changes again. The old
`move_after_fixes_a_stale_by_one_block_read` test could no longer pass — the
engine repairs that order itself — and was replaced by one pinning the full new
contract.

Culling shipped as **opt-in** (`Engine::set_cull_unreachable`), default off, and
should stay that way: the engine cannot see what the host reads (`node_output`,
`fill_usb`, a prefetch cursor), so culling by default would silently freeze a
node someone is legitimately reading. glicol can only get away with it because
its output is *defined* as one destination node.

### GL2 — Incremental graph update from a source description — ✅ **DONE**

`update_with_code` (`rs/main/src/lib.rs:172`) parses new source, **diffs the new
AST against the old**, and applies only the delta: nodes whose `Component` is
unchanged keep their identity *and their DSP state*; removed nodes are freed;
inserted nodes are added and re-referenced. A running patch survives an edit to
one node in the middle of a chain without a click.

This is a superset of the G3 "SynthDef / graph template" gap. Templates let you
instantiate a patch N times; this lets you **edit a live patch and keep the
sound going**. For an instrument with a screen-editable engine — which is what a
Deluge is — the second is worth more than the first.

**The expensive half is already done here.** glicol's hard problem is
*recovering* node identity from source text — that is what the AST
structural-equality diff and the `index_info` name table exist for. We don't
have that problem: `NodeId`s are author-assigned, and the Wren layer hands them
out from a deterministic bump allocator (`deluge-wren-core/src/audio.rs:35-47`),
so re-running the same script gives the same logical node the same id. Identity
is *given*, not reconstructed. We need no parser and no AST: **the script re-run
is the diff.**

What the engine needs is small:

- An epoch byte per arena slot, plus `Cmd::BeginUpdate` / `Cmd::EndUpdate`.
- `Arena::create` currently does `return false; // refuse to double-allocate a
  live id` (`arena.rs:51`). Under an update that becomes: same kind → stamp the
  epoch, apply args, **keep DSP state**; different kind → free and recreate.
- `SetInput` / `BusWrite` re-emission is already idempotent (both are stores).
- `EndUpdate` sweeps: free every live node whose epoch is stale.

**The catch that decides how good it is.** Bump-allocated ids are stable only
across an unchanged *prefix*. Insert a node mid-script and every later id shifts
by one — and then id 7 is still a `Lpf` by coincidence but a *different logical*
`Lpf`, so an `(id, kind)` match silently preserves state onto the wrong node.
Benign for a filter's delay line; wrong for a `StreamPlayer` inheriting another
voice's cursor. So `(id, kind)` alone is not a sound key, and mid-script edits
need the stable names of GL6 — which cost the engine nothing (see there).

**This is not speculative.** `tools/wren-web/app/src/sim.ts:103,116` — the web
editor's Run calls `sim_reset()` and rebuilds the project from scratch, so every
⌘↵ tears down the graph and drops audio. M3/M4 of the web-editor plan are done
and shipping. That is glicol's exact workflow, live in this repo, with the
click.

**Resolved.** `Cmd::BeginUpdate` advances an epoch that each `NewNode` stamps;
`Cmd::EndUpdate` frees every live node still carrying an older one. Inside an
update, `NewNode` on a live id with the same kind keeps the node and its DSP
state; a different kind replaces it. Host-side, `deluge-wren-core` gained
`begin_update()` / `end_update()` — `reset()` minus the reset — and
`re_running_a_script_under_an_update_emits_the_identical_command_stream` pins
the deterministic-id claim: the second run's command stream is byte-identical
to the first.

Three things that came out of building it, none of them predicted by the glicol
reading:

- **The sweep needs no retire list.** A node the new patch omits has no path to
  an output any more — omission is what severed it — so there is no audible tail
  to protect. It reuses the existing `Cmd::Free` path verbatim, so a swept node
  is never cleaned up less thoroughly than an explicitly freed one.
- **The real hazard was bookkeeping, not audio.** Sweeping an envelope node
  leaves `VoiceAllocator` lanes waiting on a `VoiceDone` that can never
  arrive — G1's bug returning through the update path. `Event::Freed` reports
  each swept node and the allocator treats a freed gate as permanently reported,
  for waiting lanes and later notes alike.
- **Two idempotency bugs that only an update path exposes.** `bus_write_gains`
  appended unconditionally, so a re-emitted `BusWrite` stacked a second entry
  and added 6 dB per edit; `Cmd::BindTable` overwrote a binding without
  releasing the pooled region it held, leaking a wavetable pyramid per edit.
  Both are now idempotent. The pooled fix lives at the rebind seam rather than
  in a host-side handle cache, which cannot be made safe without refcounting the
  pool.

Still open: mid-script *insertions* renumber every later id, so `(id, kind)`
matching reattaches state to the wrong node. That is what GL6 names are for.

### GL3 — Musical time is an engine-level concept

BPM is engine state broadcast to every node (`Message::SetBPM`, `lib.rs:76`, broadcast via
`context.rs:256-260`), and parameters carry musical units —
`GlicolPara::{Bar, Second, Millisecond}` (`lib.rs:102-104`). Sequencing nodes
derive their own clock from it: `Sequencer` computes
`bar_length = 240/bpm × sr / speed` per block (`sequencer/seq.rs:45`), and
`PSampler` / `PatternSynth` schedule pattern events against a cycle duration.
There are dedicated `seq`, `speed`, `choose` and `arrange` nodes plus a pattern
syntax in the language.

We have exactly one clocked node — `Steps`, which reads its clock on port 0
(`node.rs:1043`) — and no notion of tempo, bars, or a transport anywhere in the
crate. `Engine` knows only `dt = 1/sample_rate` (`engine.rs:38, 92`).

**This is the one gap the scsynth comparison structurally could not surface**,
because scsynth doesn't have it either — SuperCollider's tempo lives in sclang,
off-server, exactly as ours would live in firmware. See the assessment for why
I still think most of it stays out of the engine, and which piece shouldn't.

### GL4 — Unbounded, explicitly-ordered inputs — *confirms G5*

A node receives `HashMap<usize, Input<N>>` (`node/mod.rs:50`) — any number of
inputs, each itself a multi-buffer node output. Order is established
independently of connection order via `Message::IndexOrder(pos, index)`
(`context.rs:186-192`), which is how sidechain and `~ref` inputs stay
distinguishable from the main signal.

Our `MAX_INPUTS = 3` (`node.rs:48`) and `MAX_ARGS = 3` (`cmd.rs:8`) are the G5
gap; glicol is the second independent data point that 3 is low. The stack-budget
analysis in the scsynth note still applies and still says "not yet" — but note
that glicol's answer (inputs are *pointers* to the upstream node's own buffers,
`node/mod.rs:62-88`, not copies into scratch) sidesteps the budget entirely. Our
copy-into-scratch step exists so the poly kernels get contiguous lanes; it is
worth checking whether the non-poly path could borrow rows directly and drop
`MAX_INPUTS × BLOCK × 4 B` of stack in the process.

### GL5 — Runtime-scripted DSP nodes — *a middle path past G9*

Two escape hatches: `Meta` runs a rhai script per block with the buffer exposed
as a scope variable (`dynamic/meta.rs`), and `Eval`/`Expr` compile a small
arithmetic expression to a tree evaluated per sample (`dynamic/eval.rs`,
`dynamic/expr.rs:64-69`). Both are feature-gated (`node-dynamic`).

We declined G9 (open UGen set) deliberately and I still think that's right —
but "closed set" and "no user DSP at all" aren't the same choice. A
`Kind::Expr` holding a **pre-compiled, fixed-size postfix program** over a small
register file would keep static dispatch, keep no-alloc, and give Wren authors a
way to write a shaper or a modulation curve we didn't anticipate. glicol's
per-sample interpreter (`.unwrap()` on every sample, `expr.rs:68`) is not the
version to copy; the idea underneath it is worth revisiting.

### GL6 — Names, and a symbol table — ✅ **DONE** (identity half)

`AudioContext.tags: HashMap<&'static str, NodeIndex>` (`context.rs:96`), plus
`index_info: HashMap<String, Vec<NodeIndex>>` in the main crate mapping chain
names to node runs. Chains are named in the source (`~lfo: sin 0.2`) and
referenced by name from other chains.

We have numeric `NodeId` and nothing else. Fine for a compiler; painful for a
control plane that wants to say "replace the filter in the bass voice."

Two things follow. First, it belongs on the **Wren side, not in the engine**: a
name→id map inside `Alloc` (`deluge-wren-core/src/audio.rs`), with the engine
still seeing only `NodeId`s. It is not an engine feature at all, and it is
cheap. Second, it stops being cosmetic the moment GL2 lands — stable keys are
what make an insertion in the middle of a script survive an incremental update
instead of shifting every subsequent id and reattaching state to the wrong node.

**Resolved, for the identity half.** `Patch.named(name, fn)` opens a named
scope; allocations inside are keyed by `(scope hash, ordinal within the scope)`
instead of by position in the whole script, and `Alloc` sweeps untouched keys at
`end_update` exactly as the engine sweeps stale nodes. A named block keeps its
ids wherever it moves and whatever grows above it.

The obstacle worth recording, since it rules out the obvious API: a fluent
`Osc.saw(110).id("bass")` cannot work. Constructors allocate the id *and* emit
`Cmd::NewNode` before the name is known, so by the time `.id` runs the id is
already spent — reusing the mapped id would mean moving a live node between
slots and rewriting every edge that references it. A scope supplies the name
*before* the allocation, which is why the primitive is a block. The block is
called from Wren, not from Rust, so nothing re-enters the VM from a foreign
method.

Still not built: the **addressing** half — a host saying "replace the filter in
the bass voice" by name. Names exist only inside the binding's allocator; the
engine still sees numeric `NodeId`s, which was always the right split.

### GL7 — Typed errors from the control surface — ✅ **DONE**

`update_with_code` returns `Result<(), EngineError>` with parse spans, and
`clean_up` rolls back the partially-built graph on failure (`main/src/lib.rs:557`).
Our `Cmd` surface is infallible-by-silence: capacity exhaustion returns `false`
from `Arena::create`, a dangling `Input::Node` renders silence. That's the right
call for the audio thread, but there's no channel by which the *host* learns
that the patch it just built is missing three nodes.

**Resolved.** `Event::CmdFailed { node, reason }` with
`CmdError::{CreateFailed, DeadNode, UnsupportedRate}`, emitted by the commands
that shape the graph. Performance-rate commands (`Gate`, `Trigger`,
`GateVoice`, `TriggerVoice`, `StreamFill`) stay silent deliberately: they arrive
at note rate, and a host bug there would flood the fixed queue and evict the
envelope-completion events the voice allocator depends on.

### GL8 — Assorted

- Sample rate is a runtime broadcast (`Message::SetSampleRate`); ours is fixed
  at `Engine::new` (`engine.rs:92`).
- The engine now has a free-running sample clock (`Engine::sample_time`) — the
  prerequisite for G7 that GL3 exposed. Still not a transport: tempo stays in
  firmware, and a host converts bars or milliseconds on its own side.
- `send_msg_to_all` (`context.rs:256`) — no broadcast `Cmd` on our side.
- Arbitrary per-node channel counts (`multi_chan_node`, `graph.rs:101`); we are
  mono / stereo / `VOICES`-wide and nothing else. Only relevant if we ever want
  a genuinely N-channel node, which for this product we don't.

---

## What we have that glicol doesn't

Everything in the scsynth note's "what we have" section still applies (poly
width, native stereo, voice allocation, curated filter set, mip-mapped
wavetables, the master seam, SD streaming). Against glicol specifically, add:

### 1. Real-time safety, in the sense the term is usually meant

glicol allocates and panics inside the audio callback:

- `sampler.rs:43` — `let mut to_remove = vec![];` **inside the per-sample loop**:
  one heap allocation per sample, per sampler node.
- `psampler.rs:67` — `sample_name.to_owned()`, a `String` allocation on every
  triggered event, in `process`.
- `psampler.rs:63,75` — `self.samples_dict[name]`, an indexing panic if the
  sample is missing.
- `graph.rs:154,163` — `.expect(NO_NODE)` twice in the traversal.
- `expr.rs:64-69` — three `.unwrap()`s per sample.
- Node buffers are `Vec<Buffer<N>>` and the graph is a growable `StableGraph`;
  adding a node during playback allocates.

Ours: pool exhaustion returns `None` (`pool.rs`), arena exhaustion returns
`false` (`arena.rs`), a dangling input renders silence, total RAM is a
compile-time constant. On a 400 MHz A9 with a hard block deadline and no MMU
safety net, this is not a stylistic difference.

### 2. Bounded polyphony

glicol has no voice allocator. Polyphony exists only inside `Sampler` and
`PSampler` as an **unbounded `Vec` of active playbacks** (`sampler.rs:40`) —
trigger density directly drives CPU with no ceiling and no stealing. Our
`VoiceAllocator` (`voice.rs:56`) has fixed `VOICES` lanes, oldest-LRU stealing,
release-tail awareness, unison with detune and stereo spread, and — since G1 —
event-driven lane reclamation.

### 3. Band-limited oscillators

glicol's `saw` is a raw phase ramp (`oscillator/saw_osc.rs:47-53`) and `squ` is
a raw comparator (`squ_osc.rs:47-56`) — no PolyBLEP, no mip pyramid. They alias
audibly above a few hundred Hz. We ship `MipSet`/`LEVELS` band-limited
wavetables and anti-aliased analogue models.

### 4. Engine→host events

Neither engine had these; we now do (`event.rs:34,44`). glicol's nodes cannot
tell the host anything at all — `Node::send_msg` is one-way by trait signature
(`node/mod.rs:51`).

### 5. DSP breadth

80 `Kind` variants against roughly 45 glicol node types (grammar rule
`node`, `parser/src/glicol.pest:6`). glicol's filter set is `lpf` / `rhpf` /
`onepole` / `apfmsgain`; no ladder, no MS-20, no SVF family, no modal
resonator, no FDN. Its compound nodes (`bd`, `sn`, `hh`, `*synth`) are
convenience macros over the primitives, not new DSP.

---

## Assessment

**No change to the core design thesis.** glicol makes the opposite trade at
every axis where the trades are exclusive — open set vs. static dispatch, heap
vs. fixed capacity, per-block traversal vs. cached order — and each of those
trades is right for a browser live-coding environment and wrong for a
battery-powered instrument with a hard deadline. Nothing here argues we should
have adopted it.

**Three findings that do change the plan:**

**GL1 is a better G4 than ours.** We closed G4b by shipping the tools to repair
eval order. glicol demonstrates that never having a wrong order is achievable,
and the "sort on mutation" variant gets the guarantee at a per-block cost of
zero — cheaper than what we have now, since a cached sorted order plus
reachability marking also lets the render loop skip unreachable nodes. This is a
new item, **G11**, and it is small: a Kahn sort over the existing `order: [u16;
NODES]` array (`arena.rs:17`) run from `apply` when edges change. `MoveBefore` /
`MoveAfter` survive as the explicit override for intentional one-block feedback.

**GL2 replaces G3 and moves up.** "Templates" was the scsynth framing —
instantiate a patch N times. glicol's framing is better for our product: *edit a
running patch without a click*. And it is much cheaper here than the scsynth
note assumed, because the half glicol works hardest for — recovering node
identity — is already solved by author-assigned `NodeId`s and a deterministic
Wren-side allocator. The engine cost is an epoch byte per arena slot, two
`Cmd`s, and turning `Arena::create`'s refusal to touch a live id into a
state-preserving update; the script re-run supplies the diff. See GL2 above for
the id-shift hazard and why GL6 names (host-side, no engine change) are what
make mid-script edits sound.

**Do it in the same change as G11, not after.** The sweep leaves eval order
needing a re-sort anyway, both touch the same `order: [u16; NODES]` array, and
both change the `Cmd` surface — which is exactly the thing that gets expensive
once a production `Engine<…>` instantiation exists. Splitting them pays that
cost twice.

**GL3 mostly stays out of the engine — with one exception.** Tempo, bars and
patterns belong in firmware on this product, exactly as they belong in sclang on
SuperCollider; duplicating the Deluge's transport inside the DSP graph would
create two clocks that must agree, which is worse than one clock upstream.
`Steps` reading a clock on port 0 is the right shape. **But** the engine still
has no sample counter, and G7 (scheduled commands) cannot be built without one.
That's a one-`u64`-field change, it's a prerequisite for G7 rather than a
feature of its own, and once it exists, bar/ms→samples conversion at the `Cmd`
boundary is a host-side convenience rather than an engine concern.

**GL5 is worth a second look at G9's boundary.** Not an open UGen set — a single
`Kind::Expr` running a pre-compiled fixed-size program. Cheap to bound, keeps
every existing property. Filed, not scheduled.

**GL7 is nearly free now.** `event.rs` already exists; adding a failure event so
the host learns its patch didn't fully build costs one variant.

---

## Priority delta

Only the changes relative to the scsynth note's table. Everything not listed is
unchanged.

The four `Cmd`-surface items below were landed as **one branch**, before the
first production `Engine<…>` instantiation freezes the API.

| # | Item | Value | Effort | Status |
|---|------|-------|--------|--------|
| ~~P2.5a~~ | ~~sample counter (`u64` on `Engine`)~~ | — | trivial | ✅ **done** — `Engine::sample_time`; unblocks G7 |
| ~~P2.5b~~ | ~~GL7 — `Event::CmdFailed`~~ | low-med | trivial | ✅ **done** — three `CmdError` reasons |
| ~~P2.5c~~ | ~~G11 — topological sort on mutation~~ | high | low | ✅ **done** — `Arena::sort`; culling opt-in |
| ~~P2.5d~~ | ~~GL2 — epoch mark-and-sweep patch update~~ | **high** | low-med | ✅ **done** — engine + `deluge-wren-core` plumbing |
| **P3** | G2 — control rate | high | med | ✅ **done for width-1 nodes** (`c2ef2c6`); poly/stereo still open |
| ~~P4~~ | ~~GL6 — Wren-side name→id map~~ | med | low | ✅ **done** — `Patch.named`; addressing-by-name still unbuilt |
| P5 | Switch the editor's Run to `begin_update` | med | low | product-facing; `sim.ts` still calls `sim_reset()` |
| ~~P6~~ | ~~G7 — scheduled commands~~ | med | med | ✅ **done** — `sched.rs`, `Engine::apply_at` + `Host::audio_cmd_at` |
| — | GL5 — bounded `Kind::Expr` | low | med | filed, not scheduled |
| — | GL3 — tempo/patterns in-engine | — | high | **declined** — transport stays in firmware |
| — | GL4/GL8 — wider inputs, broadcast, runtime SR, N-channel | low | — | unchanged, host-side, or YAGNI |

Landed as `09af3a0` (sample clock + `CmdFailed`), `5bb968e` (G11), `cf180e9`
(GL2), `cfc2f9e` (pooled-table rebind), `9dc01e9` (Wren `begin_update`).

G3 as originally specced — instantiate-N templates — is retired in favour of
P2.5d; if template instantiation is ever wanted, it falls out of the same
id-relocation machinery.
