# RZ/A1 Phase 6 Prep: Standalone `rza1/` Workspace and CI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Meet Phase 6's CI condition: "the `rza1/` crates have their own CI job that builds and tests them without `deluge-*` crates". To do that, move them into a standalone `rza1/` Cargo workspace with their own tooling. Also repair the root CI, which fails before running anything because `../flare` isn't checked out.

**Architecture:**
- `rza1-pac`, `rza1-rt`, `rza1l-hal` and `rza1-alloc` move from `crates/` to `rza1/` (`git mv`, history kept).
- `rza1/` gets its own `[workspace]`, `Cargo.lock`, `.cargo/config.toml`, `rust-toolchain.toml`, PAC tooling and `tools/test.sh`.
- The root workspace excludes `rza1/` and path-depends on its crates, which is the tree `git subtree split -P rza1` will take.
- A new CI job runs `rza1/tools/test.sh` from a checkout with no `../flare` and no vendor manual. That proves the crates need nothing from the Deluge side.
- The root jobs get `flare` cloned next to the checkout.

**Tech Stack:** Cargo workspaces, GitHub Actions, bash, python3 (chiptool PAC tooling), QEMU ARM host tests.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 6 conditions, layering rules §2).

## Global Constraints

- Phase 6 conditions (spec):
  - "Phases 1–5 are done." (They are.)
  - "No `rza1l-hal` change in the last ~2 months was forced by a Deluge-specific need." This stays a judgment call at split time (user decision), and this plan does not touch it.
  - "The `rza1/` crates have their own CI job that builds and tests them without `deluge-*` crates." This plan meets it.
- Layering rule 1: nothing under `rza1/` names `deluge` in code or depends on a `deluge-*` crate. `tools/check-layering.sh` keeps enforcing it from the root.
- No behaviour change:
  - `tools/link-layout.sh check` must match after every task;
  - the full suite (`tools/test.sh`) must pass;
  - every firmware and example still builds.
- Vendor material:
  - Only `rza1/vendor/rza1.svd` is tracked in git (user decision).
  - The RZ/A1L manual (`vendor/docs/rza1/…`, 50 MB) and the Renesas BSP sources (`vendor/rbsp`) move to `rza1/vendor/` but stay gitignored.
  - Anything that reads the manual skips with a notice when it is absent.
- `flare`: the user creates the GitHub repository `FirestormAudio/flare` and pushes its `main` (user decision). This is a prerequisite for Task 4's CI to pass, not something the executor does. CI pins `ce4d6826192e9e34e2d189ddce94777d06e352c2`, flare's current `main`.
- Firmware builds: `cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p <pkg>`.
- Never push and never publish. The plan's CI changes are verified locally (YAML parse plus a fresh-clone dry run); their first real run is the user's push.

## Review Focus

1. **The standalone workspace silently resolves a `deluge-*` crate or the sibling `../flare`**, e.g. through a stray path dependency or a root `.cargo/config.toml` `[patch]` inherited from the parent directory. The CI condition would then be met in name only. Pinned by Task 3's fresh-clone run with no `../flare` beside it, and by `rza1/tools/test.sh`'s dependency-closure check.
2. **Root and `rza1/` resolve different versions of a shared crate** (`embassy-time`, `critical-section`, `embassy-hal-internal`), so the firmware links two copies. Example: two `peripherals!` singleton flags, or two time drivers. Pinned by Task 1's `cargo tree -d` check over the root workspace (no duplicate embassy or critical-section crates) and by the unchanged link layout.
3. **A tool or doc still points at `crates/rza1*`** and fails or misleads only later: `check-layering.sh`'s chip-crate rule silently checking nothing, `gen-pac.sh` writing to a dead path, or a firmware manifest path. Pinned by Task 1 Step 6's grep, and by Task 2's layering probe (the rule must still fire on a `deluge` name planted under `rza1/`).
4. **CI's PAC drift check passes vacuously without the manual.** In `--check` mode `gen-pac.sh` must still regenerate both chips from the tracked SVD and the checked-in transforms and compare them. Only the manual-derived steps may be skipped. Pinned by Task 2's probe: a hand edit to `rza1/rza1-pac/src/chips/rza1l.rs` makes the manual-less check fail.
5. **DelugeFirmware#next stops resolving the SDK** once it bumps past this change. Its git dependency must find `rza1l-hal` and the rest inside the nested `rza1/` workspace, and its gitignored `local-sdk.toml` must point at the new paths. Pinned by Task 5's DelugeFirmware build through the updated `local-sdk.toml`.

---

## File Structure

| Path | Responsibility |
|---|---|
| `rza1/Cargo.toml` (new) | The standalone workspace: members, `[workspace.package]`, `[workspace.dependencies]` for the chip crates |
| `rza1/.cargo/config.toml`, `rza1/rust-toolchain.toml` (new) | Targets, QEMU runner and rustflags, the toolchain, as the root has them |
| `rza1/{rza1-pac,rza1-rt,rza1l-hal,rza1-alloc}/` (moved from `crates/`) | The chip crates, unchanged apart from paths |
| `rza1/tools/` (moved from `tools/`) | `gen-pac.sh`, `check-pac.sh`, `pac-*.py`, `pac_manual.py`, `test_pac_manual.py`, `check-register-tables.sh`, and a new `test.sh` |
| `rza1/vendor/rza1.svd` (tracked) | The PAC's source; the rest of `rza1/vendor/` stays gitignored |
| `.github/workflows/rza1.yml` (new), `ci.yml` | The standalone chip-crate job |
| `.github/workflows/{test,examples,images}.yml` | Clone `flare` next to the checkout |
| Root `Cargo.toml`, `tools/test.sh`, `tools/check-layering.sh`, manifests, docs | Point at `rza1/` |

---

### Task 1: The `rza1/` workspace

**Files:**
- Move: `crates/rza1-pac` → `rza1/rza1-pac`, `crates/rza1-rt` → `rza1/rza1-rt`, `crates/rza1l-hal` → `rza1/rza1l-hal`, `crates/rza1-alloc` → `rza1/rza1-alloc`
- Create: `rza1/Cargo.toml`, `rza1/.cargo/config.toml`, `rza1/rust-toolchain.toml`
- Modify:
  - root `Cargo.toml`: `members` (lines 10–13) and a new `exclude`;
  - `crates/deluge-bsp/Cargo.toml:62-63` and `crates/deluge-sdk/Cargo.toml:27-28`;
  - the firmware, hw-test and wren-sys manifests listed in Step 4;
  - `tools/test.sh:47-49`.

**Interfaces:**
- Produces:
  - the workspace `rza1/Cargo.toml` with members `rza1-pac`, `rza1-rt`, `rza1l-hal`, `rza1-alloc`;
  - crate paths `rza1/<crate>`;
  - the root `Cargo.toml` excludes `rza1`;
  - chip-crate tests run with `--manifest-path rza1/Cargo.toml`.

- [ ] **Step 1: Branch, and write the failing check**

```bash
git checkout -b chore/rza1-standalone
cargo metadata --format-version 1 --no-deps --manifest-path rza1/Cargo.toml > /dev/null 2>&1; echo "exit $?"
```

Expected: `exit 101` (no `rza1/` workspace yet).

- [ ] **Step 2: Move the crates**

```bash
mkdir -p rza1
for c in rza1-pac rza1-rt rza1l-hal rza1-alloc; do git mv "crates/$c" "rza1/$c"; done
```

Inside the moved crates, the only cross-crate path is `rza1l-hal`'s `rza1-pac = { path = "../rza1-pac", … }`, which stays correct (both moved together). Confirm:

Run: `grep -rn 'path = "\.\./' rza1/*/Cargo.toml`
Expected: only `rza1/rza1l-hal/Cargo.toml: rza1-pac = { path = "../rza1-pac", …`.

- [ ] **Step 3: The workspace files**

`rza1/Cargo.toml`:

```toml
# The RZ/A1 chip crates: PAC, runtime, HAL and heap allocators. A workspace of their own, outside
# deluge-sdk's, so they build and test with no `deluge-*` crate in reach. This directory is what
# Phase 6 splits into the rza1-rs repository.
[workspace]
resolver = "3"
members = ["rza1-pac", "rza1-rt", "rza1l-hal", "rza1-alloc"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"
authors = ["Katherine Whitlock <kate@skylinesynths.nyc>"]
repository = "https://github.com/FirestormAudio/deluge-sdk"
homepage = "https://github.com/FirestormAudio/deluge-sdk"

# Must stay version-compatible with deluge-sdk's root workspace, which resolves these crates for
# the firmware: one copy each in the final link (Task 1 Step 7 checks).
[workspace.dependencies]
cortex-ar = "0.3"
critical-section = "1"
embassy-futures = "0.1.2"
embassy-hal-internal = "0.5.0"
embassy-sync = "0.8.0"
embassy-time = "0.5.1"
embassy-time-driver = "0.2.2"
embassy-time-queue-utils = "0.3.2"
embassy-usb-driver = "0.2.2"
log = { version = "0.4", features = ["release_max_level_info"] }
rtt-target = { version = "0.6", features = ["log"] }
```

Before writing the dependency table, compare it with every `workspace = true` key the four manifests use. Add any key the grep shows that the table lacks, with the root `Cargo.toml`'s version string:

```bash
grep -ho '^[a-z0-9_-]* = { workspace = true' rza1/*/Cargo.toml | cut -d' ' -f1 | sort -u
```

Copy `resolver` from the root `Cargo.toml` if it sets one other than `"3"`.

`rza1/rust-toolchain.toml`: a copy of the root `rust-toolchain.toml`.

`rza1/.cargo/config.toml`:

```toml
# The chip crates' build settings, as deluge-sdk's root .cargo/config.toml has them for these
# targets. Cargo merges this with any config in parent directories; this file is what remains
# after the Phase 6 split.
[build]
target = "armv7a-none-eabihf"

# ARM Linux under QEMU, for the host-side tests (the MMIO seam).
[target.armv7-unknown-linux-gnueabihf]
linker = "arm-linux-gnueabihf-gcc"
runner = ["qemu-arm", "-cpu", "cortex-a9", "-L", "/usr/arm-linux-gnueabihf"]
rustflags = [
  "-C", "target-cpu=cortex-a9",
  "-C", "target-feature=+neon",
]

[target.armv7a-none-eabihf]
rustflags = [
  "-C", "target-cpu=cortex-a9",
  "-C", "target-feature=+neon",
  "-C", "link-arg=--gc-sections",
  "-C", "force-frame-pointers=yes",
]
```

(Cross-check the two target tables against the root `.cargo/config.toml` and copy them verbatim if they have changed since this plan.)

- [ ] **Step 4: Point the root at `rza1/`**

Root `Cargo.toml`: delete the four `"crates/rza1…",` member lines and add, after the `members` array, `exclude` entries (merge into the existing `exclude = [ … ]` list if there is one):

```toml
  # The chip crates are their own workspace (rza1/Cargo.toml); path dependencies below reach them.
  "rza1",
```

Then rewrite every path:

```bash
sed -i 's|path = "\.\./rza1l-hal"|path = "../../rza1/rza1l-hal"|; s|path = "\.\./rza1-rt"|path = "../../rza1/rza1-rt"|; s|path = "\.\./rza1-alloc"|path = "../../rza1/rza1-alloc"|' \
  crates/deluge-bsp/Cargo.toml crates/deluge-sdk/Cargo.toml
sed -i 's|path = "\.\./\.\./crates/rza1l-hal"|path = "../../rza1/rza1l-hal"|; s|path = "\.\./\.\./crates/rza1-alloc"|path = "../../rza1/rza1-alloc"|' \
  firmwares/*/Cargo.toml hw-tests/*/Cargo.toml wren/wren-sys/Cargo.toml
grep -rn "crates/rza1\|\"\.\./rza1" --include=Cargo.toml . | grep -v "^./target"
```

Expected: the final grep prints nothing.

`tools/test.sh:47-49` (the three chip-crate `cargo test` lines): replace them with

```bash
# The chip crates are their own workspace; their suite (tests, PAC checks, register-table check)
# runs from rza1/tools/test.sh, which CI also runs on its own.
rza1/tools/test.sh
```

(Task 2 creates `rza1/tools/test.sh`; until then, run the three commands by hand with `--manifest-path rza1/Cargo.toml` in Step 7.)

- [ ] **Step 5: The chip crates build and test on their own**

```bash
Q=armv7-unknown-linux-gnueabihf
cargo test --manifest-path rza1/Cargo.toml --target $Q -p rza1-pac --features rza1l --lib 2>&1 | grep "test result"
cargo test --manifest-path rza1/Cargo.toml --target $Q -p rza1-pac --features rza1h --lib 2>&1 | grep "test result"
cargo test --manifest-path rza1/Cargo.toml --target $Q -p rza1l-hal --lib 2>&1 | grep "test result"
cargo build --manifest-path rza1/Cargo.toml -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem \
  -p rza1l-hal -p rza1-rt -p rza1-alloc 2>&1 | tail -1
ls rza1/Cargo.lock
```

Expected: three `test result: ok.` lines (the same pass counts as `tools/test.sh` printed for them before the move); `Finished`; `rza1/Cargo.lock` exists.

- [ ] **Step 6: Nothing still points at `crates/rza1*`**

```bash
grep -rn "crates/rza1" --exclude-dir=target --exclude-dir=.git --exclude-dir=superpowers . | grep -v "CHANGELOG.md"
```

Expected: hits only in files Task 2 and Task 5 handle: `tools/check-layering.sh`, `tools/check-register-tables.sh`, `tools/gen-pac.sh`, `tools/pac-a1l-absent.py`, `README.md` and `docs/*.md`. Anything else gets fixed now (rewrite `crates/rza1` to `rza1/rza1`).

- [ ] **Step 7: One copy of each shared crate, unchanged layout, green suite**

```bash
cargo tree -d -e normal --target armv7a-none-eabihf 2>/dev/null | grep -E "^(embassy|critical-section|cortex-ar)" ; echo "dups listed above (expect none)"
tools/link-layout.sh check
tools/test.sh > /tmp/claude-1000/r6-t1.log 2>&1; echo "exit $?"
```

Expected:
- No `embassy*`/`critical-section`/`cortex-ar` duplicate lines (if there are any, align the `rza1/Cargo.toml` versions with the root's and re-run);
- `==> Link layout matches tools/link-layout.baseline.`;
- `exit 0`, but only once Task 2 has created `rza1/tools/test.sh`. Until then `tools/test.sh` fails at that line, which is expected, so run Steps 5's commands as the gate and finish Task 1 with the commit below.

- [ ] **Step 8: Commit**

```bash
git add -A rza1 crates Cargo.toml Cargo.lock firmwares hw-tests wren/wren-sys/Cargo.toml tools/test.sh
git commit -m "build: the RZ/A1 chip crates are their own workspace in rza1/"
```

---

### Task 2: The chip crates' tooling moves into `rza1/`

**Files:**
- Move:
  - `tools/{gen-pac.sh,check-pac.sh,pac-a1l-absent.py,pac-check-a1l.py,pac_manual.py,pac-postprocess.py,test_pac_manual.py,check-register-tables.sh}` → `rza1/tools/`;
  - `vendor/rza1.svd` → `rza1/vendor/rza1.svd` (tracked);
  - `vendor/docs/rza1` → `rza1/vendor/docs/rza1` (stays ignored);
  - `vendor/rbsp` → `rza1/vendor/rbsp` (stays ignored).
- Create: `rza1/tools/test.sh`, `rza1/.gitignore`
- Modify: the moved tools' paths, `tools/test.sh:31-32`, `tools/check-layering.sh` (CHIP_DIRS, the `global_asm` path, the Python RULES prefixes)

**Interfaces:**
- Consumes: Task 1's layout.
- Produces:
  - `rza1/tools/test.sh`, which runs the chip crates' whole suite;
  - `rza1/tools/gen-pac.sh --check`, which works without the manual (manual steps skipped with a notice);
  - `rza1/vendor/rza1.svd`, tracked.

- [ ] **Step 1: Write the failing checks**

```bash
test -x rza1/tools/test.sh; echo "test.sh: $?"
git ls-files rza1/vendor/rza1.svd | grep -c . ; echo "(tracked svd count)"
```

Expected: `test.sh: 1` and `0`.

- [ ] **Step 2: Move the tools and the vendor inputs**

```bash
mkdir -p rza1/tools rza1/vendor/docs
for f in gen-pac.sh check-pac.sh pac-a1l-absent.py pac-check-a1l.py pac_manual.py pac-postprocess.py \
         test_pac_manual.py check-register-tables.sh; do git mv "tools/$f" "rza1/tools/$f"; done
mv vendor/rza1.svd rza1/vendor/rza1.svd
mv vendor/docs/rza1 rza1/vendor/docs/rza1
mv vendor/rbsp rza1/vendor/rbsp
```

`rza1/.gitignore`:

```
# Build output.
/target/
# Vendor material stays local, apart from the SVD the PAC is generated from.
/vendor/*
!/vendor/rza1.svd
__pycache__/
```

The root `.gitignore`'s `vendor/` pattern matches `rza1/vendor/` too. Make sure the SVD is addable:

Run: `git check-ignore -v rza1/vendor/rza1.svd; git add rza1/vendor/rza1.svd rza1/.gitignore && git ls-files rza1/vendor`
Expected: `rza1/vendor/rza1.svd` is listed. If `git check-ignore` blames the root `.gitignore`'s `vendor/`, change that line to `/vendor/` (root-anchored), then re-add.

- [ ] **Step 3: Fix the moved tools' paths**

Each moved script `cd`s to `"$(dirname "$0")/.."`, which is now `rza1/`. So relative paths inside them change as follows:

```bash
sed -i 's|^PAC=crates/rza1-pac$|PAC=rza1-pac|' rza1/tools/gen-pac.sh
sed -i 's|crates/rza1l-hal/src|rza1l-hal/src|g' rza1/tools/check-register-tables.sh
sed -i 's|crates/rza1-pac/transforms|rza1-pac/transforms|' rza1/tools/pac-a1l-absent.py
grep -rn "crates/\|tools/" rza1/tools/*.sh rza1/tools/*.py | grep -v "rza1/tools\|^rza1/tools/[a-z_-]*\.\(sh\|py\):[0-9]*:\s*#"
```

Expected: the final grep prints only `tools/…` references that are correct relative to `rza1/` (e.g. `python3 tools/pac-postprocess.py`, `tools/gen-pac.sh`). `vendor/rza1.svd` and `vendor/docs/rza1/…` are already right relative to `rza1/`. The chiptool cache path `target/chiptool-…` now resolves to `rza1/target/…`, which is fine.

- [ ] **Step 4: `gen-pac.sh --check` without the manual**

In `rza1/tools/gen-pac.sh`, replace the block from `python3 tools/pac-a1l-absent.py "$SVD" "$MANUAL" "$OUT/absent.yaml" > /dev/null` through `emit "$OUT/absent.yaml" "$PAC/transforms/rza1l/absent.yaml"` with:

```bash
# The RZ/A1L manual is vendor material kept out of git, so CI has only the SVD. Without it the
# absent-register list (generated from the manual) can't be regenerated or checked against the
# table; the chips are still regenerated from the SVD and the checked-in transforms and compared.
HAVE_MANUAL=0
[ -f "$MANUAL" ] && HAVE_MANUAL=1
if [ "$HAVE_MANUAL" -eq 1 ]; then
  python3 tools/pac-a1l-absent.py "$SVD" "$MANUAL" "$OUT/absent.yaml" > /dev/null
  emit "$OUT/absent.yaml" "$PAC/transforms/rza1l/absent.yaml"
else
  echo "  (no $MANUAL: skipping the absent-register list and the manual check)"
fi
```

and replace the line `python3 tools/pac-check-a1l.py "$OUT/rza1l/ir.yaml" "$MANUAL"` with:

```bash
if [ "$HAVE_MANUAL" -eq 1 ]; then
  python3 tools/pac-check-a1l.py "$OUT/rza1l/ir.yaml" "$MANUAL"
fi
```

In `rza1/tools/test_pac_manual.py`, decorate the test class that reads the real table (`class TheRealTable(unittest.TestCase):`) with

```python
@unittest.skipUnless(os.path.exists(MANUAL), "the RZ/A1L manual is not in this checkout (vendor material)")
```

and, because its class attribute `regs = a1l_registers(MANUAL)` runs at import, change that line to a `setUpClass`:

```python
    @classmethod
    def setUpClass(cls):
        cls.regs = a1l_registers(MANUAL)
```

`rza1/tools/check-pac.sh` needs no edit: its `python3 -m unittest discover -s tools …` and its `target/chiptool-…` check are relative to the directory it `cd`s to, which is now `rza1/`. Step 6 runs it.

- [ ] **Step 5: `rza1/tools/test.sh`**

```bash
#!/usr/bin/env bash
#
# The RZ/A1 chip crates' whole suite: they build and test with nothing from deluge-sdk in reach.
# deluge-sdk's tools/test.sh runs this; CI also runs it on its own, from a checkout with no
# ../flare and no vendor manual.
#
# Usage: rza1/tools/test.sh
set -euo pipefail

cd "$(dirname "$0")/.."

QEMU=armv7-unknown-linux-gnueabihf

echo "==> rza1: no deluge-* crate in the dependency graph"
cargo metadata --format-version 1 --all-features | python3 -c '
import json, sys
names = sorted({p["name"] for p in json.load(sys.stdin)["packages"]})
bad = [n for n in names if n.startswith("deluge") or n.startswith("flare")]
if bad:
    sys.exit("error: the rza1 workspace resolves " + ", ".join(bad))
'

tools/check-register-tables.sh
tools/check-pac.sh

echo "==> rza1: QEMU ARM tests"
cargo test --target "$QEMU" -p rza1-pac --features rza1l --lib
cargo test --target "$QEMU" -p rza1-pac --features rza1h --lib
cargo test --target "$QEMU" -p rza1l-hal --lib

echo "==> rza1: device build"
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem \
  -p rza1l-hal -p rza1-rt -p rza1-alloc

echo "==> rza1: all checks passed."
```

```bash
chmod +x rza1/tools/test.sh
```

Root `tools/test.sh:31-32`: delete `tools/check-register-tables.sh` and `tools/check-pac.sh` (now inside `rza1/tools/test.sh`, which Task 1 Step 4 put in the root script).

`tools/check-layering.sh`:
- `CHIP_DIRS=(crates/rza1-pac crates/rza1l-hal crates/rza1-rt crates/rza1-alloc)` becomes `CHIP_DIRS=(rza1)`.
- `grep -rn --include='*.rs' 'global_asm' crates/rza1l-hal` becomes `… 'global_asm' rza1/rza1l-hal`.
- In the Python `RULES`, replace the four `("crates/rza1….", …)` entries with one:

```python
    ("rza1/", lambda n: n.startswith("deluge-") or n.startswith("flare"),
     "an rza1 chip crate depends on a deluge-* or flare crate; board facts belong in deluge-bsp"),
```

The `RULES` loop walks `meta["workspace_members"]` of the root workspace, which no longer contains the chip crates. So make the dependency check also load the `rza1` workspace: change the `cargo metadata … | python3 -c '…'` invocation to run once for the root and once with `--manifest-path rza1/Cargo.toml`. Each run reports its own `workspace_root`, and the `rel.startswith(prefix)` match uses the path relative to the repo root. In the script, compute `rel` from the repo root (`os.getcwd()` at the script's `cd`) instead of `meta["workspace_root"]`.

- [ ] **Step 6: The checks pass, and still catch what they should**

```bash
rza1/tools/test.sh > /tmp/claude-1000/r6-t2.log 2>&1; echo "exit $?"; grep -E "test result|==>|skipping|error" /tmp/claude-1000/r6-t2.log
tools/check-layering.sh
```

Expected: `exit 0`, three `test result: ok.` lines, `==> rza1: all checks passed.`, `==> Layering check passed.`

Probes (each must fail, then be undone):

```bash
echo 'const BOARD: &str = "deluge";' >> rza1/rza1-rt/src/lib.rs; tools/check-layering.sh > /dev/null 2>&1; echo "layering probe exit $?"; git checkout rza1/rza1-rt/src/lib.rs
mv rza1/vendor/docs/rza1 /tmp/claude-1000/rza1-manual-aside
printf '\n// drift\n' >> rza1/rza1-pac/src/chips/rza1l.rs
(cd rza1 && tools/gen-pac.sh --check > /dev/null 2>&1); echo "drift-without-manual probe exit $?"
git checkout rza1/rza1-pac/src/chips/rza1l.rs
(cd rza1 && tools/gen-pac.sh --check 2>&1 | tail -2)
mv /tmp/claude-1000/rza1-manual-aside rza1/vendor/docs/rza1
```

Expected:
- `layering probe exit 1`;
- `drift-without-manual probe exit 1`;
- the clean manual-less check prints the `(no … skipping …)` notice and `==> rza1-pac is up to date.`

(The drift probe needs the pinned chiptool; if `rza1/target/chiptool-*` is absent, run `rza1/tools/gen-pac.sh` once first, which installs it.)

- [ ] **Step 7: Root suite and commit**

Run: `tools/test.sh > /tmp/claude-1000/r6-t2-root.log 2>&1; echo "exit $?"`
Expected: `exit 0`.

```bash
git add -A rza1 tools .gitignore
git commit -m "build: the chip crates' PAC tooling, register-table check and test runner live in rza1/; the SVD is tracked"
```

---

### Task 3: The standalone CI job

**Files:**
- Create: `.github/workflows/rza1.yml`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `rza1/tools/test.sh` (Task 2).

- [ ] **Step 1: Write the failing check**

Run: `test -f .github/workflows/rza1.yml; echo $?`
Expected: `1`.

- [ ] **Step 2: The workflow**

`.github/workflows/rza1.yml`:

```yaml
name: rza1

# The RZ/A1 chip crates on their own: no ../flare, no deluge-* crate, no vendor manual. Phase 6's
# split condition ("their own CI job that builds and tests them without deluge-* crates").
on:
  workflow_call:
  workflow_dispatch:

jobs:
  test:
    name: Chip crates (standalone)
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: rza1
    steps:
      - uses: actions/checkout@v7

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: nightly
          targets: armv7a-none-eabihf, armv7-unknown-linux-gnueabihf
          components: rust-src,llvm-tools-preview

      - name: Cache Cargo dependencies
        uses: Swatinem/rust-cache@v2
        with:
          workspaces: rza1

      - name: Install QEMU + ARM cross toolchain
        run: |
          sudo apt-get update
          sudo apt-get install -y qemu-user gcc-arm-linux-gnueabihf python3-yaml

      # rza1/tools/gen-pac.sh's pinned chiptool (check-pac.sh installs it when missing).
      - name: Cache chiptool
        uses: actions/cache@v4
        with:
          path: rza1/target/chiptool-bcf538a2e7b8584ae874ee9ab72efb1576fc6152
          key: chiptool-bcf538a2e7b8584ae874ee9ab72efb1576fc6152

      - name: Run the chip crates' suite
        run: ./tools/test.sh
```

(Copy the action versions, `actions/checkout@v7` and the others, from `test.yml` so all workflows agree. If `rza1/tools/gen-pac.sh`'s `CHIPTOOL_REV` has changed, use it in the cache path and key.)

In `.github/workflows/ci.yml` add, after the `tests:` job:

```yaml
  rza1:
    name: rza1
    uses: ./.github/workflows/rza1.yml
```

In `.github/workflows/test.yml`, change the chiptool cache path from `target/chiptool-…` to `rza1/target/chiptool-…` (root `tools/test.sh` now runs the PAC check from `rza1/`).

- [ ] **Step 3: The workflows parse**

Run: `for f in .github/workflows/*.yml; do python3 -c "import sys,yaml; yaml.safe_load(open(sys.argv[1]))" "$f" && echo "ok $f"; done; command -v actionlint && actionlint`
Expected: `ok` for every file; if `actionlint` is installed, no findings.

- [ ] **Step 4: Dry-run the job's conditions locally**

Reproduce CI's view: a fresh clone of the branch, with no `../flare` beside it and no vendor manual (the clone has only tracked files).

```bash
S=/tmp/claude-1000/r6-ci; rm -rf "$S"; mkdir -p "$S"
git clone -q --branch chore/rza1-standalone "$PWD" "$S/deluge-sdk"
ls "$S"                                   # only deluge-sdk: no flare sibling
ls "$S/deluge-sdk/rza1/vendor"            # only rza1.svd
(cd "$S/deluge-sdk/rza1" && CI=1 ./tools/test.sh > "$S/run.log" 2>&1; echo "exit $?")
grep -E "test result|==>|skipping" "$S/run.log"
```

Expected:
- `ls "$S"` shows only `deluge-sdk`, and the vendor directory holds only `rza1.svd`;
- `exit 0`;
- three `test result: ok.` lines, the manual-skip notice, `==> rza1-pac is up to date.` and `==> rza1: all checks passed.`

(`CI=1` makes `check-pac.sh` install chiptool and run the drift check, as on GitHub.)

- [ ] **Step 5: Commit**

```bash
git add .github/workflows
git commit -m "ci: a standalone job builds and tests the rza1 chip crates with no deluge-* crate or flare in reach"
```

---

### Task 4: Root CI clones `flare`

**Files:**
- Modify: `.github/workflows/test.yml`, `.github/workflows/examples.yml`, `.github/workflows/images.yml`

**Prerequisite (the user's):** the repository `FirestormAudio/flare` exists on GitHub with commit `ce4d6826192e9e34e2d189ddce94777d06e352c2` pushed. If it is private, a repository secret `FLARE_TOKEN` holds a token with read access to it. The executor does not create or push it.

- [ ] **Step 1: Write the failing check**

Run: `grep -c "FirestormAudio/flare" .github/workflows/test.yml .github/workflows/examples.yml .github/workflows/images.yml`
Expected: `0` for each.

- [ ] **Step 2: The clone step**

In each of the three workflows, directly after the job's first `- uses: actions/checkout@v7` step, add:

```yaml
      # deluge-sdk's workspace path-depends on a sibling ../flare checkout (the audio engine).
      # actions/checkout can't write outside the workspace, so check it out inside and link it.
      - name: Checkout flare
        uses: actions/checkout@v7
        with:
          repository: FirestormAudio/flare
          ref: ce4d6826192e9e34e2d189ddce94777d06e352c2
          path: flare
          token: ${{ secrets.FLARE_TOKEN || github.token }}
      - name: Place flare beside the checkout
        run: ln -s "$GITHUB_WORKSPACE/flare" "$GITHUB_WORKSPACE/../flare"
```

(`images.yml` may define several jobs or a matrix; add the two steps to every job that builds the workspace.)

- [ ] **Step 3: Parse, and dry-run the layout**

Run: `for f in .github/workflows/*.yml; do python3 -c "import sys,yaml; yaml.safe_load(open(sys.argv[1]))" "$f" && echo "ok $f"; done`
Expected: `ok` for every file.

Reproduce the CI layout with the local flare standing in for the GitHub clone:

```bash
S=/tmp/claude-1000/r6-root; rm -rf "$S"; mkdir -p "$S/ws"
git clone -q --branch chore/rza1-standalone "$PWD" "$S/ws/deluge-sdk"
git clone -q /home/kate/GitHub/flare "$S/ws/deluge-sdk/flare"
git -C "$S/ws/deluge-sdk/flare" checkout -q ce4d6826192e9e34e2d189ddce94777d06e352c2
ln -s "$S/ws/deluge-sdk/flare" "$S/ws/flare"
(cd "$S/ws/deluge-sdk" && cargo metadata --format-version 1 --no-deps > /dev/null && echo "root workspace resolves")
```

Expected: `root workspace resolves`. Note: the clone inside the checkout (`deluge-sdk/flare`) must not be picked up as a workspace member. The root `Cargo.toml` lists members explicitly, so this holds unless a glob member matches `flare/*`. If it does, add `"flare"` to `exclude` and rerun.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows
git commit -m "ci: clone flare beside the checkout so the root jobs can resolve the workspace"
```

---

### Task 5: Paths in docs, DelugeFirmware's local override, and the final check

**Files:**
- Modify: `README.md:189-191` (layout table), `docs/getting-started.md:478`, `docs/advanced-guide.md:126,176,275`, `rza1/rza1-pac/README.md`, `rza1/*/README.md` (relative links), `CHANGELOG.md`, `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md` (Phase 6)
- Modify (outside this repo, gitignored): `~/GitHub/DelugeFirmware/src/bsp/rust/local-sdk.toml`

- [ ] **Step 1: Write the failing check**

Run: `grep -rn "crates/rza1" --exclude-dir=target --exclude-dir=.git --exclude-dir=superpowers . | grep -v "CHANGELOG.md" | wc -l`
Expected: a non-zero count (the doc links).

- [ ] **Step 2: Fix the links**

```bash
grep -rl "crates/rza1" --exclude-dir=target --exclude-dir=.git --exclude-dir=superpowers . | grep -v CHANGELOG.md \
  | xargs sed -i 's|crates/rza1|rza1/rza1|g'
grep -rn "\.\./\.\./vendor\|\.\./vendor" rza1/*/README.md rza1/*/src/lib.rs | head
```

Then fix by hand any link that was relative to `crates/` and now has the wrong depth. Two examples:
- `README.md` rows that become `rza1/rza1l-hal`;
- a README under `rza1/<crate>/` that links `../../vendor/…`, which must now be `../vendor/…`.

Run the grep from Step 1 again. Expected: `0`.

- [ ] **Step 3: DelugeFirmware's local override**

`~/GitHub/DelugeFirmware/src/bsp/rust/local-sdk.toml` (gitignored, local to this machine): rewrite each `deluge-sdk/crates/rza1…` path:

```bash
sed -i 's|deluge-sdk/crates/rza1l-hal|deluge-sdk/rza1/rza1l-hal|; s|deluge-sdk/crates/rza1-alloc|deluge-sdk/rza1/rza1-alloc|; s|deluge-sdk/crates/rza1-rt|deluge-sdk/rza1/rza1-rt|; s|deluge-sdk/crates/rza1-pac|deluge-sdk/rza1/rza1-pac|' \
  ~/GitHub/DelugeFirmware/src/bsp/rust/local-sdk.toml
grep -n "rza1" ~/GitHub/DelugeFirmware/src/bsp/rust/local-sdk.toml
(cd ~/GitHub/DelugeFirmware && ./dbt build > /tmp/claude-1000/r6-dfw.log 2>&1; echo "exit $?"; git checkout -q src/bsp/rust/Cargo.lock)
```

Expected: the patch lines name `rza1/…` paths; `exit 0` (DelugeFirmware builds against this checkout through the override, the nested `rza1/` workspace included). `git status` in DelugeFirmware shows no tracked change after the lockfile restore.

- [ ] **Step 4: CHANGELOG and spec**

`CHANGELOG.md`, under `## [Unreleased]` → `### Changed`, at the top:

```markdown
- The RZ/A1 chip crates (`rza1-pac`, `rza1-rt`, `rza1l-hal`, `rza1-alloc`) moved from `crates/` to
  `rza1/`, their own Cargo workspace with its own tooling (`rza1/tools/test.sh`) and CI job. A
  path dependency on them changes from `crates/<crate>` to `rza1/<crate>`; git dependencies are
  unaffected.
```

Spec, after the Phase 6 conditions list:

```markdown
**Conditions, 2026-10-15:** Phases 1–5 are done. The chip crates are a standalone workspace in
`rza1/` with their own CI job (`.github/workflows/rza1.yml`), which runs from a checkout with no
`../flare` and no vendor manual and fails if the workspace resolves any `deluge-*` or `flare`
crate. The stability condition stays a judgment at split time. `git subtree split -P rza1` now
takes exactly the chip crates, their tooling and the SVD.
```

- [ ] **Step 5: Final verification**

```bash
tools/test.sh > /tmp/claude-1000/r6-final.log 2>&1; echo "exit $?"; grep -E "test result" /tmp/claude-1000/r6-final.log | awk '{p+=$4; f+=$6} END {print "passed",p,"failed",f}'
tools/link-layout.sh check
tools/check-manifests.sh; echo "manifests exit $?"
cargo build -Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -p uac-host-firmware 2>&1 | tail -1
```

Expected:
- `exit 0`;
- `failed 0`, with the passed total the same as before Task 1 (the chip-crate tests now run inside `rza1/tools/test.sh`);
- the layout matches;
- `manifests exit 0`;
- `Finished`.

- [ ] **Step 6: Commit**

```bash
git add -A README.md docs CHANGELOG.md rza1
git commit -m "docs: the chip crates live in rza1/; Phase 6 condition status"
```

- [ ] **Step 7: What the user does next**

List in the final message:
1. Create `FirestormAudio/flare`, push `main` (at least `ce4d6826`), and add `FLARE_TOKEN` if it is private.
2. Push `main` once this branch is merged; the first run of `CI` shows the new `rza1` job and the repaired root jobs.
3. Phase 6's remaining condition, the stability window, is a judgment at split time.
