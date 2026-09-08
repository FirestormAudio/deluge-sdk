# Flare Phase 4 — The Portability Matrix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn "flare is portable" from an assertion into something a command
proves — by *running* its tests on a third architecture, and build-guarding the
targets that cannot run tests at all.

**Architecture:** flare already builds everywhere it claims to (verified below),
so this phase is not a porting effort. Its value is in execution and lock-in:
wasm32 gains a test **runner** so the goldens actually execute there, Cortex-M
gains honest build guards, and `tools/test.sh` becomes an explicit matrix rather
than a list that grew.

**Tech Stack:** Rust nightly, `core::simd`, two Cargo workspaces, `cargo test`
with per-target runners (`qemu-arm` for ARM32, `wasmtime` or `node:wasi` for
wasm32).

**Spec:** `docs/superpowers/specs/2026-09-08-flare-audio-engine-extraction-design.md`
(this plan implements Phase 4, spec §4)

**Depends on:** Phases 1-3, complete and merged.

## Global Constraints

- **Licensing:** MIT / Apache-2.0 only. No GPL.
- **The default configuration's output must not move.** The ten golden digests
  stay byte-identical. This phase adds targets; it changes no audio.
- **NEON is kept**, and so is every other SIMD lowering. Nothing here makes a
  target fall back to scalar.
- **A build guard is weaker than a test run, and must be labelled as such.**
  Where a target cannot execute tests, say so where someone will read it, rather
  than letting a green line imply more than it means.
- **`no_std`, no heap, no panics in DSP paths.**
- `flare/tools/test.sh` is the acceptance command for every task.

## Research findings (verified 2026-09-08, before writing this plan)

These change what the phase is, so they are recorded rather than assumed:

| finding | consequence |
|---|---|
| `flare` builds for `wasm32-unknown-unknown`, `thumbv7m-none-eabi` and `thumbv8m.main-none-eabihf`, **including `--features simd` and `--features voices-16,simd`** | No porting work. `core::simd` scalarises cleanly on a Cortex-M3 with no FPU. |
| `cargo test --target wasm32-wasip1` builds a runnable test binary, and **324 `flare-graph` tests pass under `node:wasi`** — scalar and `simd`, and 319 at `voices-16,simd` | wasm can *run* tests, not just build. This is the phase's centrepiece. |
| Those runs include every golden, and they **match the digests pinned on x86-64** | The digests are architecture-invariant across three ISAs and three SIMD backends. Worth asserting deliberately. |
| `flare-kernels` cannot build tests for wasm: proptest's transitive `wait-timeout` has no wasm support | Kernels are build-guarded on wasm. Marginal loss — see Task 3's note. |
| `thumbv7em-none-eabihf` is **not** installed; `thumbv7m-none-eabi` (Cortex-M3, **no FPU**) and `thumbv8m.main-none-eabihf` are | Use what is installed. M3 soft-float is a *harsher* test than M4F. |
| `wasmtime` 48.0.1 is packaged; `node:wasi` works today with no install | Prefer wasmtime, fall back to node. The plan must not hard-require a `sudo` install. |
| `arm-none-eabi-gcc` installed 2026-09-08; **`wren-firmware` and `demo-firmware` now link against flare** (static ARM ELF, 93 flare symbols in `wren-firmware`) | Phase 2's one deferred acceptance step is closed. Nothing outstanding. |

## The matrix, and why it is not the cross product

Four targets × 2 SIMD configs × 3 voice counts × 4 block ceilings is 96
combinations, which would be slow and would not buy proportionate confidence.
The matrix is deliberately uneven, because the targets differ in what they can
prove:

| target | runs tests? | configs | what it uniquely proves |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | yes | all: scalar, simd, 3 voice counts, 4 block ceilings | the full behavioural surface; SSE/AVX lowering |
| `armv7-unknown-linux-gnueabihf` (QEMU) | yes | scalar, simd | NEON, and 32-bit `usize` in the offset arithmetic |
| `wasm32-wasip1` | **yes** (new) | scalar, simd | simd128 lowering, and a third ISA for the digests |
| `armv7a-none-eabihf` | build only | scalar, simd | bare-metal `no_std`, the actual deploy target |
| `thumbv7m-none-eabi` | build only | scalar, simd | Cortex-M, **no FPU** — soft-float, the harshest build |
| `thumbv8m.main-none-eabihf` | build only | scalar, simd | Cortex-M33, hard float |

Feature-combination breadth lives on the host, where it is cheapest. The other
targets carry the axis they alone can test.

## File Structure

| file | change |
|---|---|
| `flare/tools/wasi-run.mjs` (create) | node:wasi runner, the no-install fallback |
| `flare/.cargo/config.toml` (modify) | `wasm32-wasip1` runner entry |
| `flare/tools/test.sh` (modify) | restructured as the explicit matrix above |
| `flare/crates/flare-graph/src/golden.rs` (modify) | the architecture-invariance note |
| `flare/README.md` (modify) | the matrix, and what each row does and does not prove |
| `flare/.github/workflows/test.yml` (create) | calls `tools/test.sh`; inert until flare has a remote |

---

### Task 1: Run flare's tests on wasm

The centrepiece. Everything else in this phase is lock-in; this adds evidence
that does not exist today.

**Files:**
- Create: `flare/tools/wasi-run.mjs`
- Modify: `flare/.cargo/config.toml`

**Interfaces:**
- Consumes: nothing.
- Produces: `cargo test --target wasm32-wasip1 -p flare-graph` runs, with no
  manual runner plumbing. Tasks 2 and 4 depend on it.

- [ ] **Step 1: Record the baseline digests**

```bash
cd ~/GitHub/flare
grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs > /tmp/flare-digests-phase4.txt
wc -l < /tmp/flare-digests-phase4.txt
```
Expected: 10.

- [ ] **Step 2: Write the node:wasi runner**

`wasmtime` is the better runner but needs a `sudo` install; `node:wasi` works
today. Write the fallback so the phase never blocks on a package, and prefer
wasmtime when it is present (Step 3).

Create `flare/tools/wasi-run.mjs`:

```javascript
// Minimal WASI runner, so `cargo test --target wasm32-wasip1` can execute.
//
// Exists because wasmtime is not always installed and this needs no install:
// node has shipped `node:wasi` for years. `.cargo/config.toml` prefers wasmtime
// when it is on PATH and falls back to this.
//
// `returnOnExit` makes `wasi.start` hand back the exit code instead of killing
// the process, so a failing test suite propagates its status to cargo.
import { WASI } from 'node:wasi';
import { argv, env, exit } from 'node:process';
import { readFile } from 'node:fs/promises';

const [, , wasmPath, ...rest] = argv;
if (!wasmPath) {
  console.error('usage: node tools/wasi-run.mjs <file.wasm> [args...]');
  exit(2);
}
const wasi = new WASI({
  version: 'preview1',
  args: [wasmPath, ...rest],
  env,
  // The test harness reads nothing, but a preopen is required for WASI to
  // initialise; '/' is the least surprising choice for a local test run.
  preopens: { '/': '/' },
  returnOnExit: true,
});
const module = await WebAssembly.compile(await readFile(wasmPath));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
exit(wasi.start(instance));
```

- [ ] **Step 3: Wire it up as the cargo runner**

In `flare/.cargo/config.toml`, after the ARM block:

```toml
# wasm32, for the simd128 lowering of `core::simd` and a third ISA to check the
# characterisation digests against. `wasm32-wasip1` rather than
# `wasm32-unknown-unknown` because the test harness needs WASI to print results
# and set an exit code.
#
# `node:wasi` is the runner because it needs no install. If you have wasmtime,
#   runner = "wasmtime"
# is faster and less experimental — swap the line.
[target.wasm32-wasip1]
runner = ["node", "--no-warnings", "tools/wasi-run.mjs"]
```

- [ ] **Step 4: Verify it runs**

```bash
cd ~/GitHub/flare
cargo test --target wasm32-wasip1 -p flare-graph 2>&1 | grep -E '^test result'
cargo test --target wasm32-wasip1 -p flare-graph --features simd 2>&1 | grep -E '^test result'
```
Expected: `324 passed; 0 failed` from both. If cargo reports it cannot run the
binary, the runner path is relative to the workspace root — check you are in
`~/GitHub/flare`.

- [ ] **Step 5: Verify the goldens specifically**

```bash
cd ~/GitHub/flare
cargo test --target wasm32-wasip1 -p flare-graph --features simd golden 2>&1 | grep -E '^test golden|^test result'
```
Expected: 10 passed. **These are the digests pinned on x86-64.** Passing here
means the engine renders bit-identical audio on a third architecture through a
third SIMD backend — which is the single most valuable fact this phase
establishes.

- [ ] **Step 6: Confirm the kernels' wasm limitation, and its shape**

```bash
cd ~/GitHub/flare
cargo test --target wasm32-wasip1 -p flare-kernels --no-run 2>&1 | grep -m2 -E '^error|could not compile'
cargo build --target wasm32-wasip1 -p flare-kernels 2>&1 | grep -E '^error|Finished'
```
Expected: the *test* build fails on `wait-timeout` (a proptest transitive
dependency with no wasm support), while the *library* builds fine. Record that
distinction — the crate is portable; its test harness is not.

- [ ] **Step 7: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "test: run flare's tests on wasm

cargo test --target wasm32-wasip1 now executes, via a small node:wasi runner
wired up in .cargo/config.toml. node rather than wasmtime because it needs no
install; the config says how to switch if you have wasmtime.

324 flare-graph tests pass there in both configurations, including all ten
characterisation goldens — and those digests were pinned on x86-64. The engine
renders bit-identical audio on a third architecture through a third SIMD
backend (simd128, against NEON and SSE/AVX), which is the strongest determinism
evidence the project has.

flare-kernels' library builds for wasm but its tests cannot: proptest's
transitive wait-timeout has no wasm support. The crate is portable; its test
harness is not."
```

---

### Task 2: Assert the digests are architecture-invariant

Task 1 *observed* that the digests hold on three architectures. That is a
property worth asserting deliberately, and worth writing down where the next
person will find it — otherwise the next re-pin quietly discards it.

**Files:**
- Modify: `flare/crates/flare-graph/src/golden.rs` (module doc)

**Interfaces:**
- Consumes: Task 1's runner.
- Produces: documentation only. No new test — the goldens *are* the test; what
  is missing is the statement of what their passing on three targets means.

- [ ] **Step 1: Extend the module doc**

The module doc currently says goldens are "target-invariant" and names two
buckets. Make it say what is now actually true, and why it matters:

```rust
//! Every golden is **architecture-invariant**, and that is a stronger claim
//! than it sounds. The same digests hold on x86-64, on 32-bit ARM under QEMU,
//! and on wasm32 — three ISAs, three `core::simd` lowerings (SSE/AVX, NEON,
//! simd128), and two pointer widths. Nothing in the engine's audio path depends
//! on the machine it runs on.
//!
//! That is a deliberate property, not a coincidence: the kernels use no
//! fast-math, no fused-multiply-add that would vary by target, and no
//! `f32`-to-`usize` conversion whose rounding differs. If a change makes a
//! digest differ *between targets* rather than between versions, that is a
//! portability bug and the digest is what found it.
```

- [ ] **Step 2: Verify the claim on all three, in both configurations**

Before writing it down, check it. A documented invariant nobody re-ran is
folklore.

```bash
cd ~/GitHub/flare
for t in "" "--target armv7-unknown-linux-gnueabihf" "--target wasm32-wasip1"; do
  for f in "" "--features simd"; do
    printf '%-42s ' "${t:-host} ${f:-scalar}"
    cargo test $t -p flare-graph $f golden 2>&1 | grep -m1 -oE '[0-9]+ passed; [0-9]+ failed'
  done
done
```
Expected: `10 passed; 0 failed` in all six. **If any target disagrees, do not
write the doc** — report which digest differs where; that is a portability bug
and a real finding.

- [ ] **Step 3: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "docs(golden): state the architecture-invariance the digests now prove

The module said 'target-invariant' and named two buckets. It is now three ISAs,
three core::simd lowerings and two pointer widths, all producing identical
digests — verified across six target/config combinations before this was
written down.

Says why it holds (no fast-math, no target-varying FMA, no rounding-dependent
float-to-int) so that a future digest difference *between targets* is read as
the portability bug it would be, rather than as a golden needing a re-pin."
```

---

### Task 3: Bare-metal build guards for Cortex-M

The deploy target (`armv7a-none-eabihf`) is already guarded. Cortex-M is the
target that would find assumptions the Cortex-A does not — especially
`thumbv7m-none-eabi`, which has **no FPU**, so every `f32` operation is
soft-float and `core::simd` must scalarise entirely.

**Files:**
- Modify: `flare/tools/test.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: build guards Task 4 folds into the matrix.

- [ ] **Step 1: Verify the targets build, and note which are installed**

```bash
cd ~/GitHub/flare
for t in thumbv7m-none-eabi thumbv8m.main-none-eabihf; do
  rustup target list --installed | grep -qx "$t" || { echo "$t NOT INSTALLED"; continue; }
  for f in "" "--features simd" "--features voices-16,simd"; do
    printf '%-30s %-22s ' "$t" "${f:-default}"
    cargo build -p flare --target "$t" $f 2>&1 | grep -qE '^error' && echo FAIL || echo ok
  done
done
```
Expected: `ok` throughout. This was verified while planning; re-run it because a
plan's research goes stale.

**On `thumbv7em-none-eabihf`:** the spec named it, and it is not installed here.
`thumbv7m-none-eabi` is a strictly harder test (M3 has no FPU where M4F does),
so the matrix uses what is present. Add the other with `rustup target add
thumbv7em-none-eabihf` if you want M4F specifically; it is one more line in the
loop below.

- [ ] **Step 2: Add the guards to the runner**

In `flare/tools/test.sh`, extend the existing bare-metal section:

```bash
# Targets that cannot run tests — no test harness on bare metal — so these are
# BUILD guards, and weaker than the runs above. They catch what a build catches:
# missing `no_std` support, a type that assumes 64-bit, a SIMD path that will
# not lower. They cannot catch wrong audio.
#
# thumbv7m (Cortex-M3) earns its place by having NO FPU: every f32 op is
# soft-float and core::simd must scalarise completely. If flare ever grows a
# hidden hardware-float assumption, this is where it surfaces.
echo "==> Bare-metal build guards (no test harness — builds only)"
for t in armv7a-none-eabihf thumbv7m-none-eabi thumbv8m.main-none-eabihf; do
  if rustup target list --installed | grep -qx "$t"; then
    echo "    -- $t"
    cargo build -p flare --target "$t"
    cargo build -p flare --target "$t" --features simd
  else
    echo "    (skipped $t: not installed)"
  fi
done
```

This replaces the existing single-target bare-metal guard.

- [ ] **Step 3: Run it**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | grep -E '^==>|^    --|skipped|error'
```
Expected: three targets built, no errors.

- [ ] **Step 4: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "test: build-guard flare on Cortex-M as well as Cortex-A

thumbv7m (Cortex-M3) and thumbv8m.main join armv7a in the bare-metal guards,
both configurations each.

thumbv7m earns its place by having no FPU: every f32 operation is soft-float
and core::simd must scalarise completely, so a hidden hardware-float assumption
would surface there and nowhere else in the matrix.

Labelled as build guards, not test runs, because bare metal has no test
harness. They catch missing no_std support, 64-bit assumptions and SIMD paths
that will not lower; they cannot catch wrong audio, and the runner says so."
```

---

### Task 4: Restructure `tools/test.sh` as an explicit matrix

The runner has grown by accretion across three phases. It now covers six
targets and several feature axes, and a reader cannot tell from it what is
tested where — or, more importantly, what is *not*.

**Files:**
- Modify: `flare/tools/test.sh`

**Interfaces:**
- Consumes: Tasks 1 and 3.
- Produces: the acceptance command for this phase and every later one.

- [ ] **Step 1: Restructure it**

Keep every invocation that exists; reorganise so the file states the matrix
from the "The matrix, and why it is not the cross product" table above, and so
each section says what it uniquely proves. Print a closing summary that
distinguishes what ran from what merely built:

```bash
echo "==> All tests passed."
echo "    ran:   host (all feature combos), QEMU ARM32 (NEON, 32-bit usize),"
echo "           wasm32 (simd128, third ISA for the digests)"
echo "    built: armv7a-none-eabihf, thumbv7m (no FPU), thumbv8m.main"
```

That last distinction is the point: a reader should never mistake a green
bare-metal line for evidence that the audio is right there.

- [ ] **Step 2: Handle a missing wasm runner gracefully**

Follow the ARM bucket's existing pattern — skip loudly, never silently:

```bash
if command -v node >/dev/null || command -v wasmtime >/dev/null; then
  ...run the wasm bucket...
else
  echo "  !! SKIPPED wasm32: no node or wasmtime."
  echo "  !! That is the only target proving the simd128 lowering and the"
  echo "  !! digests' architecture-invariance. Install one rather than"
  echo "  !! treating a green run without it as complete."
fi
```

- [ ] **Step 3: Run the whole thing**

```bash
cd ~/GitHub/flare && tools/test.sh 2>&1 | tail -20
```
Expected: every section runs, the summary prints, and the exit status is 0.

- [ ] **Step 4: Verify the digests one final time**

```bash
cd ~/GitHub/flare
diff <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase4.txt && echo "DIGESTS UNCHANGED"
```

- [ ] **Step 5: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "test: restructure the runner as an explicit portability matrix

The runner grew by accretion across three phases and now covers six targets and
several feature axes; a reader could not tell from it what was tested where, or
what was not.

It now states the matrix and what each row uniquely proves, and its closing
summary separates what RAN from what merely BUILT — so a green bare-metal line
is never mistaken for evidence that the audio is right on that target.

Feature-combination breadth stays on the host where it is cheapest; the other
targets each carry the one axis only they can test. A missing wasm runner skips
loudly, saying what coverage is being lost."
```

---

### Task 5: A CI workflow, ready for a remote

flare has no remote, so a workflow runs nowhere today. It is still worth adding:
it is three lines, it is what publishing would need, and deluge-sdk's CI already
works this way (`.github/workflows/test.yml` just calls `./tools/test.sh`).

**Files:**
- Create: `flare/.github/workflows/test.yml`

**Interfaces:**
- Consumes: Task 4's runner.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Read deluge-sdk's workflow for the house pattern**

```bash
sed -n '1,40p' ~/GitHub/deluge-sdk/.github/workflows/test.yml
```
Match its structure — toolchain setup, target installation, prerequisites, then
`./tools/test.sh` — rather than inventing a different shape.

- [ ] **Step 2: Write flare's**

Install the rustup targets the matrix needs, the QEMU and cross-linker packages
the ARM bucket needs, and node for the wasm bucket; then call `tools/test.sh`.
The workflow must not duplicate the matrix — the runner owns it, so that a
developer running `tools/test.sh` locally gets exactly what CI gets.

Add a comment at the top recording that this is inert until flare has a remote,
so nobody wonders why it has never run.

- [ ] **Step 3: Check it is at least well-formed**

There is no remote to run it, so validate the syntax locally:

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('$HOME/GitHub/flare/.github/workflows/test.yml')); print('valid YAML')"
```

- [ ] **Step 4: Commit**

```bash
cd ~/GitHub/flare && git add -A && git commit -m "ci: add the workflow flare will need when it has a remote

Three lines of setup and a call to tools/test.sh, matching deluge-sdk's
pattern, so the matrix has exactly one definition and a developer running the
runner locally gets what CI would.

Inert until flare has a remote, and the file says so rather than leaving
someone to wonder why it has never run."
```

---

### Task 6: The Phase 4 exit gate, and the record

**Files:**
- Modify: `flare/README.md`, and the spec

**Interfaces:**
- Consumes: everything above.
- Produces: the recorded result.

- [ ] **Step 1: The gate**

```bash
cd ~/GitHub/flare
printf '[1] digests unchanged:      '; diff -q <(grep -E 'const [A-Z0-9_]+_DIGEST: u64' crates/flare-graph/src/golden.rs) /tmp/flare-digests-phase4.txt >/dev/null && echo PASS || echo FAIL
printf '[2] flare runner:           '; tools/test.sh >/dev/null 2>&1 && echo PASS || echo FAIL
printf '[3] deluge-sdk runner:      '; (cd ~/GitHub/deluge-sdk && tools/test.sh >/dev/null 2>&1) && echo PASS || echo FAIL
printf '[4] firmware links:         '; (cd ~/GitHub/deluge-sdk && cargo build --release -p wren-firmware >/dev/null 2>&1) && echo PASS || echo FAIL
printf '[5] goldens on all 3 ISAs:  '
ok=1; for t in "" "--target armv7-unknown-linux-gnueabihf" "--target wasm32-wasip1"; do
  cargo test $t -p flare-graph --features simd golden 2>&1 | grep -q '10 passed' || ok=0
done; [ $ok = 1 ] && echo PASS || echo FAIL
```
Expected: five PASS.

- [ ] **Step 2: Update flare's README**

Replace the Testing section's prose with the matrix table, keeping the
run/build distinction explicit. State plainly that the characterisation digests
hold identically on x86-64, ARM32 and wasm32 — it is the most interesting thing
about the project's testing and currently appears nowhere a reader would look.

- [ ] **Step 3: Record the result in the spec**

Append to the spec's `### Phase 4` subsection: the matrix as built, the
architecture-invariance result, what each bare-metal guard does and does not
prove, the `flare-kernels`-on-wasm limitation and why its cost is low, and the
`thumbv7em` → `thumbv7m` substitution with its reasoning.

- [ ] **Step 4: Commit both repos**

---

## What this plan does NOT cover

- **Publishing flare.** Still a local repo. Task 5's workflow is the piece that
  would be needed; nothing else here assumes a remote.
- **Running tests on Cortex-M.** It has no test harness. Doing it properly means
  a semihosting or `defmt` test runner on real hardware or under QEMU's
  `cortex-m` machine — a project of its own, and one worth doing only if flare
  actually targets a Cortex-M product.
- **`flare-kernels` tests on wasm.** Blocked by proptest's transitive
  `wait-timeout`. Fixable with a target-specific dev-dependency plus
  `#[cfg(not(target_arch = "wasm32"))]` on 26 `proptest!` blocks, but the
  marginal coverage is low: the graph goldens already drive those same kernels
  through the engine under simd on wasm and match the x86-64 digests bit for
  bit, which is a stronger statement than the tolerance-based equivalence tests
  would add.
- **`wasm32-unknown-unknown` test execution.** The library builds for it, and
  the matrix runs `wasm32-wasip1` instead because the test harness needs WASI to
  report results and set an exit code. The SIMD lowering being exercised is the
  same.
- **Benchmarks on any target.** Correctness only. `flare-fft` has criterion
  benches; wiring them into the matrix is a separate question.
