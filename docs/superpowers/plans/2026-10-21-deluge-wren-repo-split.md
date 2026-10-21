# deluge-wren Repo Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move deluge-sdk's `wren/` subsystem into a new local repo `~/GitHub/deluge-wren` that consumes deluge-sdk only through the `deluge` facade, and leave deluge-sdk with no Wren code.

**Architecture:** Check the firmware on hardware, then close the one layering leak (`wren-sys` → `rza1-alloc`) through the facade's existing `deluge::deluge_alloc` re-export, while deluge-sdk's layering check can still see `wren/`. Copy the tracked `wren/` tree into a fresh repo with a `crates/` layout that keeps every relative path depth (`../../../wren-rs`, `../flare`). It gets its own workspace, config, tests and layering check. Then strip Wren from deluge-sdk and check the device again from the new repo.

**Tech Stack:** Rust nightly (`armv7a-none-eabihf`, `-Zbuild-std`), Cargo workspaces, bash tooling, QEMU `armv7-unknown-linux-gnueabihf`, wasm32-wasip1, J-Link on the pi-top bench.

**Spec:** `docs/superpowers/specs/2026-10-21-deluge-wren-repo-split-design.md`

## Global Constraints

- History: deluge-wren starts with **one fresh initial commit**. No `git filter-repo`.
- Dependencies: deluge-wren reaches deluge-sdk and flare by **sibling relative path** (`../deluge-sdk/...`, `../flare/crates/...`). No git deps on them, no remotes created, nothing pushed.
- deluge-linux-wren is **not touched**.
- deluge-wren code and manifests never name `deluge_bsp`, `rza1l_hal`, `rza1_alloc` or any `rza1-*`/`rza1_*` crate. The only exception is wren-firmware's `build.rs` passing `-Tlink.x`, which reaches rza1-rt's script through the link search path and names no crate.
- No Wren feature, refactor or API changes beyond what the move needs.
- deluge-sdk gets **no** replacement for the dropped 32-bit `deluge-wren-core` QEMU test.
- deluge-sdk keeps only: one README pointer line to deluge-wren and a CHANGELOG `### Removed` entry.
- deluge-wren `repository`/`homepage` = `https://github.com/FirestormAudio/deluge-wren`. Licence `MIT OR Apache-2.0`.
- deluge-wren's `tools/test.sh` builds wren-firmware **both plain and `--features rtt`**. There is no link-layout baseline.
- deluge-sdk work happens on branch `chore/wren-split`, cut from `docs/deluge-wren-split-spec` (which holds the spec and this plan). Don't merge to `main`; that's for superpowers:finishing-a-development-branch.
- Hardware checks run on the pi@pi-top bench, driven by the executor. Kate confirms the OLED by eye. A failed J-Link load gets **one retry** before Wren is blamed.

## Review Focus

1. **Copying untracked junk:** `wren/` has `target/`, `node_modules/`, `dist/`, `test-results/` on disk. The copy must take tracked files only (`git archive`), or deluge-wren's first commit fills up with build output. Task 3 checks the file count against `git ls-files wren | wc -l`.
2. **Relative-path depth after the move:** `wren-sys/build.rs` (`$CARGO_MANIFEST_DIR/../../../wren-rs/ext/wren`) and `wren-web/app/scripts/build-debug-wasm.sh` (`$APP/../../wren-web-debug`) must still resolve. Task 3's `crates/<name>` layout keeps both depths, and Task 3's builds run both scripts.
3. **Dangling `wren/` paths in moved prose and scripts:** READMEs, comments and scripts in the moved tree that say `wren/wren-web` etc. must say `crates/wren-web`. Task 3 adds a grep that must come back empty.
4. **Hidden leftovers in deluge-sdk:** an alias, baseline row, `.gitignore` line or test line that still names a Wren path breaks `cargo` or `tools/test.sh` only on the path that touches it. Task 4 ends with a repo-wide grep whose allowed hits are listed.
5. **Device-only facade dependency leaking to host/wasm:** wren-sys's new `deluge-sdk` dependency must be `cfg(target_os = "none")` only, or `deluge-sdk` (no backend feature) gets compiled for host/wasm and fails. Task 2 runs the host test and the wasm check after the change.

---

### Task 1: Pre-split on-device check of wren-firmware

Spec decisions 7 and 8. Nothing else starts until this passes. If it fails twice (with the one load retry), stop and report: fixing it is its own job.

**Files:**
- Create: `wren/tools/repl-smoke.py` (deluge-sdk). It moves to deluge-wren in Task 3 and is reused in Task 5.

**Interfaces:**
- Produces: `repl-smoke.py <tty>`. It runs on the Pi, exits 0 on pass and non-zero with a message on fail, and prints every line received.

- [ ] **Step 1: Branch**

```bash
cd ~/GitHub/deluge-sdk && git switch docs/deluge-wren-split-spec && git switch -c chore/wren-split
```

- [ ] **Step 2: Write `wren/tools/repl-smoke.py`**

Python 3, stdlib only. Open the tty with `os.open(tty, O_RDWR | O_NOCTTY | O_NONBLOCK)` and **no** `tty.setraw`/`tcsetattr`: the bench's CDC TCSAFLUSH hang applies. Poll-read with a timeout. Sequence:
- Send `^^v\n`. Expect a non-empty banner line within 3 s.
- Send `System.print(6 * 7)\n`. Expect a line that is exactly `42` within 3 s.
- Send `Oled.clear()\n`, `Oled.text(0, 0, "SPLIT OK")\n`, `Oled.show()\n`. Expect no line containing `error` (case-insensitive) within 2 s.
- Print `PASS — confirm the OLED shows "SPLIT OK"` and exit 0.
On any failure, print what was expected and the lines received, and exit 1.

- [ ] **Step 3: Build the firmware**

Run: `cargo build-wren --release --features rtt`
Expected: success; ELF at `target/armv7a-none-eabihf/release/wren-firmware`.

- [ ] **Step 4: Load it on the bench**

Run: `cd ~/GitHub/DelugeFirmware && python3 tools/deluge_jlink.py ~/GitHub/deluge-sdk/target/armv7a-none-eabihf/release/wren-firmware --select IP=pi-top --timeout 15`
Expected: load succeeds and the core is left running. On "GDB server never connected" or a load error, retry once.

- [ ] **Step 5: Check enumeration on the Pi**

Run: `ssh pi@pi-top 'sleep 2; lsusb | grep -i 1209:5741; ls /dev/ttyACM*'`
Expected: one `1209:5741` line and a `/dev/ttyACM*` node. If it's missing, re-enumerate with `ssh pi@pi-top 'echo 1 | sudo tee /sys/bus/usb/devices/1-1:1.0/1-1-port3/disable; sleep 1; echo 0 | sudo tee /sys/bus/usb/devices/1-1:1.0/1-1-port3/disable'` and check again.

- [ ] **Step 6: Run the REPL smoke test**

Run: `scp wren/tools/repl-smoke.py pi@pi-top:/tmp/ && ssh pi@pi-top 'python3 /tmp/repl-smoke.py /dev/ttyACM0'` (use the node Step 5 found).
Expected: `PASS …`, exit 0.

- [ ] **Step 7: Ask Kate to confirm the OLED**

Ask (AskUserQuestion, multiple choice): does the Deluge OLED show "SPLIT OK"? Options: "Yes, it shows" / "No / garbled". On "No", stop and report.

- [ ] **Step 8: Commit**

```bash
git add wren/tools/repl-smoke.py
git commit -m "wren: REPL smoke test for the pi-top bench"
```

---

### Task 2: wren-sys uses the allocator through the facade; layering check bans rza1 crates in Wren

Spec decision 5. This lands in deluge-sdk before the split.

**Files:**
- Modify: `tools/check-layering.sh` (the facade-only block, lines ~18–39)
- Modify: `wren/wren-sys/Cargo.toml`
- Modify: `wren/wren-sys/src/lib.rs:238` (`use rza1_alloc::{SDRAM, SRAM, Spill};`) and the two doc comments that cite `rza1_alloc` (lines 13, 213, 228). Keep their meaning and name the facade path.

**Interfaces:**
- Consumes: `deluge::deluge_alloc` (deluge-sdk's lib is named `deluge`; `#[cfg(target_os = "none")] pub use rza1_alloc as deluge_alloc;` in `crates/deluge-sdk/src/lib.rs:199`).
- Produces: wren-sys device builds depend on `deluge-sdk` (lib `deluge`) instead of `rza1-alloc`.

- [ ] **Step 1: Extend the facade-only check (the failing test)**

In `tools/check-layering.sh`, widen both facade-only regexes so the banned set is `deluge_bsp|rza1l_hal|rza1_[a-z_]+` in code and `deluge-bsp|rza1l-hal|rza1-[a-z-]+` in manifests (all three manifest forms already matched). Update the error text to say "reaches past `deluge` into deluge-bsp or the rza1 chip crates".

- [ ] **Step 2: Run it and see it fail**

Run: `tools/check-layering.sh`
Expected: exit 1, listing `wren/wren-sys/Cargo.toml` (`rza1-alloc = …`) and `wren/wren-sys/src/lib.rs:238`. If an `examples/baremetal` hit shows up too, stop and report: the spec assumed none.

- [ ] **Step 3: Switch wren-sys to the facade**

`wren/wren-sys/Cargo.toml`: remove `rza1-alloc` from `[dependencies]` and add

```toml
[target.'cfg(target_os = "none")'.dependencies]
deluge-sdk = { path = "../../crates/deluge-sdk", default-features = false }
```

`src/lib.rs:238`: `use deluge::deluge_alloc::{SDRAM, SRAM, Spill};`

- [ ] **Step 4: Run the layering check**

Run: `tools/check-layering.sh`
Expected: `==> Layering check passed.`

- [ ] **Step 5: Device builds, both configs**

Run: `cargo build-wren --release && cargo build-wren --release --features rtt`
Expected: both succeed.

- [ ] **Step 6: Full suite (covers host + wasm + QEMU — Review Focus 5)**

Run: `tools/test.sh`
Expected: exit 0. The `wren-web` wasm32-wasip1 check must run, not skip; install the target if it's missing.

- [ ] **Step 7: Commit**

```bash
git add tools/check-layering.sh wren/wren-sys
git commit -m "wren-sys: VM heap via deluge::deluge_alloc; layering check bans rza1 crates in facade-only code"
```

---

### Task 3: Stand up `~/GitHub/deluge-wren`

**Files (all in the new repo):**
- Create: `crates/{wren-sys,deluge-wren-core,wren-firmware,wren-web,wren-web-debug,wren-analyzer-wasm}/` (from deluge-sdk `wren/<name>/`)
- Create: `tools/repl-smoke.py` (from `wren/tools/`)
- Create: `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml`, `rust-toolchain.toml`, `.gitignore`, `LICENSE-MIT`, `LICENSE-APACHE`, `README.md`
- Create: `tools/test.sh`, `tools/check-layering.sh`
- Create: `docs/wren-scripting.md`, `docs/web-editor-plan.md`, `docs/superpowers/specs/` (the 5 `*wren*` specs), `docs/superpowers/plans/` (the 5 `*wren*` plans), all moved from deluge-sdk

**Interfaces:**
- Consumes: Task 2's wren-sys manifest. Its path `../../crates/deluge-sdk` becomes `../../../deluge-sdk/crates/deluge-sdk`.
- Produces: `~/GitHub/deluge-wren` at one commit, with `tools/test.sh` green and aliases `cargo build-wren`, `cargo build-wren-bin`, `cargo test-wren-debug`. Task 5 uses `cargo build-wren` and `tools/repl-smoke.py`.

- [ ] **Step 1: Copy the tracked tree only (Review Focus 1)**

```bash
mkdir ~/GitHub/deluge-wren && cd ~/GitHub/deluge-wren && git init -b main
git -C ~/GitHub/deluge-sdk archive HEAD wren | tar -x
mkdir crates && mv wren/tools tools && mv wren/* crates/ && rmdir wren
```

Check: `find crates tools -type f | wc -l` equals `git -C ~/GitHub/deluge-sdk ls-files wren | wc -l`.
Copy the docs listed above with `git -C ~/GitHub/deluge-sdk archive HEAD <paths> | tar -x`, then `cp` deluge-sdk's `LICENSE-MIT`, `LICENSE-APACHE`, `rust-toolchain.toml` and `Cargo.lock`. cargo prunes the lock and keeps versions pinned.

- [ ] **Step 2: Repoint the manifests**

- `crates/wren-firmware/Cargo.toml`: `deluge-sdk = { path = "../../../deluge-sdk/crates/deluge-sdk", … }` (features unchanged).
- `crates/wren-sys/Cargo.toml`: the device-only `deluge-sdk` path, same target.
- Root `Cargo.toml`:
  - `members = ["crates/wren-sys", "crates/deluge-wren-core", "crates/wren-firmware"]`
  - `exclude = ["crates/wren-web", "crates/wren-web-debug", "crates/wren-analyzer-wasm"]`
  - `resolver = "2"`
  - `[workspace.package]`: deluge-sdk's version/edition/license/authors, with repository/homepage set per Global Constraints.
  - `[workspace.dependencies]`: only `flare-graph`, `flare-kernels`, `flare-mipgen` (paths `../flare/crates/<name>`), `embassy-executor`, `embassy-futures`, `embassy-sync`, `embassy-time`, `embassy-usb`, `cortex-ar`, `log` (with `release_max_level_info`), all with the versions copied verbatim from deluge-sdk.
  - deluge-sdk's `[profile.release]`, `[profile.release.package.flare-fft]` and `[profile.dev]` copied verbatim with their comment. Drop the flare-fft override only if cargo warns it matches no package.

- [ ] **Step 3: `.cargo/config.toml` and `.gitignore`**

Copy from deluge-sdk's `.cargo/config.toml`:
- `[build] target`
- both `[target.*]` tables
- the `[env]` table (`EMBASSY_USB_MAX_INTERFACE_COUNT = "8"` and its comment)
- the `build-wren`, `build-wren-bin` and `test-wren-debug` aliases, with `test-wren-debug`'s path changed to `crates/wren-web-debug/Cargo.toml`
- `build-fw` / `build-fw-rel`, for parity with the SDK's README conventions.

`.gitignore`: deluge-sdk's generic entries (`/target/`, `**target/`, `**/node_modules/*`, `.vite/`, `__pycache__/`, IDE/OS lines, `*.jdebug.user`, `/JLinkLog.log`).

- [ ] **Step 4: Fix the moved-path references (Review Focus 3)**

Run: `grep -rnI 'wren/\(wren-\|deluge-wren\)' crates tools docs/wren-scripting.md docs/web-editor-plan.md`, and change each hit to `crates/…`. Leave `docs/superpowers/` alone: those are historical records. Also grep `crates tools` for `\.\./\.\./crates/deluge-sdk`, `\.\./\.\./rza1` and `tools/test.sh` mentions that assume the SDK layout, and fix each.
Expected after: the first grep returns nothing.

- [ ] **Step 5: `tools/check-layering.sh`**

Copy the facade-only block from Task 2's deluge-sdk version (both regexes and the comment-skipping filter), with `FACADE_ONLY_DIRS=(crates)` and nothing else from the SDK script.
Run: `tools/check-layering.sh` → `==> Layering check passed.`
Prove the check works: temporarily add `use rza1_alloc as _x;` to `crates/wren-sys/src/lib.rs` and run it again. Expect exit 1 naming that line, then revert.

- [ ] **Step 6: `tools/test.sh`**

`set -euo pipefail`, then run from the repo root:
1. `tools/check-layering.sh`
2. QEMU: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-wren-core`. Carry over the comment about 32-bit `usize` offset arithmetic, minus the "this repo's integration check against flare" framing.
3. Host: `cargo test --target x86_64-unknown-linux-gnu -p deluge-wren-core` and `cargo test --target x86_64-unknown-linux-gnu --manifest-path crates/wren-web-debug/Cargo.toml`
4. The `wren-web` wasm32-wasip1 `cargo check`, with deluge-sdk's install-guard/skip block and its comment.
5. `cargo build-wren --release` and `cargo build-wren --release --features rtt`, with a comment that the rtt build is what exercises `link_rtt.x`'s overlap check against the RTT reserve (spec decision 9).

Copy the `QEMU`/`HOST` variable style from deluge-sdk's `tools/test.sh`.

- [ ] **Step 7: Run it**

Run: `tools/test.sh`
Expected: exit 0, with the wasm check run (not skipped).
Also run: `cd crates/wren-web/app && npm ci && npm run build:debug-wasm && npm run build`
Expected: success. This proves `build-debug-wasm.sh`'s `../../wren-web-debug` path (Review Focus 2). `wren-sys`'s `../../../wren-rs` path is proved by `test.sh`'s `cargo build-wren` builds.

- [ ] **Step 8: README**

`README.md` in deluge-wren covers:
- what deluge-wren is (Wren live-coding for the Deluge: on-device firmware, plus the web editor/simulator and debugger)
- that it needs `deluge-sdk`, `flare` and `wren-rs` checked out next to it
- the crate table
- how to build the firmware (`cargo build-wren` / `-bin`) and run `tools/test.sh`
- how to run the web app
- a link to `docs/wren-scripting.md`
- "No CI yet (no remote)" as a known gap.

- [ ] **Step 9: Initial commit**

```bash
git add -A
git status --short | grep -E 'target/|node_modules/|dist/|test-results/'   # must print nothing
git commit -m "Import the Wren subsystem from deluge-sdk $(git -C ~/GitHub/deluge-sdk rev-parse --short HEAD)"
```

Expected: the grep prints nothing and the commit succeeds.

---

### Task 4: Remove Wren from deluge-sdk

**Files (deluge-sdk):**
- Delete: `wren/`, `docs/wren-scripting.md`, `docs/web-editor-plan.md`, the 5 `docs/superpowers/specs/*wren*` and 5 `docs/superpowers/plans/*wren*` files (not this split's spec and plan)
- Modify: `Cargo.toml` (members/exclude), `Cargo.lock` (cargo updates it), `.cargo/config.toml:46-49`, `tools/test.sh:53-57,67,75-85`, `tools/check-layering.sh` (`FACADE_ONLY_DIRS`, header comment, error text), `tools/link-layout.sh:19-21`, `tools/link-layout.baseline` (all 49 `wren-firmware` rows), `README.md:175-176,200,270`, `docs/advanced-guide.md:347,600`, `docs/audio-graph-vs-glicol.md`, `docs/audio-graph-vs-scsynth.md`, `tools/deluge-web-sim/src/panel.ts:4`, `tools/deluge-web-sim/vite.config.ts:4`, `tools/deluge-web-sim/src/deluge-layout.ts` (comments), `CHANGELOG.md`

**Interfaces:**
- Consumes: Task 3's repo, so the README pointer has something to point at.

- [ ] **Step 1: Delete the tree and docs**

`git rm -r wren` plus the doc files above.

- [ ] **Step 2: Workspace and aliases**

- `Cargo.toml`: drop `wren/deluge-wren-core`, `wren/wren-sys` and `wren/wren-firmware` from `members`, and the three `wren/*` entries from `exclude`. Update the exclude comment, which mentions "the wasm web-simulator core".
- `.cargo/config.toml`: drop the `test-wren-debug`, `build-wren` and `build-wren-bin` aliases and their comment.

- [ ] **Step 3: Tooling**

- `tools/test.sh`: remove both `deluge-wren-core` runs, the `wren-web-debug` run, and the `wren-web` wasm block with its comment. Rewrite the flare comment so it says only that flare tests itself.
- `tools/check-layering.sh`: `FACADE_ONLY_DIRS=(examples/baremetal)`. Header comment and error text name only the bare-metal examples. Keep the rza1 ban from Task 2.
- `tools/link-layout.sh`: drop `wren-firmware` from `PLAIN` and `RTT`.
- `tools/link-layout.baseline`: `grep -v '^wren-firmware' tools/link-layout.baseline > tmp && mv tmp tools/link-layout.baseline`

- [ ] **Step 4: Prose**

- `README.md`:
  - Replace the Wren scripting bullet with `- **Wren scripting** — live-code the Deluge in [Wren](https://wren.io): see the [deluge-wren](https://github.com/FirestormAudio/deluge-wren) repo.`
  - Drop the `wren/` row from the layout table.
  - Drop `cargo build-wren` / `-bin` from the alias sentence.
- `docs/advanced-guide.md`:
  - Line 347: "as deluge-wren's `wren-firmware` does".
  - Line 600: drop `build-wren` / `build-wren-bin`.
- `docs/audio-graph-vs-glicol.md` / `docs/audio-graph-vs-scsynth.md`: change each code path into the moved tree (`wren/…`, `deluge-wren-core/src/…`) to read "deluge-wren's `crates/…`". Prose that just says "Wren" stays.
- `tools/deluge-web-sim` comments: change `wren/wren-web/app` to "deluge-wren's `crates/wren-web/app`".
- `CHANGELOG.md` under `## [Unreleased]`: add a `### Removed` entry: "The Wren scripting subsystem (`wren-sys`, `deluge-wren-core`, `wren-firmware`, the web editor/simulator and debugger) moved to its own repo, deluge-wren; it consumes this SDK through the `deluge` facade."

- [ ] **Step 5: Leftover grep (Review Focus 4)**

Run: `git grep -n -i wren -- ':!docs/superpowers' ':!CHANGELOG.md' ':!vendor'`
Expected hits, and only these:
- the README pointer line
- `advanced-guide.md:347`
- the rewritten deluge-web-sim comments, including `deluge-layout.ts`'s "wren id space"/"wren binding" mentions (these describe the link protocol and stay)
- prose mentions in the two audio-graph docs

Any hit in a `Cargo.toml`, `.cargo/config.toml`, `.gitignore`, `tools/*.sh` or baseline is a bug.

- [ ] **Step 6: Run the suite**

Run: `tools/test.sh && tools/link-layout.sh check`
Expected: both exit 0. `cargo` doesn't complain about missing workspace members.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "Remove wren/: the Wren subsystem now lives in deluge-wren"
```

---

### Task 5: Post-split on-device check from deluge-wren

Spec order step 5. This is Task 1 again, against the new repo and the post-Task-4 deluge-sdk.

**Interfaces:**
- Consumes: deluge-wren's `cargo build-wren` and `tools/repl-smoke.py` (Task 3).

- [ ] **Step 1: Build from the new repo**

Run: `cd ~/GitHub/deluge-wren && cargo build-wren --release --features rtt`
Expected: success; ELF at `~/GitHub/deluge-wren/target/armv7a-none-eabihf/release/wren-firmware`.

- [ ] **Step 2: Load, enumerate, smoke-test**

Repeat Task 1 Steps 4–6 with that ELF and `~/GitHub/deluge-wren/tools/repl-smoke.py`.
Expected: `1209:5741` enumerates and the smoke test prints `PASS`.

- [ ] **Step 3: Kate confirms the OLED**

Same question as Task 1 Step 7. On "No", stop and report: that's a split regression, since Task 1 passed.

- [ ] **Step 4: Record the outcome**

Update the memory file `sdk-layout-reorg.md` to say the split is DONE (deluge-wren at its initial commit, deluge-sdk branch `chore/wren-split`, both hardware checks passed) and that the September REPL/`Oled.show` check is resolved. Add a one-line `MEMORY.md` index entry for deluge-wren if none exists. Nothing to commit in either repo.
