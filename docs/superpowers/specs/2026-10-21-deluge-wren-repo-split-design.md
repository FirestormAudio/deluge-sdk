# Split Wren out of deluge-sdk into a deluge-wren repo

Date: 2026-10-21
Status: design, approved for planning once reviewed

## Intent

deluge-sdk is the Rust platform: chip crates, the BSP, the `deluge` facade and its
backends, the examples. Wren scripting is one application built on that platform, and
it shouldn't weigh down the SDK's clones, CI or test suite. Move the Wren subsystem into
its own repo, `deluge-wren`, which consumes deluge-sdk the way any outside app would.

The 2026-09-16 layout reorg did the groundwork: all Wren code lives under `wren/`, and
`tools/check-layering.sh` keeps it on the `deluge` facade. Wren has had about 9 commits
since then, while the BSP and USB work changes daily, so the two halves already move at
different speeds.

## Goals

- A new local repo `~/GitHub/deluge-wren` holding all six crates from `deluge-sdk/wren/`:
  `wren-sys`, `deluge-wren-core`, `wren-firmware`, `wren-web` (including its `app/` web
  front end), `wren-web-debug`, `wren-analyzer-wasm`.
- The Wren docs from deluge-sdk move with them.
- After the split, deluge-sdk contains no Wren code and has no Wren build, test or alias
  entries. One README line points to deluge-wren, and the CHANGELOG notes the removal.
- deluge-wren builds and tests on its own: wren-firmware for the device, the QEMU and host
  test runs, the wasm crates.
- Both repos' `tools/test.sh` pass after the split.

## Non-goals

- **deluge-linux-wren stays out.** It stays a separate repo, unchanged. It is stale today:
  its real work sits on the unmerged `feat/wren-host` branch, `package.sh` builds through
  the retired `deluge-linux-sdk`, and its path dependencies point at pre-September
  deluge-sdk paths. Reviving it and folding it into deluge-wren is a separate job. This
  split doesn't try to keep it building (it doesn't build now).
- No git remotes are created or published. deluge-sdk, flare and deluge-wren all stay local.
- No new features, refactors or API changes in Wren beyond what the move requires.
- deluge-sdk doesn't replace the Wren-based 32-bit integration test (see Constraints).
- `wren-rs`, `wren-rs-vm` and `tree-sitter-wren` are already separate repos and are not touched.

## Anti-goals (a failure even if it builds)

- deluge-wren reaching into deluge-sdk below the facade: naming `deluge_bsp`,
  `rza1l_hal`, `rza1_alloc` or any `rza1-*` crate in code or manifests. The one exception
  is wren-firmware's build script selecting `rza1-rt`'s `link.x` and deluge-bsp's
  `memory.x` (today's mechanism). Keep it as it is, unless the builder finds a
  facade-provided way that's no more work.
- deluge-sdk keeping hidden Wren leftovers that still build or need Wren: aliases,
  link-layout baseline rows, test lines, layering rules for a directory that no longer exists.
- Losing the Wren work in progress: anything uncommitted under `wren/` at split time
  has to make it across.

## Decisions (made by Kate)

1. **Why:** keep deluge-sdk lean. Wren is an app on the platform, not part of it.
2. **History: fresh start.** deluge-wren begins with one initial commit containing the
   moved tree. The old history stays in deluge-sdk's log. No `git filter-repo`.
3. **Dependencies: sibling paths for now.** deluge-wren names
   `../deluge-sdk/crates/deluge-sdk` (and any other deluge-sdk crate it uses, e.g.
   `deluge-fonts` if needed) and `../flare/crates/*` by relative path, the way deluge-sdk
   already names `../flare`. These switch to pinned git revs once the remotes exist (a
   later job).
4. **Test gap: drop it.** deluge-sdk's `cargo test --target $QEMU -p deluge-wren-core`
   (its only 32-bit ARM check against flare) goes with the crate, and deluge-sdk adds
   nothing in its place. flare's own repo tests on ARM, and deluge-wren's suite keeps
   running the same test.
5. **Allocator leak: fix through the facade, before the split.** `wren-sys` depends on
   `rza1/rza1-alloc` directly (for its device heap: `rza1_alloc::{SDRAM, SRAM, Spill}`
   in `wren-sys/src/lib.rs`'s `mod heap`). The facade already re-exports it on device as
   `deluge::deluge_alloc` (`crates/deluge-sdk/src/lib.rs`,
   `#[cfg(target_os = "none")] pub use rza1_alloc as deluge_alloc`). wren-sys switches to
   that path. `check-layering.sh` grows to ban `rza1_alloc` / `rza1-*` in Wren code and
   manifests. This lands in deluge-sdk as its own commit before the split, while the
   layering check can still see `wren/`.
6. **What deluge-sdk keeps:** one README line ("scripting: see deluge-wren") and a CHANGELOG
   entry. The Wren docs move.
7. **Order:** the pending on-device check of wren-firmware comes first: the USB REPL
   enumerates as 1209:5741 and `Oled.show` draws (still open from the 2026-09-16 reorg). That
   way a split regression can't be confused with something that was already broken.
8. **Who runs the hardware checks: Claude, on the pi-top bench.** The executor runs the
   on-device checks (step 1 and step 5 below) itself on the pi@pi-top J-Trace bench: load
   wren-firmware by J-Link with `deluge_jlink.py --select IP=pi-top`, then confirm on the Pi
   that the board enumerates as 1209:5741 (Deluge USB is on Pi port 1-1.3; re-enumerate
   through sysfs port disable) and that the REPL answers over the CDC serial port. Kate
   confirms `Oled.show` by eye. RAM launch can be flaky when the boot menu is busy (a known
   bench issue), so a failed load gets one retry before anyone blames Wren.
9. **Link layout: build plain and rtt.** deluge-wren's `tools/test.sh` builds wren-firmware
   twice, plain and with `--features rtt`, so the `link_rtt.x` overlap check runs on every
   test. No link-layout baseline. Its `wren-firmware` rows leave deluge-sdk's baseline.

## Current coupling, as surveyed (2026-10-21)

Things in deluge-sdk that name Wren, which the split has to move or remove:

| Where | What |
|---|---|
| `Cargo.toml` | members `wren/deluge-wren-core`, `wren/wren-sys`, `wren/wren-firmware`; excludes `wren/wren-web`, `wren/wren-web-debug`, `wren/wren-analyzer-wasm` |
| `.cargo/config.toml` | aliases `test-wren-debug`, `build-wren`, `build-wren-bin` |
| `tools/test.sh` | QEMU and host `-p deluge-wren-core`; `wren/wren-web-debug` host test; the `wren/wren-web` wasm32-wasip1 check (and the comment explaining deluge-wren-core as the flare integration check) |
| `tools/check-layering.sh` | `FACADE_ONLY_DIRS=(wren examples/baremetal)` and its error text |
| `tools/link-layout.sh` + `tools/link-layout.baseline` | `wren-firmware` in the firmware lists and its baseline rows |
| `README.md` | the Wren scripting doc link (~line 175) and the `wren/` row in the layout table (~line 200) |
| `docs/` | `wren-scripting.md`, `web-editor-plan.md`; Wren mentions in `advanced-guide.md`, `audio-graph-vs-glicol.md`, `audio-graph-vs-scsynth.md` (these stay, with mentions fixed to point at deluge-wren where they'd otherwise dangle) |
| `docs/superpowers/specs/` + `plans/` | the `*wren*` specs (2026-07-01 web debugger, 07-04 project IO, 07-06 audio bindings, 07-10 sy synth surface, 07-11 slot hardening) and their five matching `*wren*` plans move too (Claude's call). The ~40 other suite specs/plans that mention Wren bindings in passing stay, unedited: they're historical records |
| `tools/deluge-web-sim/` | comments only (it was slimmed down from wren-web's panel). Stays in deluge-sdk; fix the comments so they don't point at a path that no longer exists |
| CI (`.github/workflows/*.yml`) | no direct Wren mentions; `test.yml` runs `tools/test.sh` |

Wren's own outward dependencies:

- `wren-firmware` → `../../crates/deluge-sdk` (features `usb-serial`), workspace `flare-*`, embassy, `cortex-ar`, `log`. Its `build.rs` picks `rza1-rt`'s `link.x`, raises the program stack, and links newlib.
- `deluge-wren-core` → workspace `flare-graph`, `flare-kernels`, `flare-mipgen`; optional `wren-sys`.
- `wren-sys` → `rza1-alloc` (fixed by decision 5). Its `build.rs` finds the patched C VM
  at `$CARGO_MANIFEST_DIR/../../../wren-rs/ext/wren` (or an env override), so it assumes
  `wren-rs` sits next to the repo.
- `wren-web-debug`, `wren-analyzer-wasm` → `wren-rs` crates by git rev
  `189765a1…` (the comments mention a local path override; keep that as it is).
- `wren-web/app/scripts/build-debug-wasm.sh` finds `wren-web-debug` at `$APP/../../wren-web-debug`.
- Everything uses `[workspace.package]` (version, edition, license, authors, repository,
  homepage) and `[workspace.dependencies]` from deluge-sdk's root manifest. There are no
  `[patch]` sections.
- Build config deluge-wren needs its own copy of: `rust-toolchain.toml` (nightly,
  `armv7a-none-eabihf`, `rust-src`, `llvm-tools-preview`), and the `.cargo/config.toml`
  pieces Wren uses (`[build] target`, the `armv7a-none-eabihf` rustflags, the
  `armv7-unknown-linux-gnueabihf` QEMU runner, the three Wren aliases, and the
  `[env] EMBASSY_USB_MAX_INTERFACE_COUNT = "8"` entry, which the device USB REPL depends on).
  The root manifest's `[profile.*]` sections, including the
  `release.package.flare-fft opt-level=3` override, also have to be copied: profiles only
  apply in the workspace being built.

## The new repo

- Location: `~/GitHub/deluge-wren`, next to `deluge-sdk`, `flare` and `wren-rs`.
- Licence: same as the moved crates (workspace `MIT OR Apache-2.0`). Copy deluge-sdk's
  `LICENSE-MIT` / `LICENSE-APACHE`.
- Its own root `Cargo.toml` workspace, with copies of the `[workspace.package]` and
  `[workspace.dependencies]` entries the Wren crates use (only those). Same
  member/exclude split as today: device and host crates are members, wasm crates are
  excluded and build from their own directories.
- `repository`/`homepage` → `https://github.com/FirestormAudio/deluge-wren`, by
  analogy with deluge-sdk. Nothing is pushed there yet.
- Its own `tools/test.sh` running what deluge-sdk's ran for Wren: QEMU and host
  `deluge-wren-core`, the `wren-web-debug` host tests, the `wren-web` wasm check (skipped
  when the target isn't installed), and device builds of `wren-firmware`, plain and
  `--features rtt` (decision 9).
- Its own facade-only layering check. A small script is enough: no `deluge_bsp`,
  `rza1l_hal`, `rza1_alloc`, `rza1-*` in code or manifests, with the `build.rs`
  linker-script exception from the Anti-goals.
- A README: what this is, how it relates to deluge-sdk and flare (sibling checkouts
  needed, plus wren-rs for the C VM), how to build the firmware and the web editor.
- No CI workflow yet (no remote). Note it as a follow-up.

## Order of work

1. On-device check of wren-firmware: REPL plus `Oled.show` (decision 7). If it fails,
   fix it in deluge-sdk first, as its own job.
2. Allocator fix plus the stricter layering check in deluge-sdk (decision 5). Run
   deluge-sdk's `tools/test.sh` and `cargo build-wren`.
3. Create deluge-wren from the current `wren/` tree, docs and config; get it building
   and its `tools/test.sh` green.
4. Remove Wren from deluge-sdk (everything in the coupling table); deluge-sdk
   `tools/test.sh` green; README/CHANGELOG updated.
5. Smoke test: the device firmware built from deluge-wren boots and the REPL answers.
   This is the same check as step 1, now against the new repo.

Steps 3 and 4 can be one deluge-sdk commit ("remove wren/, now in deluge-wren") plus the
deluge-wren initial commit. Commit in deluge-sdk on a branch, per the usual workflow.

## Left to the builder

- The exact directory layout inside deluge-wren (for example `crates/<name>` vs a flat
  layout), as long as `wren-sys`'s `../../../wren-rs` lookup and the web app's
  `../../wren-web-debug` lookup still resolve, or are updated to match.
- Whether wren-sys takes `deluge-sdk` as a device-only (`cfg(target_os = "none")`)
  dependency or another arrangement that still compiles on host and wasm (wren-sys
  builds on all three).
- Which deluge-sdk docs that merely mention Wren need a reworded pointer vs no change.
- Whether `node_modules`, `dist`, `test-results`, `target` under `wren/` are left behind
  (they should be ignored, not copied) and the `.gitignore` deluge-wren needs.
- Memory/doc housekeeping afterwards (the `sdk-layout-reorg` memory's "Wren separable
  later" note becomes "done").
