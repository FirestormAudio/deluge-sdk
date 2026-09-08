# Flare Phase 2 — The Move — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move five crates out of `deluge-sdk` into a standalone `~/GitHub/flare`
workspace with history preserved, rename them, add a facade, and rewire
`deluge-sdk` to consume them — changing no behaviour whatsoever.

**Architecture:** A pure rename. `git filter-repo` splits the five crate
directories into a new local repo; the crates are renamed; `deluge-sdk` consumes
flare by path dependency. Because nothing about the audio should change, **any
golden digest change is a bug** — which is what makes a diff this large
reviewable.

**Tech Stack:** Rust nightly, `git filter-repo`, two Cargo workspaces, `cargo
test` across the QEMU-ARM and host buckets.

**Spec:** `docs/superpowers/specs/2026-09-08-flare-audio-engine-extraction-design.md`
(this plan implements Phase 2, spec §4)

**Depends on:** `docs/superpowers/plans/2026-09-08-flare-phase1-characterisation.md`,
complete and merged. Phase 1's goldens are this plan's entire acceptance test.

## Global Constraints

- **Licensing:** MIT / Apache-2.0 only. No GPL. (Spec §Background.)
- **This phase changes no behaviour.** Every golden digest must be
  **bit-identical** before and after. A changed digest is a bug in the move, not
  an output to re-pin.
- **flare never names a `deluge-*` crate.** One direction only. If something
  forces a back-dependency, stop — that is a design finding, not a detail.
- **`no_std`, no heap, no panics in DSP paths.** Unchanged from Phase 1.
- **`simd` is default-off**, and both configurations are tested (Phase 1, Task 1).
- **Test per-crate, never `--workspace`.** Host tests need an explicit
  `--target x86_64-unknown-linux-gnu` in deluge-sdk (its default target is
  `armv7a-none-eabihf`). In flare this stops being necessary — see Task 3.
- **Nothing is deleted from deluge-sdk until flare is proven.** The deletion is
  the last commit of the phase, not the first.
- **No github.com remote in this phase.** flare is a local repo at
  `~/GitHub/flare`, exactly as `~/GitHub/deluge-ndk` has been since July.
  Publishing is a separate, later decision.

## Baseline (verified 2026-09-08, on `main` after Phase 1)

| fact | value |
|---|---|
| goldens | 9 tests, 8 digest-pinned, 10 constants, in `crates/deluge-audio-graph/src/golden.rs` |
| `deluge-audio-graph` tests | 313 (both configs, both targets) |
| `deluge-dsp-kernels` tests | 288 host scalar / 286 host simd / 278 QEMU scalar / 286 QEMU simd |
| `deluge-fft` tests | 25 + 8 |
| `deluge-wren-core` tests | 228 — stays in deluge-sdk, becomes the integration check |
| full runner | `tools/test.sh` → "All tests passed." |

## The rename map

| from (deluge-sdk) | to (flare) | package | `[lib] name` |
|---|---|---|---|
| `crates/deluge-dsp-kernels` | `crates/flare-kernels` | `flare-kernels` | `flare_kernels` |
| `crates/deluge-audio-graph` | `crates/flare-graph` | `flare-graph` | `flare_graph` |
| `crates/deluge-fft` | `crates/flare-fft` | `flare-fft` | `flare_fft` |
| `crates/mipgen` | `crates/flare-mipgen` | `flare-mipgen` | `flare_mipgen` |
| `crates/deluge-dsp-test` | `crates/flare-dsp-test` | `flare-dsp-test` | `flare_dsp_test` |
| — | `crates/flare` | `flare` | `flare` |

`deluge-fft` and `mipgen` have no `[lib] name` today (they take the default), so
those two gain an explicit one.

## File Structure

**Created in `~/GitHub/flare`:**

| file | responsibility |
|---|---|
| `Cargo.toml` | workspace: 6 members, shared package metadata, the `flare-fft` release profile |
| `rust-toolchain.toml` | nightly (`portable_simd`, `generic_const_exprs`) |
| `.cargo/config.toml` | the `gen-tables` alias and the ARM bench/QEMU target settings — but **no** default `[build] target` |
| `tools/test.sh` | flare's own runner: both configs, both buckets |
| `LICENSE-MIT`, `LICENSE-APACHE` | copied from deluge-sdk |
| `PROVENANCE.md` | the clean-room licensing record, quoting its evidence inline |
| `README.md` | what flare is, the crate table, how to test |
| `crates/flare/src/lib.rs` | the facade |

**Modified in `deluge-sdk`:** `Cargo.toml` (members/excludes/profile),
`.cargo/config.toml` (drop the `gen-tables` alias), `tools/test.sh` (drop the
moved crates), `crates/deluge-wren-core/Cargo.toml` + sources,
`wren-firmware/Cargo.toml` + sources, `firmwares/demo-firmware/Cargo.toml` +
`src/tasks/analysis.rs`, `crates/deluge-fixedpoint/Cargo.toml` +
`tests/dsp_pipeline.rs`.

**Deleted from `deluge-sdk` (Task 8 only):** the five crate directories.

---

### Task 1: Prove history preservation is possible before relying on it

`git filter-repo` is not installed, and the whole plan's history story depends
on it. Establish that it works, and capture the "before" facts every later
verification compares against — while the crates are still in place.

**Files:** none modified.

**Interfaces:**
- Consumes: nothing.
- Produces: a recorded baseline (commit counts per crate directory, a
  representative file whose `git log --follow` output is known) that Task 2
  checks its output against.

- [ ] **Step 1: Install `git filter-repo`**

```bash
sudo pacman -S --needed git-filter-repo || pipx install git-filter-repo
git filter-repo --version
```
Expected: a version string. If neither install route works, **stop and report** —
`git subtree split` is a fallback but it handles one prefix at a time and
rewrites paths manually, which is a different plan, not a substitution.

- [ ] **Step 2: Record the "before" history facts**

```bash
cd ~/GitHub/deluge-sdk
for d in crates/deluge-dsp-kernels crates/deluge-audio-graph crates/deluge-fft \
         crates/mipgen crates/deluge-dsp-test; do
  printf '%-34s %s commits\n' "$d" "$(git log --oneline -- "$d" | wc -l)"
done
echo "--- representative file, follow depth:"
git log --follow --oneline -- crates/deluge-dsp-kernels/src/poly.rs | wc -l
git log --follow --oneline -- crates/deluge-audio-graph/src/engine.rs | wc -l
```

Write the numbers into the commit message of Task 2. They are the only way to
tell "history preserved" from "history plausibly present".

- [ ] **Step 3: Record the "before" digests**

The goldens are the behavioural baseline. Capture them explicitly rather than
trusting that they are unchanged later:

```bash
grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/deluge-audio-graph/src/golden.rs
```
Expected: 10 constants. Save this output — Task 5 and Task 7 compare against it
character for character.

- [ ] **Step 4: Confirm the tree is clean and green**

```bash
git status --short && tools/test.sh 2>&1 | tail -2
```
Expected: no output from `status`, and "All tests passed."

- [ ] **Step 5: Commit (baseline note only)**

Nothing has changed in the tree, so there is nothing to commit yet. Proceed to
Task 2 carrying the recorded numbers forward.

---

### Task 2: Split the five crates into `~/GitHub/flare`

**Files:**
- Create: `~/GitHub/flare` (a new local git repo, no remote)

**Interfaces:**
- Consumes: the baseline numbers from Task 1.
- Produces: `~/GitHub/flare` containing `crates/flare-{kernels,graph,fft,mipgen,dsp-test}`
  with history, and nothing else.

- [ ] **Step 1: Clone deluge-sdk into the new location**

`filter-repo` refuses to run on a repo with a remote by default and rewrites
history destructively, so it operates on a fresh clone — never on your working
repo.

```bash
cd ~/GitHub
git clone --no-local deluge-sdk flare
cd flare
git remote remove origin
git log --oneline -1
```
Expected: the clone succeeds and shows the current `main` tip. `--no-local`
forces a real object copy rather than hardlinks, so nothing here can corrupt
`~/GitHub/deluge-sdk`.

- [ ] **Step 2: Filter to the five crate paths, renaming as it goes**

```bash
cd ~/GitHub/flare
git filter-repo --force \
  --path crates/deluge-dsp-kernels --path-rename crates/deluge-dsp-kernels:crates/flare-kernels \
  --path crates/deluge-audio-graph --path-rename crates/deluge-audio-graph:crates/flare-graph \
  --path crates/deluge-fft        --path-rename crates/deluge-fft:crates/flare-fft \
  --path crates/mipgen            --path-rename crates/mipgen:crates/flare-mipgen \
  --path crates/deluge-dsp-test   --path-rename crates/deluge-dsp-test:crates/flare-dsp-test
```

- [ ] **Step 3: Verify the result contains what it should and nothing else**

```bash
cd ~/GitHub/flare
find . -path ./.git -prune -o -type f -print | sed 's|/[^/]*$||' | sort -u | head -20
```
Expected: only paths under `crates/flare-*`. **No `firmwares/`, no `wren-sys/`,
no `docs/`, no `app-loader/`.** If anything else survived, the `--path` list was
wrong — re-clone and redo rather than deleting by hand, so history stays honest.

- [ ] **Step 4: Verify history actually survived**

```bash
cd ~/GitHub/flare
for d in crates/flare-kernels crates/flare-graph crates/flare-fft \
         crates/flare-mipgen crates/flare-dsp-test; do
  printf '%-28s %s commits\n' "$d" "$(git log --oneline -- "$d" | wc -l)"
done
echo "--- follow depth (compare to Task 1 Step 2):"
git log --follow --oneline -- crates/flare-kernels/src/poly.rs | wc -l
git log --follow --oneline -- crates/flare-graph/src/engine.rs | wc -l
echo "--- a real old commit is reachable:"
git log --oneline -- crates/flare-graph/src/engine.rs | tail -3
```
Expected: counts matching Task 1 Step 2, and `--follow` resolving through the
rename. **If the counts collapsed to a handful, history was not preserved** —
stop, re-clone, and investigate before building anything on top.

- [ ] **Step 5: Confirm the source files are byte-identical to the originals**

The rename must not have touched content:

```bash
diff -r ~/GitHub/deluge-sdk/crates/deluge-audio-graph/src ~/GitHub/flare/crates/flare-graph/src && echo "GRAPH IDENTICAL"
diff -r ~/GitHub/deluge-sdk/crates/deluge-dsp-kernels/src ~/GitHub/flare/crates/flare-kernels/src && echo "KERNELS IDENTICAL"
```
Expected: both print IDENTICAL with no diff output.

- [ ] **Step 6: Commit**

`filter-repo` rewrites rather than adds, so the tree is already committed. Record
the split as an empty marker commit carrying the verification numbers:

```bash
cd ~/GitHub/flare
git commit --allow-empty -F - <<'MSG'
chore: split the audio engine out of deluge-sdk

Five crates extracted from deluge-sdk with `git filter-repo`, renamed in the
same pass:

  crates/deluge-dsp-kernels -> crates/flare-kernels
  crates/deluge-audio-graph -> crates/flare-graph
  crates/deluge-fft         -> crates/flare-fft
  crates/mipgen             -> crates/flare-mipgen
  crates/deluge-dsp-test    -> crates/flare-dsp-test

History preserved: per-directory commit counts and `git log --follow` depth
match the pre-split repo, and the source trees are byte-identical to their
originals. Nothing has been removed from deluge-sdk yet — that is the last
commit of this migration, not the first.

deluge-dsp-test comes along because it is a dev-dependency of both the kernels
and mipgen; leaving it behind would make this repo dev-depend on the SDK.
Nothing remaining in deluge-sdk uses it.
MSG
```

---

### Task 3: Make flare a standalone workspace

The crates are present but nothing builds — there is no workspace manifest.

**Files:**
- Create: `~/GitHub/flare/Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml`,
  `LICENSE-MIT`, `LICENSE-APACHE`, `.gitignore`

**Interfaces:**
- Consumes: the crate directories from Task 2.
- Produces: a workspace whose default target is the **host**, so `cargo test`
  works with no `--target` flag. Every later task's commands assume this.

- [ ] **Step 1: Write the workspace manifest**

Create `~/GitHub/flare/Cargo.toml`:

```toml
[workspace]
members = [
  "crates/flare",
  "crates/flare-kernels",
  "crates/flare-graph",
  "crates/flare-fft",
  "crates/flare-mipgen",
  "crates/flare-dsp-test",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"
authors = ["Katherine Whitlock <kate@skylinesynths.nyc>"]
repository = "https://github.com/FirestormAudio/flare"
homepage = "https://github.com/FirestormAudio/flare"

# The FFT is the one crate whose release performance is load-bearing regardless
# of the consumer's profile. Carried over from deluge-sdk's Cargo.toml:129.
[profile.release.package.flare-fft]
opt-level = 3
```

Note every crate is a `members` entry, including `flare-dsp-test` — in
deluge-sdk it was an `exclude` because that workspace cross-compiles to
`armv7a-none-eabihf` by default and `flare-dsp-test` is std-only. flare has no
default target, so the exclusion is unnecessary here. That is a small, real
improvement the move buys.

- [ ] **Step 2: Write `rust-toolchain.toml`**

`flare-fft` needs `#![feature(generic_const_exprs)]` and
`#![feature(portable_simd)]`; `flare-mipgen` needs the former; the `simd`
feature needs the latter.

```toml
[toolchain]
channel = "nightly"
components = ["rust-src"]
```

Deliberately **no `targets`** list: deluge-sdk pins `armv7a-none-eabihf` because
it builds firmware. flare is a library that should build for whatever its
consumer targets, so it pins nothing.

- [ ] **Step 3: Write `.cargo/config.toml`**

```toml
# NOTE: deliberately no `[build] target`. deluge-sdk sets
# `target = "armv7a-none-eabihf"` because it builds firmware, which is why every
# test command there needs an explicit `--target`. flare is a portable library:
# it builds for the host by default so `cargo test` just works, and consumers
# cross-compile it as a dependency.

# QEMU ARM, for testing the NEON path and the 32-bit `usize` arithmetic that the
# 64-bit host cannot reproduce. See tools/test.sh.
[target.armv7-unknown-linux-gnueabihf]
linker = "arm-linux-gnueabihf-gcc"
runner = ["qemu-arm", "-cpu", "cortex-a9", "-L", "/usr/arm-linux-gnueabihf"]
rustflags = [
  "-C", "target-cpu=cortex-a9",
  "-C", "target-feature=+neon",
]

[alias]
# Host-side static-wavetable generator (crates/flare-mipgen, bin gen_tables).
# Usage: cargo gen-tables > crates/flare-kernels/src/wavetables_generated.rs
gen-tables = "run -p flare-mipgen --bin gen_tables --features gen-tables --release"
```

The `gen-tables` alias no longer needs `--target x86_64-unknown-linux-gnu`,
because the host is now the default.

- [ ] **Step 4: Copy the licences and add a `.gitignore`**

```bash
cd ~/GitHub/flare
cp ~/GitHub/deluge-sdk/LICENSE-MIT ~/GitHub/deluge-sdk/LICENSE-APACHE .
printf 'target/\n' > .gitignore
```

Do **not** copy `LICENSE-GPL`. flare contains no GPL code, and shipping the file
would imply otherwise.

- [ ] **Step 5: Verify the workspace is recognised**

```bash
cd ~/GitHub/flare
cargo metadata --no-deps --format-version 1 2>&1 | head -c 400
```
Expected: JSON naming the six members — or a clear error about the packages
still being called `deluge-*`, which Task 4 fixes. Either outcome is progress;
a "virtual manifest" or "no such member" error is not.

- [ ] **Step 6: Commit**

```bash
cd ~/GitHub/flare
git add -A
git commit -m "chore: stand up the flare workspace

Workspace manifest, nightly toolchain (portable_simd, generic_const_exprs),
cargo config and licences. The crates are still named deluge-*; renaming is the
next commit.

Two deliberate differences from deluge-sdk's setup. There is no default build
target: deluge-sdk pins armv7a-none-eabihf because it builds firmware, which is
why every test command there needs an explicit --target. flare is a portable
library, so it builds for the host and consumers cross-compile it. And
flare-dsp-test is a full workspace member rather than an exclude — it was
excluded from deluge-sdk only because that workspace cross-compiles by default
and the crate is std-only.

No LICENSE-GPL: flare contains no GPL code and shipping the file would imply
otherwise."
```

---

### Task 4: Rename the packages and their import paths

**Files:**
- Modify: all five `crates/flare-*/Cargo.toml`
- Modify: every source file naming an old crate (see the sweep in Step 2)

**Interfaces:**
- Consumes: the workspace from Task 3.
- Produces: crates importable as `flare_kernels`, `flare_graph`, `flare_fft`,
  `flare_mipgen`, `flare_dsp_test`.

- [ ] **Step 1: Rename the packages and lib targets**

In each `crates/flare-*/Cargo.toml`, set `[package] name` and `[lib] name` per
the rename map above, and repoint the intra-flare path dependencies:

- `flare-kernels`: deps `libm`; dev-deps `proptest`, `flare-dsp-test = { path = "../flare-dsp-test" }`, `flare-mipgen = { path = "../flare-mipgen" }`
- `flare-graph`: deps `flare-kernels = { path = "../flare-kernels" }`, `libm`; dev-deps `flare-mipgen`; feature `simd = ["flare-kernels/simd"]`
- `flare-fft`: deps none; dev-deps `approx`, `criterion`, `rustfft`
- `flare-mipgen`: deps `flare-fft`, `flare-kernels`, `libm`; dev-deps `flare-dsp-test`
- `flare-dsp-test`: deps `realfft`

`flare-fft`'s dev-deps no longer include `fixedpoint` or `armv7-dsp-intrinsics` —
Phase 1 Task 10 already removed them.

- [ ] **Step 2: Sweep the import paths**

```bash
cd ~/GitHub/flare
grep -rl 'deluge_dsp_kernels\|deluge_audio_graph\|deluge_fft\|deluge_dsp_test\|\bmipgen\b' crates --include='*.rs' | sort
```

Then rewrite, longest-first so no substring of one name eats another:

```bash
cd ~/GitHub/flare
grep -rl 'deluge_dsp_kernels\|deluge_audio_graph\|deluge_dsp_test\|deluge_fft\|mipgen' crates --include='*.rs' \
  | xargs sed -i \
      -e 's/deluge_dsp_kernels/flare_kernels/g' \
      -e 's/deluge_audio_graph/flare_graph/g' \
      -e 's/deluge_dsp_test/flare_dsp_test/g' \
      -e 's/deluge_fft/flare_fft/g' \
      -e 's/\bmipgen\b/flare_mipgen/g'
```

**The `mipgen` substitution is the dangerous one** — it is a bare word, not a
prefixed name. After running it, check it did not corrupt prose or the crate's
own paths:

```bash
grep -rn 'flare_mipgen' crates --include='*.rs' | grep -vE 'use flare_mipgen|flare_mipgen::' | head -20
```
Read every hit. Comments reading "the flare_mipgen crate" are fine; a mangled
path or a rewritten English sentence is not. Fix by hand.

- [ ] **Step 3: Sweep the doc comments for stale crate names**

Source comments reference the old names in prose (e.g. `deluge-audio-graph`'s
`Event`). These are documentation, not code, but leaving them makes the crate
lie about itself:

```bash
cd ~/GitHub/flare
grep -rn 'deluge-audio-graph\|deluge-dsp-kernels\|deluge-fft\|deluge-dsp-test\|Deluge' crates --include='*.rs' | head -30
```
Rewrite the crate-name references. Leave genuine references to the Deluge
hardware where they are factual (e.g. the Cortex-A9 measurement notes in
`wavetable.rs` and `filter.rs`) — those are true statements about where a number
came from, and erasing them would destroy real provenance.

- [ ] **Step 4: Build and test every crate**

```bash
cd ~/GitHub/flare
for p in flare-dsp-test flare-fft flare-mipgen flare-kernels flare-graph; do
  echo "== $p"; cargo test -p $p 2>&1 | grep -E '^test result|^error' | head -5
done
```
Expected: all pass, with counts matching the Phase 2 baseline table
(`flare-graph` 313, `flare-kernels` 288 host-scalar, `flare-fft` 25 + 8).

- [ ] **Step 5: The gate — goldens bit-identical**

```bash
cd ~/GitHub/flare
grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs
cargo test -p flare-graph golden 2>&1 | grep -E '^test golden|^test result'
cargo test -p flare-graph --features simd golden 2>&1 | grep -E '^test result'
```
Expected: the **same 10 constants** recorded in Task 1 Step 3, character for
character, and 9/9 passing in both configurations.

**A changed digest here is a bug in the rename, not a value to re-pin.** The
most likely cause is the `mipgen` sweep having altered a table path or a
constant. Find it.

- [ ] **Step 6: Verify the ARM bucket too**

```bash
cd ~/GitHub/flare
cargo test --target armv7-unknown-linux-gnueabihf -p flare-graph golden 2>&1 | grep '^test result'
cargo test --target armv7-unknown-linux-gnueabihf -p flare-graph --features simd golden 2>&1 | grep '^test result'
```
Expected: 9/9 both times, same constants.

- [ ] **Step 7: Commit**

```bash
cd ~/GitHub/flare
git add -A
git commit -m "refactor: rename the crates to flare-*

deluge-dsp-kernels -> flare-kernels   (lib flare_kernels)
deluge-audio-graph -> flare-graph     (lib flare_graph)
deluge-fft         -> flare-fft       (lib flare_fft)
mipgen             -> flare-mipgen    (lib flare_mipgen)
deluge-dsp-test    -> flare-dsp-test  (lib flare_dsp_test)

deluge-fft and mipgen had no explicit [lib] name and now get one.

A pure rename: all 10 golden digest constants are unchanged, and the goldens
pass 9/9 in both the scalar and simd configurations on the host and under QEMU
ARM. That the audio is bit-identical is what makes a diff this size reviewable.

Doc comments referencing the old crate names are updated. References to the
Deluge hardware itself are left alone where they are factual — the Cortex-A9
measurement notes in wavetable.rs and filter.rs say where a number came from,
and erasing that would destroy real provenance."
```

---

### Task 5: Add the `flare` facade

**Files:**
- Create: `crates/flare/Cargo.toml`, `crates/flare/src/lib.rs`

**Interfaces:**
- Consumes: `flare-graph`, `flare-kernels`.
- Produces: `flare::{Engine, Cmd, Host, Node, Kind, Rate, In, Input, NodeId,
  BusId, CtrlBusId, OutputSrc, StereoFrame, Event, EventQueue, Pool, PoolHandle,
  TableId, VOICES, MonoAllocator, VoiceAllocator, nrt}` — the surface Task 6
  tries to make `deluge-wren-core` consume.

- [ ] **Step 1: Write the manifest**

Create `crates/flare/Cargo.toml`:

```toml
[package]
name = "flare"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
description = "A portable no_std block-rendering audio graph engine: nodes, ports, buses, polyphony as signal width."
categories = ["embedded", "no-std", "multimedia::audio"]
keywords = ["dsp", "audio", "no-std", "synthesis", "graph"]

[dependencies]
flare-graph = { path = "../flare-graph" }
flare-kernels = { path = "../flare-kernels" }

[features]
default = []
simd = ["flare-graph/simd", "flare-kernels/simd"]

[lib]
name = "flare"
path = "src/lib.rs"
```

- [ ] **Step 2: Write the facade**

Create `crates/flare/src/lib.rs`:

```rust
//! A portable `no_std` block-rendering audio graph engine.
//!
//! This crate is a facade: it re-exports the surface most consumers need from
//! [`flare_graph`] and [`flare_kernels`], so a patch-building host names one
//! dependency rather than three. Reach past it to the underlying crates when
//! you need a specific kernel type — that is supported, not a workaround.
//!
//! ```ignore
//! use flare::{BusId, Cmd, Engine, Input, Kind, NodeId};
//!
//! let mut e = Engine::<64, 16, 64, 4, 45056, 2048>::new(48_000.0);
//! e.apply(Cmd::NewNode {
//!     node: NodeId(0),
//!     kind: Kind::Saw,
//!     args: [Input::Const(110.0), Input::Const(0.0), Input::Const(0.0)],
//! });
//! e.apply(Cmd::BusWrite { src: Input::Node { node: NodeId(0), port: 0 }, bus: BusId(0) });
//! e.apply(Cmd::SetRoot { bus: BusId(0) });
//! ```
#![no_std]

// Exactly the set flare-graph re-exports at its own top level.
pub use flare_graph::{
    Arena, BusId, Cmd, Engine, Event, EventQueue, Host, In, Input, Kind, MonoAllocator, Node,
    NodeId, OutputSrc, Pool, PoolHandle, Rate, StereoFrame, TableId, VoiceAllocator, EVENT_QUEUE,
    MAX_GATES, MAX_TRIGGERS, USB_CHANNELS, VOICES,
};

/// `CtrlBusId` and `CTRL_BUSES` are not top-level in `flare-graph` — they live
/// in its `ids` module. Lifted here so a host addressing control buses does not
/// have to know that.
pub use flare_graph::ids::{CtrlBusId, CTRL_BUSES};

/// Offline rendering and the digest/peak/RMS helpers used to characterise a
/// patch's output.
pub use flare_graph::nrt;

/// The node kinds' underlying DSP implementations. Re-exported whole rather
/// than item by item: a consumer binding a wavetable or building a mip pyramid
/// needs types the graph never names in its own signatures.
pub use flare_kernels as kernels;

/// The node-kind enum and its supporting types, for a host that constructs
/// patches programmatically.
pub use flare_graph::node;
```

Adjust the re-export list to whatever `flare-graph`'s `lib.rs` actually exposes —
read it rather than trusting this list:

```bash
sed -n '/^pub use/,$p' crates/flare-graph/src/lib.rs
```

- [ ] **Step 3: Verify it compiles and the surface resolves**

```bash
cd ~/GitHub/flare
cargo build -p flare
cargo build -p flare --features simd
```
Expected: both succeed. An unresolved re-export means the name does not exist in
`flare-graph`; fix the list against the source, do not invent names.

- [ ] **Step 4: Add a test that the facade is actually usable**

Create `crates/flare/tests/facade.rs`:

```rust
//! The facade is only worth having if a patch can be built through it alone.
//! This test names nothing but `flare::`, so it fails if a re-export is missing
//! rather than merely unused.

use flare::{BusId, Cmd, Engine, Input, Kind, NodeId, StereoFrame};

#[test]
fn a_patch_can_be_built_and_rendered_through_the_facade_alone() {
    let mut e = Engine::<64, 16, 64, 4, 45056, 2048>::new(48_000.0);
    e.apply(Cmd::NewNode {
        node: NodeId(0),
        kind: Kind::Saw,
        args: [Input::Const(110.0), Input::Const(0.0), Input::Const(0.0)],
    });
    e.apply(Cmd::BusWrite {
        src: Input::Node { node: NodeId(0), port: 0 },
        bus: BusId(0),
    });
    e.apply(Cmd::SetRoot { bus: BusId(0) });

    let mut out = [StereoFrame::default(); 256];
    e.render_offline(&mut out);

    assert!(flare::nrt::is_clean(&out), "finite and within [-1, 1]");
    assert!(flare::nrt::peak(&out) > 1e-3, "not silent");
}
```

- [ ] **Step 5: Run it**

```bash
cd ~/GitHub/flare
cargo test -p flare 2>&1 | grep -E '^test result|^error'
```
Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
cd ~/GitHub/flare
git add -A
git commit -m "feat: add the flare facade crate

Re-exports the surface a patch-building host needs from flare-graph and
flare-kernels, so a consumer names one dependency rather than three. Reaching
past it to the underlying crates stays supported — the kernels are re-exported
whole as flare::kernels for exactly that reason.

The integration test names nothing but flare::, so a missing re-export fails
the build rather than going unnoticed until a consumer trips on it."
```

---

### Task 6: Rewire deluge-sdk onto flare

The moved crates still exist in deluge-sdk; this task points every consumer at
flare instead, without deleting anything. Keeping both present means the switch
is verifiable in isolation, and revertable with a single `git checkout`.

**Files (all in `~/GitHub/deluge-sdk`):**
- Modify: `Cargo.toml` (members, excludes, `[workspace.dependencies]`, profile)
- Modify: `crates/deluge-wren-core/Cargo.toml` and its sources
- Modify: `wren-firmware/Cargo.toml` and `src/{audio,host,stream}.rs`
- Modify: `firmwares/demo-firmware/Cargo.toml` and `src/tasks/analysis.rs`
- Modify: `crates/deluge-fixedpoint/Cargo.toml` and `tests/dsp_pipeline.rs`
- Modify: `crates/armv7-dsp-intrinsics/src/lib.rs` (a doc reference only)

**Interfaces:**
- Consumes: the flare workspace from Tasks 3-5.
- Produces: a deluge-sdk that builds against flare by path dependency.

- [ ] **Step 1: Add the flare path dependencies to the workspace**

In `~/GitHub/deluge-sdk/Cargo.toml`, under `[workspace.dependencies]`:

```toml
# The audio engine, extracted to its own repo (see
# docs/superpowers/specs/2026-09-08-flare-audio-engine-extraction-design.md).
# A path dependency on a sibling checkout: flare has no remote yet, exactly as
# deluge-ndk has none. Swapping this for a pinned git dependency is a later,
# separate decision.
flare         = { path = "../flare/crates/flare" }
flare-graph   = { path = "../flare/crates/flare-graph" }
flare-kernels = { path = "../flare/crates/flare-kernels" }
flare-fft     = { path = "../flare/crates/flare-fft" }
flare-mipgen  = { path = "../flare/crates/flare-mipgen" }
```

- [ ] **Step 2: Repoint `deluge-wren-core`**

Replace its `deluge-audio-graph`, `deluge-dsp-kernels` and `mipgen`
dependencies with `flare-graph.workspace = true`, `flare-kernels.workspace =
true`, `flare-mipgen.workspace = true`, then sweep its sources:

```bash
cd ~/GitHub/deluge-sdk
sed -i -e 's/deluge_audio_graph/flare_graph/g' \
       -e 's/deluge_dsp_kernels/flare_kernels/g' \
       -e 's/\bmipgen\b/flare_mipgen/g' \
  crates/deluge-wren-core/src/*.rs crates/deluge-wren-core/tests/*.rs
```

- [ ] **Step 3: Test whether the facade is sufficient (spec §7)**

Spec §7 flags that `deluge-wren-core` names `flare-kernels` types directly
(`TableId`, `TableSrc`). Now find out whether it needs to:

```bash
cd ~/GitHub/deluge-sdk
grep -rn 'flare_kernels::' crates/deluge-wren-core/src crates/deluge-wren-core/tests | sort -u
```

For each hit, check whether `flare::` re-exports an equivalent. Where it does,
prefer the facade. Where it does not, **record what is missing** — that is the
finding spec §7 asked for, and it tells you whether the facade is real or
decorative. Do not add re-exports to flare in this task; note them and decide in
Task 9.

- [ ] **Step 4: Repoint `wren-firmware` and `demo-firmware`**

```bash
cd ~/GitHub/deluge-sdk
sed -i -e 's/deluge_audio_graph/flare_graph/g' \
       -e 's/deluge_dsp_kernels/flare_kernels/g' \
       -e 's/deluge_fft/flare_fft/g' \
  wren-firmware/src/*.rs firmwares/demo-firmware/src/tasks/analysis.rs
```

and update both `Cargo.toml`s to the workspace flare dependencies.

- [ ] **Step 5: Repoint the relocated pipeline test**

`crates/deluge-fixedpoint/Cargo.toml`'s dev-dependency becomes
`flare-fft.workspace = true`, and `tests/dsp_pipeline.rs`'s
`use deluge_fft::{Complex, Fft};` becomes `use flare_fft::{Complex, Fft};`.

- [ ] **Step 6: Remove the moved crates from the workspace lists**

In `~/GitHub/deluge-sdk/Cargo.toml`, delete these `members` entries —
`crates/deluge-dsp-kernels`, `crates/deluge-audio-graph`, `crates/deluge-fft`,
`crates/mipgen` — and the `crates/deluge-dsp-test` `exclude` entry, plus the
`[profile.release.package.deluge-fft]` block (it moved to flare in Task 3).

The directories still exist on disk but are no longer part of the build. That is
deliberate: it proves nothing is silently still using them.

- [ ] **Step 7: Drop the `gen-tables` alias**

In `~/GitHub/deluge-sdk/.cargo/config.toml`, delete the `gen-tables` alias and
its comment block (lines ~67-72). The generator lives in flare now; leaving a
broken alias behind is worse than removing it.

- [ ] **Step 8: Update `tools/test.sh`**

Delete the six lines that test the moved crates — the `deluge-dsp-kernels` and
`deluge-audio-graph` runs in both buckets and both configs, and the
`deluge-fft` runs — and replace them with a comment:

```bash
# The audio engine (flare-kernels, flare-graph, flare-fft) moved to its own
# repo and is tested by ~/GitHub/flare/tools/test.sh. deluge-wren-core below is
# this repo's integration check against it.
```

Keep `deluge-fixedpoint` without `--lib` (Phase 1 Task 10) — the pipeline test
still lives there and now uses `flare-fft`.

- [ ] **Step 9: Build and test deluge-sdk**

```bash
cd ~/GitHub/deluge-sdk
cargo build -p deluge-wren-core --target armv7a-none-eabihf 2>&1 | tail -5
tools/test.sh 2>&1 | tail -3
```
Expected: builds, and "All tests passed" — including all 228 `deluge-wren-core`
tests, which are the real integration check that the rewire worked.

- [ ] **Step 10: Build the firmware**

```bash
cd ~/GitHub/deluge-sdk
cargo build --release -p wren-firmware 2>&1 | tail -5
cargo build --release -p demo-firmware 2>&1 | tail -5
```
Expected: both link for `armv7a-none-eabihf`. This is the proof flare still
cross-compiles `no_std` to the device target as a path dependency.

- [ ] **Step 11: Commit**

```bash
cd ~/GitHub/deluge-sdk
git add -A
git commit -m "refactor: consume the audio engine from flare

deluge-wren-core, wren-firmware, demo-firmware and the fixedpoint pipeline test
now depend on flare-graph / flare-kernels / flare-fft / flare-mipgen by path
(../flare), and the four crates leave this workspace's members list.

The directories are still on disk but out of the build, deliberately: if
anything were silently still using them this would not link. Deleting them is
the next commit.

Verified: wren-firmware and demo-firmware both link for armv7a-none-eabihf, so
flare still cross-compiles no_std to the device as a dependency, and all 228
deluge-wren-core tests pass — they are this repo's integration check against
the engine now that its own tests live in the other repo.

Drops the gen-tables cargo alias (the generator moved) and the moved crates'
lines from tools/test.sh."
```

---

### Task 7: flare's own test runner

flare needs the two-bucket, two-config runner deluge-sdk has, or its tests only
ever run the way whoever is at the keyboard remembers to run them.

**Files:**
- Create: `~/GitHub/flare/tools/test.sh`

**Interfaces:**
- Consumes: the flare workspace.
- Produces: `tools/test.sh` — the command Task 8 Step 2 and every later phase use.

- [ ] **Step 1: Write the runner**

```bash
mkdir -p ~/GitHub/flare/tools
cat > ~/GitHub/flare/tools/test.sh <<'EOF'
#!/usr/bin/env bash
#
# Canonical test runner for the flare workspace.
#
# Two axes, both load-bearing:
#
#   Configuration: `simd` is default-OFF, so the plain runs are the scalar
#   oracle and the --features simd runs are the f32x8 fast path. The kernels'
#   scalar==simd equivalence tests only mean anything if both are built, and a
#   bug in one path is invisible to the other — perturbing one constant in
#   fast_sin fails 5 of 9 goldens scalar but only 1 of 9 under simd.
#
#   Target: the host (x86-64, core::simd -> SSE/AVX) and 32-bit ARM under QEMU
#   (NEON, and `usize == u32`). These crates are full of offset arithmetic —
#   arena rows, pool chunks, mip-pyramid offsets — where an overflow panics on a
#   32-bit device and CANNOT be reproduced on the 64-bit host.
#
# Prerequisites for the ARM bucket (Arch: pacman; Debian/Ubuntu: apt):
#   - rustup target armv7-unknown-linux-gnueabihf
#   - qemu-user (provides qemu-arm)            [apt: qemu-user]
#   - arm-linux-gnueabihf-gcc (cross linker)   [apt: gcc-arm-linux-gnueabihf]
# Configured in .cargo/config.toml.
#
# Usage: tools/test.sh
set -euo pipefail

cd "$(dirname "$0")/.."

ARM=armv7-unknown-linux-gnueabihf
CRATES=(flare-dsp-test flare-fft flare-mipgen flare-kernels flare-graph flare)

echo "==> Host bucket (default target)"
for p in "${CRATES[@]}"; do cargo test -p "$p"; done
echo "==> Host bucket, simd"
cargo test -p flare-kernels --features simd
cargo test -p flare-graph   --features simd
cargo test -p flare         --features simd

if rustup target list --installed | grep -q "^${ARM}\$" && command -v qemu-arm >/dev/null; then
  echo "==> QEMU ARM bucket ($ARM)"
  for p in "${CRATES[@]}"; do cargo test --target "$ARM" -p "$p"; done
  echo "==> QEMU ARM bucket, simd (NEON)"
  cargo test --target "$ARM" -p flare-kernels --features simd
  cargo test --target "$ARM" -p flare-graph   --features simd
  cargo test --target "$ARM" -p flare         --features simd
else
  echo "  (skipped ARM bucket: target or qemu-arm not installed)"
fi

echo "==> All tests passed."
EOF
chmod +x ~/GitHub/flare/tools/test.sh
```

- [ ] **Step 2: Run it**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | tail -5
```
Expected: "All tests passed." If the ARM bucket is skipped, install the
prerequisites rather than accepting a half-run — the 32-bit path is where the
offset-arithmetic bugs live.

- [ ] **Step 3: Commit**

```bash
cd ~/GitHub/flare
git add -A
git commit -m "test: add flare's canonical two-bucket, two-config runner

Both axes are load-bearing and the header says why. simd is default-off, so a
bug in one path is invisible to the other — perturbing one constant in fast_sin
fails 5 of 9 goldens scalar but only 1 of 9 under simd. And the 32-bit ARM
bucket catches offset-arithmetic overflows that a 64-bit host cannot reproduce,
in code that is full of arena rows, pool chunks and mip-pyramid offsets."
```

---

### Task 8: The Phase 2 gate

Confirm the move changed nothing. This task writes no code.

**Files:** none modified.

**Interfaces:**
- Consumes: Tasks 2-7.
- Produces: the evidence that Phase 3 is safe to start.

- [ ] **Step 1: Golden digests unchanged, character for character**

```bash
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' ~/GitHub/flare/crates/flare-graph/src/golden.rs) \
     <(cd ~/GitHub/deluge-sdk && git show main:crates/deluge-audio-graph/src/golden.rs | grep -E 'const [A-Z0-9_]+_DIGEST: u64') \
  && echo "DIGESTS IDENTICAL"
```
Expected: `DIGESTS IDENTICAL`, no diff output.

- [ ] **Step 2: Full flare suite, both configs, both buckets**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | tail -3
```

- [ ] **Step 3: Full deluge-sdk suite**

```bash
cd ~/GitHub/deluge-sdk && tools/test.sh 2>&1 | tail -3
```
Expected: "All tests passed."

- [ ] **Step 4: flare names no `deluge-*` crate**

```bash
grep -rn 'deluge' ~/GitHub/flare/crates --include='Cargo.toml'
```
Expected: **no output.** Any hit is a back-dependency across the seam and must
be resolved before the phase is done.

- [ ] **Step 5: Prove the net still catches a regression in its new home**

The goldens moved repos; confirm they still work as a gate rather than assuming
the move preserved that:

```bash
cd ~/GitHub/flare
sed -i '72s/0\.225/0.226/' crates/flare-kernels/src/lib.rs
cargo test -p flare-graph golden 2>&1 | grep -E '^test result'
git checkout crates/flare-kernels/src/lib.rs
cargo test -p flare-graph golden 2>&1 | grep -E '^test result'
```
Expected: `5 failed` then `9 passed` — matching the Phase 1 result exactly. A
different failure count means something about the move changed which code the
goldens reach.

- [ ] **Step 6: Commit (nothing to commit; record in Task 9)**

---

### Task 9: Delete the moved crates and record the result

The last commit of the migration. Everything before this is revertable by
pointing deluge-sdk back at its own crates; after it, flare is the only copy —
which is why it comes after the gate, not before.

**Files:**
- Delete: the five crate directories in `~/GitHub/deluge-sdk`
- Create: `~/GitHub/flare/README.md`, `~/GitHub/flare/PROVENANCE.md`
- Modify: the spec, recording the Phase 2 result

**Interfaces:**
- Consumes: a green Task 8.
- Produces: the finished split.

- [ ] **Step 1: Refuse to proceed unless the gate passed**

Re-run Task 8 Steps 1, 3 and 4. If any fails, **stop.** Deleting the only other
copy of 47k lines on a red gate is the one genuinely unrecoverable act in this
plan.

- [ ] **Step 2: Write flare's `PROVENANCE.md`**

The licensing record, quoting its evidence inline rather than citing documents
that live in another repo:

```markdown
# Provenance

flare is `MIT OR Apache-2.0` throughout. Its DSP kernels were written
clean-room from published algorithm descriptions, not ported from GPL sources.
This file records the evidence, because a licensing position that lives only in
someone's memory is not a position.

## The kernels are not derived from GPL code

- `flare-kernels/src/eq.rs` — *"RBJ parametric EQ … Coefficients (RBJ 'Audio EQ
  Cookbook') recomputed per block. Not ported from any GPL source."*
- `flare-kernels/src/reverb.rs` — *"Schroeder-Moorer (freeverb-topology) room
  reverb … Implemented from the public algorithm structure — not the GPL
  freeverb source."*
- The plate reverb was built from the public Dattorro 1997 paper, with its
  constants cross-checked against an MIT-licensed implementation.
- `flare-kernels/src/wavetables_generated.rs` is synthesized analytically by
  `flare-mipgen`'s `gen_tables` binary — Fourier series for saw, square, sine,
  triangle, organ and formant. No sampled data from any instrument's ROM.

Every DSP implementation plan written during development carried an explicit
constraint line, e.g. *"Licensing: MIT / Apache-2.0 only. No GPL."* and *"Do NOT
port spark's GPL-3.0 `deluge/fx/eq.rs` or `freeverb`."* Those plans remain in
the `deluge-sdk` repository's history, under `docs/superpowers/plans/`.

## What was GPL, and stayed behind

The `deluge-sdk` project these crates came from does contain GPL-3.0 code — the
OLED UI toolkit, its fonts, and the pad-grid colour toolkit, all vendored from
`spark`. None of it was ever in this engine's dependency tree; deluge-sdk's
README made "the permissive facade does not depend on them" an explicit
invariant. None of it moved here.
```

- [ ] **Step 3: Write flare's `README.md`**

Cover: what flare is (a portable `no_std`, zero-allocation, block-rendering
audio graph with polyphony as signal width); the six-crate table from this
plan's rename map; `simd` being default-off with the scalar path as oracle;
`tools/test.sh`; and the licence. Keep it short — the crate docs carry the
detail.

- [ ] **Step 4: Delete the crate directories from deluge-sdk**

```bash
cd ~/GitHub/deluge-sdk
git rm -r --quiet crates/deluge-dsp-kernels crates/deluge-audio-graph \
                  crates/deluge-fft crates/mipgen crates/deluge-dsp-test
```

- [ ] **Step 5: Verify deluge-sdk still builds with them gone**

```bash
cd ~/GitHub/deluge-sdk
tools/test.sh 2>&1 | tail -3
cargo build --release -p wren-firmware 2>&1 | tail -3
```
Expected: "All tests passed" and a linked firmware. Anything referencing a
deleted path fails loudly here, which is the point of deleting last.

- [ ] **Step 6: Record the result in the spec**

Append to the spec's `### Phase 2` subsection what actually happened: the
per-directory commit counts proving history survived, confirmation that all 10
digests are unchanged, the firmware link, the 228 wren-core tests, the
regression-catch re-verification from Task 7 Step 5, and — importantly — the
Task 6 Step 3 finding about whether the facade was sufficient for
`deluge-wren-core` or had to be reached past.

- [ ] **Step 7: Commit both repos**

```bash
cd ~/GitHub/flare
git add -A
git commit -m "docs: add flare's README and provenance record

PROVENANCE.md quotes its licensing evidence inline rather than citing plan
documents that live in another repository — the clean-room disclaimers in
eq.rs and reverb.rs, the Dattorro paper origin of the plate reverb, and the
fact that the wavetables are synthesized analytically rather than sampled.

A licensing position that lives only in someone's memory is not a position:
this project started from the assumption that these kernels were GPL, and that
assumption was wrong by 29.7k lines."
```

```bash
cd ~/GitHub/deluge-sdk
git add -A
git commit -m "refactor: remove the audio engine crates, now in flare

deluge-dsp-kernels, deluge-audio-graph, deluge-fft, mipgen and deluge-dsp-test
are deleted here and live in ~/GitHub/flare as flare-kernels, flare-graph,
flare-fft, flare-mipgen and flare-dsp-test, with history preserved.

This is the last commit of the migration rather than the first, so every step
before it was revertable by pointing back at these directories. The gate it
waited on: all 10 golden digests bit-identical, both firmwares linking for
armv7a-none-eabihf, 228 deluge-wren-core tests passing, and flare's Cargo.toml
files naming no deluge- crate at all.

flare has no remote — a local sibling checkout, as deluge-ndk has been since
July. Whether it gets published is a separate decision."
```

---

## What this plan does NOT cover

- **A CI workflow for flare.** The spec's Phase 2 mentions CI. flare has no
  remote in this phase, so a GitHub Actions workflow would run nowhere;
  `tools/test.sh` (Task 7) is the runnable equivalent and is what a workflow
  would call. The workflow lands with publishing, or with Phase 4's target
  matrix — whichever comes first.
- **Publishing flare to github.com.** Deliberately out of scope; flare is a
  local repo. If and when it is published, the path dependencies become a git
  dependency pinned by rev with a `[patch]` override, and deluge-sdk's CI gains
  a check that it builds against the **pinned rev** rather than the override
  (spec §7).
- **Phase 3** — `AUX_OUTS`, the `MAX_BLOCK` feature, the `VOICES` chunk
  refactor, scoped updates, the multi-engine test.
- **Phase 4** — the four-target CI matrix.
- **Any behaviour change.** If something in flare looks wrong during this phase,
  write it down and fix it in Phase 3, where a digest change is allowed to mean
  something.
