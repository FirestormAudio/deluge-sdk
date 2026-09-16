# SDK Layout Reorg (Wren grouping + facade-only layering) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Wren subsystem use the SDK only through the public `deluge` API and group it under `wren/`, then tidy the rest of the repo layout, with no behaviour change.

**Architecture:** First, give the SDK the two public APIs `wren-firmware` currently gets by reaching into `deluge_bsp`/`rza1l_hal`: an OLED frame-buffer API and a USB CDC-ACM serial capability. Port the firmware onto them and lock that in with a layering check script. Then do a series of directory moves. Each move changes only paths: manifests, `include_str!`, scripts and docs. Package names stay the same, so every `cargo -p …` alias and CI job keeps working. A manifest-resolution script catches stale `path =` dependencies after each move.

**Tech Stack:** Rust nightly (`armv7a-none-eabihf` device workspace plus separately-built excluded crates), Embassy + `embassy-usb`, bash, cargo.

**Spec:** No separate spec doc. This plan implements the reorg agreed in conversation on 2026-09-16:
- Group Wren under `wren/` in this repo now. Remove its reach-through into the BSP/HAL. Moving it to its own repo is a **later** decision and is not part of this plan.
- `tools/deluge-simulator` → `crates/`
- `app-loader` → `firmwares/`
- hardware probes → `hw-tests/`
- Linux crates → `linux/`
- `examples/` + `examples-linux/` → `examples/{baremetal,linux}/`
- **No `crates/gpl/` folder**: the GPL crates stay where they are (the user's call).

## Global Constraints

- **No behaviour change.** Tasks 4–7 are path-only. Tasks 1–3 add SDK API and port `wren-firmware` onto it, keeping the same USB identity (VID `0x1209`, PID `0x5741`, manufacturer `"skyline"`, product `"wren: deluge"`, serial `"deluge-wren"`), the same 512-byte bulk endpoints and the same OLED output.
- **Package names never change.** Aliases in `.cargo/config.toml` and `.github/workflows/images.yml` select by `-p <name>` and must still work unedited (except the `--manifest-path` alias called out in Task 4).
- **The GPL crates stay put:** `crates/deluge-fonts`, `crates/deluge-ui-toolkit`, `crates/deluge-grid-toolkit`. Do not create a `gpl/` directory.
- **Don't rewrite history docs.** Leave `docs/superpowers/**` and `CHANGELOG.md` history untouched. You may add a new `[Unreleased]` entry.
- **flare is a sibling checkout.** Workspace deps point at `../flare/…` relative to the repo root, and `wren-sys` falls back to `../wren-rs`. Work on a branch in `~/GitHub/deluge-sdk` itself, or in a worktree that is a *sibling* directory (e.g. `~/GitHub/deluge-sdk-reorg`). **Never** use a nested `.worktrees/` dir: `../flare` would not resolve there.
- **Move directories with `mv`, not `git mv`**, so ignored build products (`target/`, `node_modules/`, `dist/`, `Cargo.lock`) move with the source. Then `git add -A -- <old> <new>` so git records the renames.
- **Wren code must never name `deluge_bsp` or `rza1l_hal` in code** (comments are fine), and no Wren manifest may depend on `deluge-bsp` or `rza1l-hal`. `tools/check-layering.sh` enforces this from Task 3 on.
- **Another repo depends on these paths.** `~/GitHub/deluge-linux-wren/crates/wren-host/Cargo.toml` has path deps into `deluge-wren-core` and `wren-web-debug`. Update it in the same task that moves them (Task 4).
- **Commit style:** conventional commits scoped like the existing log, e.g. `feat(deluge-sdk): …`, `refactor(wren-firmware): …`, `chore(layout): …`.

## Before you start: baseline

- [ ] **B1: Record the baseline** so pre-existing failures aren't blamed on this work.

```bash
cd ~/GitHub/deluge-sdk
git switch -c chore/sdk-layout-reorg
tools/test.sh 2>&1 | tail -20
tools/build-examples.sh 2>&1 | tail -5
cargo build-wren 2>&1 | tail -3
cargo build-app-loader 2>&1 | tail -3
```

Write down anything that already fails. Every later "Expected: PASS" means "no new failures compared with this baseline".

---

### Task 1: Public OLED frame-buffer API (`deluge::oled`)

`wren-firmware` keeps its own static frame buffer, which its bindings draw into from the VM task. `ui_task` then copies that buffer into the SDK `Oled`. To do this it names `deluge_bsp::oled::FrameBuffer` and calls `deluge_bsp::oled::text::draw_str`. This task exposes both through the facade.

**Files:**
- Modify: `crates/deluge-sdk/src/oled.rs` (imports at top, `Oled::text`, add tests at end)
- Modify: `crates/deluge-sdk/src/lib.rs:87` (`mod oled;` → `pub mod oled;`)

**Interfaces:**
- Produces: `deluge::oled::FrameBuffer` (re-export of `deluge_bsp::oled::FrameBuffer`) and `pub fn deluge::oled::draw_str(fb: &mut FrameBuffer, x: usize, y: usize, s: &[u8])`.

- [ ] **Step 1: Write the failing test.** Append to `crates/deluge-sdk/src/oled.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{FrameBuffer, draw_str};

    /// `draw_str` is the facade over the BSP's 5×7 text renderer: same pixels.
    #[test]
    fn draw_str_matches_bsp_text() {
        let mut ours = FrameBuffer::new();
        draw_str(&mut ours, 3, 10, b"Wren");
        let mut bsp = FrameBuffer::new();
        deluge_bsp::oled::text::draw_str(&mut bsp, 3, 10, b"Wren");
        assert_eq!(ours.as_bytes(), bsp.as_bytes());
        assert!(ours.as_bytes().iter().any(|&b| b != 0), "nothing was drawn");
    }
}
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cargo test --target x86_64-unknown-linux-gnu -p deluge-sdk --features sim oled::tests`
Expected: compile error, `cannot find function 'draw_str' in module 'super'`.

- [ ] **Step 3: Implement.** At the top of `crates/deluge-sdk/src/oled.rs`, replace

```rust
use deluge_bsp::oled::{self, FrameBuffer};
```

with

```rust
use deluge_bsp::oled;
/// The raw 128 × 48, 1-bit OLED frame buffer — what [`Oled`] draws into. Apps
/// that render off-task (e.g. a scripting VM) can own one and copy it into
/// [`Oled::frame`] before [`Oled::flush`].
pub use deluge_bsp::oled::FrameBuffer;

/// Draw an ASCII string into `fb` at pixel (`x`, `y`) with the built-in 5×7
/// font — [`Oled::text`] for a [`FrameBuffer`] the app owns itself.
#[inline]
pub fn draw_str(fb: &mut FrameBuffer, x: usize, y: usize, s: &[u8]) {
    oled::text::draw_str(fb, x, y, s);
}
```

Then change the body of `Oled::text` so there is only one code path:

```rust
    pub fn text(&mut self, x: usize, y: usize, s: &str) {
        draw_str(&mut self.fb, x, y, s.as_bytes());
    }
```

In `crates/deluge-sdk/src/lib.rs`, change `mod oled;` to `pub mod oled;` (the existing `pub use oled::Oled;` stays).

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --target x86_64-unknown-linux-gnu -p deluge-sdk --features sim`
Expected: PASS, including `oled::tests::draw_str_matches_bsp_text`.

Run: `cargo build-fw -p oled_hello`
Expected: builds (the device path still compiles).

- [ ] **Step 5: Commit**

```bash
git add crates/deluge-sdk/src/oled.rs crates/deluge-sdk/src/lib.rs
git commit -m "feat(deluge-sdk): expose oled::FrameBuffer and oled::draw_str"
```

---

### Task 2: `usb-serial` capability (`Deluge::usb_serial`)

`wren-firmware` builds its own CDC-ACM device from `rza1l_hal::usb` and registers the USB0 ISR with `rza1l_hal::gic`. The HAL requires `gic::register` to run **before** IRQs are enabled (`crates/rza1l-hal/src/gic.rs:264-270`). The runtime therefore registers the ISR before `setup()` whenever the feature is on, and `Deluge::usb_serial` builds the device after IRQs are enabled. That matches the ordering `wren-firmware` uses today: register in `setup`, build in `main`.

**Files:**
- Create: `crates/deluge-sdk/src/usb_serial.rs`
- Modify: `crates/deluge-sdk/Cargo.toml` (`[features]`, next to `usb-log`)
- Modify: `crates/deluge-sdk/src/lib.rs` (module decl near `mod usb_debug;` ~line 93; method after `Deluge::gate` ~line 363; ISR registration in the device `run()` ~line 600)
- Modify: `CHANGELOG.md` (`[Unreleased]` → `### Added`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces (device-only, `cfg(all(target_os = "none", feature = "usb-serial"))`):
  - `pub type deluge::usb_serial::Driver = rza1l_hal::usb::Rusb1Driver;`
  - `pub struct deluge::usb_serial::UsbIdentity { pub vid: u16, pub pid: u16, pub manufacturer: &'static str, pub product: &'static str, pub serial_number: &'static str }`
  - `pub struct deluge::usb_serial::UsbSerial { pub tx: embassy_usb::class::cdc_acm::Sender<'static, Driver>, pub rx: embassy_usb::class::cdc_acm::Receiver<'static, Driver> }`
  - `pub fn Deluge::usb_serial(&self, id: UsbIdentity) -> UsbSerial`

This is device code with no host test harness (see `tools/build-examples.sh` header), so the test for this task is that it compiles for the device and for the host. Task 3 then checks it end to end on hardware.

- [ ] **Step 1: Add the feature.** In `crates/deluge-sdk/Cargo.toml`, directly after the `usb-log = ["dep:embassy-usb"]` line:

```toml
## Let the app take USB0 as a CDC-ACM (virtual serial) device with its own
## VID/PID/strings via `Deluge::usb_serial` — e.g. a REPL transport. Device-only.
## Mutually exclusive with `usb-log` at runtime (both own USB0).
usb-serial = ["dep:embassy-usb"]
```

- [ ] **Step 2: Create `crates/deluge-sdk/src/usb_serial.rs`**

```rust
//! USB CDC-ACM serial port on USB0 (the `usb-serial` feature).
//!
//! [`Deluge::usb_serial`](crate::Deluge::usb_serial) brings up a USB device with
//! one CDC-ACM interface under the app's own [`UsbIdentity`], spawns the device
//! task, and hands back the IN/OUT halves. The USB0 ISR is registered by the
//! runtime before interrupts are enabled (the GIC contract), so the app needs no
//! `unsafe` and no `setup` hook.

use core::ptr::addr_of_mut;

use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, Sender, State};
use embassy_usb::{Builder, Config, UsbDevice};
use rza1l_hal::gic;
use rza1l_hal::usb::{Rusb1Driver, USB0_IRQ, dcd_int_handler, init_device_mode};

/// The `embassy-usb` driver for the Deluge's USB0 device port.
pub type Driver = Rusb1Driver;

/// How the serial device identifies itself to the host.
#[derive(Clone, Copy, Debug)]
pub struct UsbIdentity {
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: &'static str,
    pub product: &'static str,
    pub serial_number: &'static str,
}

/// A USB CDC-ACM port: write host-bound bytes to `tx`, read host bytes from `rx`.
pub struct UsbSerial {
    pub tx: Sender<'static, Driver>,
    pub rx: Receiver<'static, Driver>,
}

// `embassy_usb::Builder` and `CdcAcmClass` need `'static` buffers. One CDC
// interface fits comfortably in 256 B of config descriptor.
static mut USB_CONFIG_DESC: [u8; 256] = [0; 256];
static mut USB_BOS_DESC: [u8; 64] = [0; 64];
static mut USB_MSOS_DESC: [u8; 0] = [];
static mut USB_CONTROL_BUF: [u8; 64] = [0; 64];
static mut CDC_ACM_STATE: State<'static> = State::new();

/// Register the USB0 device-mode ISR.
///
/// # Safety
/// Call once, with IRQs masked (before `cortex_ar::interrupt::enable()`).
pub(crate) unsafe fn register_irq() {
    unsafe { gic::register(USB0_IRQ, || dcd_int_handler(0)) };
}

/// Build the CDC-ACM device and spawn its task. Called once, via
/// [`Deluge::usb_serial`](crate::Deluge::usb_serial)'s take-once guard.
pub(crate) fn start(spawner: Spawner, id: UsbIdentity) -> UsbSerial {
    // SAFETY: the take-once guard in `Deluge::usb_serial` makes this the only
    // caller, so USB0 is initialised once and the `static mut` buffers are
    // borrowed exactly once.
    let (device, cdc) = unsafe {
        let (_port, driver) = init_device_mode(0);
        let mut config = Config::new(id.vid, id.pid);
        config.manufacturer = Some(id.manufacturer);
        config.product = Some(id.product);
        config.serial_number = Some(id.serial_number);
        config.self_powered = false;
        config.max_power = 250; // 500 mA

        let mut builder = Builder::new(
            driver,
            config,
            &mut *addr_of_mut!(USB_CONFIG_DESC),
            &mut *addr_of_mut!(USB_BOS_DESC),
            &mut *addr_of_mut!(USB_MSOS_DESC),
            &mut *addr_of_mut!(USB_CONTROL_BUF),
        );
        // 512-byte bulk endpoints: the RUSB1 PHY negotiates high speed, where
        // USB 2.0 requires HS bulk wMaxPacketSize 512.
        let cdc = CdcAcmClass::new(&mut builder, &mut *addr_of_mut!(CDC_ACM_STATE), 512);
        (builder.build(), cdc)
    };
    let (tx, rx) = cdc.split();
    spawner.spawn(usb_run(device).unwrap());
    UsbSerial { tx, rx }
}

/// Run the USB device stack (enumeration, control transfers, endpoints).
#[embassy_executor::task]
async fn usb_run(mut device: UsbDevice<'static, Driver>) {
    device.run().await;
}
```

- [ ] **Step 3: Wire it into `lib.rs`.** Directly after the `mod usb_debug;` declaration (~line 93):

```rust
/// USB CDC-ACM serial capability; see [`Deluge::usb_serial`].
#[cfg(all(target_os = "none", feature = "usb-serial"))]
pub mod usb_serial;
```

After the `Deluge::gate` method (~line 363), inside `impl Deluge`:

```rust
    /// Take USB0 as a CDC-ACM (virtual serial) device identified as `id` — a
    /// `/dev/ttyACM*` port on the host. Starts the USB device task; read and
    /// write through the returned [`UsbSerial`](usb_serial::UsbSerial) halves.
    ///
    /// Device-only, behind the `usb-serial` feature. Takeable once: a second
    /// call panics, as does calling it while `usb-log` (which owns USB0) is on.
    #[cfg(all(target_os = "none", feature = "usb-serial"))]
    pub fn usb_serial(&self, id: usb_serial::UsbIdentity) -> usb_serial::UsbSerial {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::usb_serial() called more than once");
        }
        if cfg!(feature = "usb-log") {
            panic!("Deluge::usb_serial(): USB0 is already owned by the `usb-log` feature");
        }
        usb_serial::start(self.spawner, id)
    }
```

In the device `run()` (~line 600), between `unsafe { init_platform() };` and `setup();`:

```rust
            // `usb-serial`: register the USB0 device ISR while IRQs are still
            // masked (the GIC contract); `Deluge::usb_serial` builds the device.
            #[cfg(feature = "usb-serial")]
            unsafe { crate::usb_serial::register_irq() };
```

(If `usb-log` is also on, `usb_debug::build` registers the same handler again. That's harmless: it's the same function pointer.)

- [ ] **Step 4: Build for device and host**

Run: `cargo build-fw -p deluge-sdk --features usb-serial`
Expected: builds with no warnings from `usb_serial.rs`.

Run: `cargo build-fw -p usb_log`
Expected: builds (`usb-log` is unaffected).

Run: `cargo test --target x86_64-unknown-linux-gnu -p deluge-sdk --features sim,usb-serial`
Expected: PASS (the module is compiled out on the host).

- [ ] **Step 5: CHANGELOG.** Under `## [Unreleased]` → `### Added`, append:

```markdown
- `deluge::oled::{FrameBuffer, draw_str}` for apps that render into their own
  frame buffer, and the `usb-serial` feature's `Deluge::usb_serial` — take USB0
  as a CDC-ACM port under your own VID/PID, no `unsafe`.
```

- [ ] **Step 6: Commit**

```bash
git add crates/deluge-sdk/Cargo.toml crates/deluge-sdk/src/usb_serial.rs crates/deluge-sdk/src/lib.rs CHANGELOG.md
git commit -m "feat(deluge-sdk): add usb-serial capability (Deluge::usb_serial)"
```

---

### Task 3: Port `wren-firmware` onto the facade, and add the layering check

**Files:**
- Create: `tools/check-layering.sh`
- Modify: `tools/test.sh` (call the check first)
- Modify: `wren-firmware/Cargo.toml:29` (enable `usb-serial`)
- Modify: `wren-firmware/src/main.rs`: imports (lines 40–57), USB statics (~360–372), `OLED_FB` (99–100), `oled_text` (237), `setup` (~383–388), `main`'s device REPL block (~430–458), `usb_task` / `cdc_rx_task` / `cdc_tx_task` (~510–544)

**Interfaces:**
- Consumes: `deluge::oled::{FrameBuffer, draw_str}` (Task 1); `deluge::usb_serial::{Driver, UsbIdentity, UsbSerial}` and `Deluge::usb_serial` (Task 2).
- Produces: `tools/check-layering.sh` with a `WREN_DIRS` array. Task 4 changes that array to `(wren)`.

- [ ] **Step 1: Write the failing check.** Create `tools/check-layering.sh`:

```bash
#!/usr/bin/env bash
#
# Layering check: the Wren subsystem consumes the SDK only through the `deluge`
# facade, never the BSP/HAL crates beneath it, so SDK-internal changes can't
# silently break it (and it can later move to its own repo).
#
# Usage: tools/check-layering.sh
set -euo pipefail

cd "$(dirname "$0")/.."

WREN_DIRS=(wren-sys wren-firmware crates/deluge-wren-core tools/wren-web tools/wren-web-debug tools/wren-analyzer-wasm)

# Code lines only: a `//` comment naming a BSP item for reference is fine.
code_hits="$(grep -rnE --include='*.rs' --exclude-dir=target --exclude-dir=node_modules \
  '\b(deluge_bsp|rza1l_hal)\b' "${WREN_DIRS[@]}" \
  | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
dep_hits="$(grep -rnE --include='Cargo.toml' --exclude-dir=target --exclude-dir=node_modules \
  '^[[:space:]]*(deluge-bsp|rza1l-hal)[[:space:]]*=' "${WREN_DIRS[@]}" || true)"

if [ -n "$code_hits$dep_hits" ]; then
  printf '%s\n' "$code_hits" "$dep_hits" | sed '/^$/d'
  echo "error: Wren code reaches past the \`deluge\` facade into deluge-bsp/rza1l-hal" >&2
  exit 1
fi
echo "==> Layering check passed."
```

```bash
chmod +x tools/check-layering.sh
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `tools/check-layering.sh`
Expected: exit 1, listing `wren-firmware/src/main.rs` lines 40, 51, 57, 99, 100, 237, 387 (the line-49 comment mentions `rza1l_hal` inside a `//` comment and is filtered out).

- [ ] **Step 3: Enable the feature.** In `wren-firmware/Cargo.toml`, change line 29 to:

```toml
deluge-sdk = { path = "../crates/deluge-sdk", default-features = false, features = ["usb-serial"] }
```

- [ ] **Step 4: Port `main.rs`.**

(a) Delete `use deluge::deluge_bsp;` (line 40). Replace the block from the comment `// The USB-CDC REPL transport is device-only …` through `use rza1l_hal::usb::{…};` (lines 47–57) with:

```rust
// The USB-CDC REPL transport is device-only — the desktop simulator has no USB,
// so the host build drives the REPL over stdin/stdout instead (see `host_repl`
// and `host_tx_task`).
#[cfg(target_os = "none")]
use deluge::usb_serial::{Driver, UsbIdentity};
#[cfg(target_os = "none")]
use embassy_usb::class::cdc_acm::{Receiver, Sender};
```

(b) `OLED_FB` (lines 99–100):

```rust
static OLED_FB: Mutex<CriticalSectionRawMutex, RefCell<deluge::oled::FrameBuffer>> =
    Mutex::new(RefCell::new(deluge::oled::FrameBuffer::new()));
```

(c) `oled_text` body (line 237):

```rust
    OLED_FB.lock(|r| deluge::oled::draw_str(&mut r.borrow_mut(), x, y, s));
```

(d) Delete the five `#[cfg(target_os = "none")] static mut` USB buffers (`USB_CONFIG_DESC`, `USB_BOS_DESC`, `USB_MSOS_DESC`, `USB_CONTROL_BUF`, `CDC_ACM_STATE`) just above `// ── Pre-interrupt setup`. The SDK owns them now.

(e) In `setup`, delete the USB0 ISR block: the comment starting `// USB0 device-mode interrupt.` and the `#[cfg(target_os = "none")] unsafe { rza1l_hal::gic::register(…) };` under it. In `setup`'s doc comment, change `here we register the USB0 ISR and boot the` to `here we boot the`.

(f) In `main`, replace the whole `#[cfg(target_os = "none")] { let (device, cdc) = unsafe { … }; … spawner.spawn(cdc_rx_task(rx).unwrap()); }` block with:

```rust
    #[cfg(target_os = "none")]
    {
        let serial = dlg.usb_serial(UsbIdentity {
            vid: 0x1209,
            pid: 0x5741,
            manufacturer: "skyline",
            product: "wren: deluge",
            serial_number: "deluge-wren",
        });
        info!("USB: CDC-ACM device built (1209:5741)");
        spawner.spawn(cdc_tx_task(serial.tx).unwrap());
        spawner.spawn(cdc_rx_task(serial.rx).unwrap());
    }
```

Also update that section's comment `Device: a USB-CDC-ACM port (ISR registered in \`setup\`).` to `Device: a USB-CDC-ACM port (\`Deluge::usb_serial\`).`

(g) Delete the `usb_task` fn together with its `#[cfg(target_os = "none")]` and `#[embassy_executor::task]` attributes; the SDK spawns the device task now. In `cdc_rx_task` and `cdc_tx_task` signatures, replace `Rusb1Driver` with `Driver`.

(h) Check for leftovers:

Run: `grep -nE 'usb_task|Rusb1Driver|CDC_ACM_STATE|USB_CONFIG_DESC|init_device_mode|dcd_int_handler|deluge_bsp|rza1l_hal' wren-firmware/src/main.rs`
Expected: no output. If the module doc header (top of file) lists `usb_task`, remove that bullet.

- [ ] **Step 5: Run the checks and builds**

Run: `tools/check-layering.sh`
Expected: `==> Layering check passed.`

Run: `cargo build-wren && cargo build-wren --release --no-default-features`
Expected: both build with no new warnings (unused imports would mean step 4 missed something).

Run: `cargo test --target x86_64-unknown-linux-gnu -p deluge-wren-core`
Expected: PASS (unchanged crate; confirms nothing else broke).

- [ ] **Step 6: Wire the check into `tools/test.sh`.** Directly after `cd "$(dirname "$0")/.."`:

```bash
tools/check-layering.sh
```

- [ ] **Step 7: Hardware check (the user runs this; cannot be done by an agent).** Load `target/armv7a-none-eabihf/release/wren-firmware` onto a Deluge the usual way (dev-mode upload, or copy the ELF to `/APPS/` on the SD card). Then:
  1. `lsusb | grep 1209:5741` shows the device, and `dmesg` shows product `wren: deluge`.
  2. Open the port (`picocom /dev/ttyACM0`), type `System.print(1 + 1)` and press Enter. Expected: `2`.
  3. Type the following three lines, pressing Enter after each: `Oled.clear()`, `Oled.text(0, 0, "facade ok")`, `Oled.show()`. Expected: `facade ok` appears on the OLED.

  Do not commit until the user confirms all three.

- [ ] **Step 8: Commit**

```bash
git add tools/check-layering.sh tools/test.sh wren-firmware/Cargo.toml wren-firmware/src/main.rs
git commit -m "refactor(wren-firmware): use the deluge facade for OLED and USB serial"
```

---

### Task 4: Group Wren under `wren/`

Target layout:

```
wren/
  wren-sys/             ← wren-sys/
  deluge-wren-core/     ← crates/deluge-wren-core/
  wren-firmware/        ← wren-firmware/
  wren-web/             ← tools/wren-web/
  wren-web-debug/       ← tools/wren-web-debug/
  wren-analyzer-wasm/   ← tools/wren-analyzer-wasm/
```

**Files:**
- Create: `tools/check-manifests.sh`
- Move: the six directories above
- Modify: `Cargo.toml` (members + exclude + exclude comment), `wren/wren-sys/Cargo.toml:12`, `wren/wren-sys/build.rs:36`, `wren/deluge-wren-core/Cargo.toml:36`, `wren/deluge-wren-core/tests/audio_bindings.rs:452`, `wren/wren-firmware/Cargo.toml:29,33`, `wren/wren-web/Cargo.toml:22,24`, `wren/wren-web-debug/Cargo.toml:37`, `wren/wren-analyzer-wasm/src/lib.rs:39`, `.cargo/config.toml:63,66`, `tools/test.sh:70,72,78`, `tools/check-layering.sh` (`WREN_DIRS`), `docs/wren-scripting.md:145,154`, and prose references (step 6)
- Modify (other repo): `~/GitHub/deluge-linux-wren/crates/wren-host/Cargo.toml:16,19`

**Interfaces:**
- Consumes: `tools/check-layering.sh` (Task 3).
- Produces: `tools/check-manifests.sh`, which Tasks 5–7 run after every move.

- [ ] **Step 1: Create the manifest check** as `tools/check-manifests.sh` and run it on the unmoved tree:

```bash
#!/usr/bin/env bash
#
# Resolve every Cargo manifest in the repo — the workspace plus each excluded
# crate — so a moved directory's stale `path =` dependency or workspace
# membership fails fast. Resolves only; builds nothing (the excluded crates need
# wasm / musl-bundle toolchains a plain checkout may lack).
#
# Usage: tools/check-manifests.sh
set -euo pipefail

cd "$(dirname "$0")/.."

status=0
while read -r m; do
  if ! out="$(cargo metadata --format-version 1 --manifest-path "$m" 2>&1 >/dev/null)"; then
    echo "FAIL $m"
    echo "$out" | tail -5
    status=1
  fi
done < <(git ls-files --cached --others --exclude-standard -- 'Cargo.toml' '**/Cargo.toml')

[ "$status" -eq 0 ] && echo "==> All manifests resolve."
exit "$status"
```

```bash
chmod +x tools/check-manifests.sh
tools/check-manifests.sh
```

Expected: `==> All manifests resolve.` (If a manifest already fails at baseline, note it and treat it as pre-existing.)

- [ ] **Step 2: Move**

```bash
mkdir wren
mv wren-sys wren/wren-sys
mv wren-firmware wren/wren-firmware
mv crates/deluge-wren-core wren/deluge-wren-core
mv tools/wren-web wren/wren-web
mv tools/wren-web-debug wren/wren-web-debug
mv tools/wren-analyzer-wasm wren/wren-analyzer-wasm
git add -A -- wren wren-sys wren-firmware crates/deluge-wren-core tools/wren-web tools/wren-web-debug tools/wren-analyzer-wasm
```

- [ ] **Step 3: Confirm the check fails**

Run: `tools/check-manifests.sh`
Expected: `FAIL` lines, e.g. for `Cargo.toml` (members `wren-sys`, `wren-firmware` and `crates/deluge-wren-core` not found).

- [ ] **Step 4: Fix manifests and code paths** (exact new values):

`Cargo.toml` `members`: replace `"crates/deluge-wren-core",` with `"wren/deluge-wren-core",`, `"wren-sys",` with `"wren/wren-sys",`, and `"wren-firmware",` with `"wren/wren-firmware",`. In `exclude`, replace `"tools/wren-web",`, `"tools/wren-web-debug",` and `"tools/wren-analyzer-wasm",` with `"wren/wren-web",`, `"wren/wren-web-debug",` and `"wren/wren-analyzer-wasm",`. In the comment above `exclude`, replace `` `tools/wren-web` `` with `` `wren/wren-web` `` and `` `tools/wren-web-debug` `` with `` `wren/wren-web-debug` ``.

`wren/wren-sys/Cargo.toml`:
```toml
deluge-alloc = { path = "../../crates/deluge-alloc" }
```

`wren/wren-sys/build.rs` (fallback checkout is now one level further up):
```rust
        .unwrap_or_else(|_| manifest.join("../../../wren-rs/ext/wren"));
```

`wren/deluge-wren-core/Cargo.toml`:
```toml
wren-sys = { path = "../wren-sys", optional = true }
```

`wren/deluge-wren-core/tests/audio_bindings.rs:452`:
```rust
    let src = include_str!("../../wren-firmware/examples/midi_synth.wren");
```

`wren/wren-firmware/Cargo.toml` (the `wren-sys = { path = "../wren-sys" }` line is still correct):
```toml
deluge-sdk = { path = "../../crates/deluge-sdk", default-features = false, features = ["usb-serial"] }
deluge-wren-core = { path = "../deluge-wren-core" }
```

`wren/wren-web/Cargo.toml`:
```toml
deluge-wren-core = { path = "../deluge-wren-core" }
wren-sys = { path = "../wren-sys" }
```

`wren/wren-web-debug/Cargo.toml`:
```toml
deluge-wren-core = { path = "../deluge-wren-core", default-features = false }
```

`wren/wren-analyzer-wasm/src/lib.rs:39`:
```rust
const PRELUDE: &str = include_str!("../../deluge-wren-core/wren/prelude.wren");
```

`wren/wren-web/README.md:6`: change the link target `../../crates/deluge-wren-core` to `../deluge-wren-core`.

`.cargo/config.toml`: in line 63's comment and line 66's `--manifest-path`, `tools/wren-web-debug` → `wren/wren-web-debug`.

`tools/test.sh`: lines 70 and 78 `--manifest-path tools/wren-web…` → `--manifest-path wren/wren-web…`, and the comment on line 72 `` `tools/wren-web` `` → `` `wren/wren-web` ``.

`tools/check-layering.sh`:
```bash
WREN_DIRS=(wren)
```

`wren/wren-web/app/scripts/build-debug-wasm.sh` needs **no** change: `$APP/../../wren-web-debug` still resolves, because the two dirs moved together.

- [ ] **Step 5: Update the other repo** (`~/GitHub/deluge-linux-wren`, on a branch):

```bash
cd ~/GitHub/deluge-linux-wren && git switch -c chore/deluge-sdk-wren-paths
```

In `crates/wren-host/Cargo.toml`:
```toml
deluge-wren-core = { path = "../../../deluge-sdk/wren/deluge-wren-core", default-features = false }
wren-web-debug = { path = "../../../deluge-sdk/wren/wren-web-debug" }
```
(Line 22, `deluge-fonts`, is unchanged.) Run `cargo metadata --format-version 1 --manifest-path crates/wren-host/Cargo.toml >/dev/null`. Expected: exit 0. Then run:

```bash
git commit -am "chore: follow deluge-sdk's move of the Wren crates to wren/"
cd ~/GitHub/deluge-sdk
```

- [ ] **Step 6: Update prose path references**

```bash
git grep -lE 'tools/wren-(web|analyzer)|crates/deluge-wren-core' -- ':!docs/superpowers' ':!CHANGELOG.md' ':!*.wasm' ':!*.lock' \
  | xargs sed -i -E 's#tools/wren-(web|analyzer)#wren/wren-\1#g; s#crates/deluge-wren-core#wren/deluge-wren-core#g'
```

In `docs/wren-scripting.md`, change line 145 to `` See `wren/wren-firmware/examples/`: ``. On line 154, replace the stale `` `wren-firmware/wren/prelude.wren` `` with `` `wren/deluge-wren-core/wren/prelude.wren` ``.

Run: `git grep -nE 'tools/wren-|crates/deluge-wren-core|"\.\./wren-sys"|\.\./\.\./\.\./wren-firmware' -- ':!docs/superpowers' ':!CHANGELOG.md' ':!*.wasm' ':!*.lock'`
Expected: no output.

- [ ] **Step 7: Verify**

```bash
tools/check-manifests.sh
tools/check-layering.sh
cargo build-wren
cargo test --target x86_64-unknown-linux-gnu -p deluge-wren-core
cargo test --target armv7-unknown-linux-gnueabihf -p deluge-wren-core
cargo test-wren-debug
cargo check --manifest-path wren/wren-web/Cargo.toml --target wasm32-wasip1
cargo check --manifest-path wren/wren-analyzer-wasm/Cargo.toml --target wasm32-unknown-unknown
(cd wren/wren-web/app && npm run build)
```

Expected: every command succeeds (the wasm checks need their targets installed; if a target is missing, note it as skipped rather than failed). `audio_bindings` compiling proves the `include_str!` path; the host test build proves `wren-sys`'s `../../../wren-rs` fallback (with `WREN_SRC` unset).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "chore(layout): group the Wren subsystem under wren/"
```

---

### Task 5: Move `deluge-simulator` into `crates/`

`deluge-sdk` depends on it (feature `sim`), so it's a library, not a tool.

**Files:**
- Move: `tools/deluge-simulator/` → `crates/deluge-simulator/`
- Modify: `Cargo.toml` (`exclude`), `crates/deluge-sdk/Cargo.toml:76`, `crates/deluge-simulator/Cargo.toml:29,31`, and prose references

**Interfaces:**
- Consumes: `tools/check-manifests.sh` (Task 4).

- [ ] **Step 1: Move, then confirm the manifest check fails**

```bash
mv tools/deluge-simulator crates/deluge-simulator
git add -A -- tools/deluge-simulator crates/deluge-simulator
tools/check-manifests.sh
```

Expected: `FAIL` for `crates/deluge-sdk/Cargo.toml` (or the workspace root), because the path dependency is missing.

- [ ] **Step 2: Fix paths**

`Cargo.toml` `exclude`: `"tools/deluge-simulator",` → `"crates/deluge-simulator",`

`crates/deluge-sdk/Cargo.toml:76`:
```toml
deluge-simulator = { path = "../deluge-simulator", optional = true }
```

`crates/deluge-simulator/Cargo.toml`:
```toml
deluge-protocol = { path = "../deluge-protocol", features = ["std"] }
deluge-sim-link = { path = "../deluge-sim-link" }
```

Prose:
```bash
git grep -l 'tools/deluge-simulator' -- ':!docs/superpowers' ':!CHANGELOG.md' ':!*.lock' \
  | xargs sed -i 's#tools/deluge-simulator#crates/deluge-simulator#g'
```

- [ ] **Step 3: Verify**

```bash
tools/check-manifests.sh
cargo test --target x86_64-unknown-linux-gnu -p deluge-sdk --features sim
cargo build --manifest-path crates/deluge-simulator/Cargo.toml --target x86_64-unknown-linux-gnu
git grep -n 'tools/deluge-simulator' -- ':!docs/superpowers' ':!CHANGELOG.md' ':!*.lock'
```

Expected: the first three succeed; the grep prints nothing. (`include_bytes!("../assets/Deluge.svg")` is relative to the crate, so it moved with it.)

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(layout): move deluge-simulator from tools/ to crates/"
```

---

### Task 6: `app-loader` → `firmwares/`, hardware probes → `hw-tests/`

**Files:**
- Move: `app-loader/` → `firmwares/app-loader/`; `firmwares/{wp-probe,vbus-probe,sd-bench,uac-host-firmware}/` → `hw-tests/`
- Modify: `Cargo.toml` (members), `firmwares/app-loader/Cargo.toml:13,14,17,18`, `firmwares/app-loader/README.md:96,97`, `docs/app-loader.md:287`, `docs/getting-started.md:46`, `tools/cargo-deluge/src/main.rs:52`, `.cargo/config.toml:53`

**Interfaces:**
- Consumes: `tools/check-manifests.sh` (Task 4).

- [ ] **Step 1: Move, then confirm the manifest check fails**

```bash
mv app-loader firmwares/app-loader
mkdir hw-tests
mv firmwares/wp-probe firmwares/vbus-probe firmwares/sd-bench firmwares/uac-host-firmware hw-tests/
git add -A -- app-loader firmwares hw-tests
tools/check-manifests.sh
```

Expected: `FAIL` for the workspace root (members not found).

- [ ] **Step 2: Fix paths**

`Cargo.toml` `members`: `"app-loader",` → `"firmwares/app-loader",`; `"firmwares/wp-probe",` → `"hw-tests/wp-probe",`; `"firmwares/sd-bench",` → `"hw-tests/sd-bench",`; `"firmwares/vbus-probe",` → `"hw-tests/vbus-probe",`; `"firmwares/uac-host-firmware",` → `"hw-tests/uac-host-firmware",`.

`firmwares/app-loader/Cargo.toml` (one level deeper now):
```toml
rza1l-hal = { path = "../../crates/rza1l-hal" }
deluge-alloc = { path = "../../crates/deluge-alloc" }
deluge-bsp = { path = "../../crates/deluge-bsp", features = ["flash", "fat"] }
deluge-image = { path = "../../crates/deluge-image" }
```

`firmwares/app-loader/README.md`: `(../docs/device-setup.md)` → `(../../docs/device-setup.md)`, and `(../README.md)` → `(../../README.md)`.

The probes' `../../crates/…` paths are unchanged (same depth).

`docs/app-loader.md:287` and `docs/getting-started.md:46`: `` [`app-loader/README.md`](../app-loader/README.md) `` → `` [`firmwares/app-loader/README.md`](../firmwares/app-loader/README.md) ``.

`tools/cargo-deluge/src/main.rs:52`: `` `app-loader/src/devupload.rs` `` → `` `firmwares/app-loader/src/devupload.rs` ``.

`.cargo/config.toml:53`: `firmwares/uac-host-firmware/Cargo.toml` → `hw-tests/uac-host-firmware/Cargo.toml`.

- [ ] **Step 3: Verify**

```bash
tools/check-manifests.sh
cargo build-app-loader
cargo build-uac
cargo build-fw -p wp-probe
cargo build-fw -p vbus-probe
cargo build-fw -p sd-bench
git grep -nE '(^|[^a-z/-])app-loader/|firmwares/(wp-probe|vbus-probe|sd-bench|uac-host-firmware)' -- ':!docs/superpowers' ':!CHANGELOG.md' ':!*.lock'
```

Expected: all builds succeed. The grep prints only `README.md:181` (the layout table, rewritten in Task 7).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(layout): move app-loader into firmwares/, hardware probes into hw-tests/"
```

---

### Task 7: Linux crates → `linux/`, examples → `examples/{baremetal,linux}/`, layout docs

**Files:**
- Move: `crates/{deluge-sys,deluge-hal-linux,deluge-linux-ui}/` → `linux/`; `examples/*/` → `examples/baremetal/`; `examples-linux/` → `examples/linux/`
- Modify: `Cargo.toml` (members + exclude), `crates/deluge-sdk/Cargo.toml:77`, `linux/deluge-linux-ui/Cargo.toml:10,11`, `examples/baremetal/*/Cargo.toml`, `examples/baremetal/additive_osc/build.rs:14,56`, `examples/linux/*/Cargo.toml`, `.gitignore:33-36`, `tools/build-examples.sh:26`, `tools/cargo-deluge/src/new.rs:10-12`, `README.md` (examples links + layout table), `docs/getting-started.md`, `docs/advanced-guide.md:425`, `crates/deluge-sdk/src/audio.rs:54`, `examples/linux/README.md:8,38,57`, `CHANGELOG.md`
- Modify (other repo): `~/GitHub/deluge-ndk/docs/building-an-app.md:161`

**Interfaces:**
- Consumes: `tools/check-manifests.sh` (Task 4).

- [ ] **Step 1: Move, then confirm the manifest check fails**

```bash
mkdir linux
mv crates/deluge-sys crates/deluge-hal-linux crates/deluge-linux-ui linux/
mkdir examples/baremetal
for d in examples/*/; do
  n="$(basename "$d")"
  [ "$n" = baremetal ] || mv "examples/$n" "examples/baremetal/$n"
done
mv examples-linux examples/linux
git add -A -- crates linux examples examples-linux
tools/check-manifests.sh
```

Expected: `FAIL` lines (workspace members, the `deluge-sdk` → `deluge-hal-linux` path, example deps).

- [ ] **Step 2: Fix manifests and scripts**

```bash
# Workspace members / exclude
sed -i -E 's#"examples/#"examples/baremetal/#; s#"examples-linux/#"examples/linux/#; s#"crates/deluge-(sys|hal-linux|linux-ui)"#"linux/deluge-\1"#' Cargo.toml
# Bare-metal examples are one level deeper
sed -i 's#path = "\.\./\.\./crates/#path = "../../../crates/#' examples/baremetal/*/Cargo.toml
# Linux examples: SDK crates one level deeper, linux crates now under linux/
sed -i -E 's#path = "\.\./\.\./crates/deluge-(hal-linux|linux-ui)"#path = "../../../linux/deluge-\1"#; s#path = "\.\./\.\./crates/#path = "../../../crates/#' examples/linux/*/Cargo.toml
```

`crates/deluge-sdk/Cargo.toml:77`:
```toml
deluge-hal-linux = { path = "../../linux/deluge-hal-linux", optional = true }
```

`linux/deluge-linux-ui/Cargo.toml` (`deluge-hal-linux = { path = "../deluge-hal-linux" }` stays):
```toml
deluge-ui-toolkit = { path = "../../crates/deluge-ui-toolkit" }
deluge-grid-toolkit = { path = "../../crates/deluge-grid-toolkit" }
```

`linux/deluge-hal-linux/Cargo.toml`: `deluge-sys = { path = "../deluge-sys" }` is unchanged.

`examples/baremetal/additive_osc/build.rs`: line 56 becomes `.unwrap_or_else(|_| manifest_dir.join("../../../../argon"));` and the line-14 comment's `` `../../../argon` `` becomes `` `../../../../argon` ``.

`.gitignore`, last four lines:
```gitignore
/linux/deluge-sys/Cargo.lock
/linux/deluge-hal-linux/Cargo.lock
/linux/deluge-linux-ui/Cargo.lock
examples/linux/*/Cargo.lock
```

`tools/build-examples.sh:26`:
```bash
for dir in examples/baremetal/*/; do
```

`tools/cargo-deluge/src/new.rs:10-12`:
```rust
const TPL_BUILD_RS: &str = include_str!("../../../examples/baremetal/blinky/build.rs");
const TPL_MEMORY_X: &str = include_str!("../../../examples/baremetal/blinky/memory.x");
const TPL_MEMORY_RTT_X: &str = include_str!("../../../examples/baremetal/blinky/memory_rtt.x");
```

- [ ] **Step 3: Fix doc and comment references**

```bash
sed -i -E 's#\(\.\./examples/#(../examples/baremetal/#g; s#`examples/([a-z_]+)`#`examples/baremetal/\1`#g' docs/getting-started.md
sed -i -E 's#\(examples/([a-z_]+)/\)#(examples/baremetal/\1/)#g' README.md
sed -i 's#`examples/audio_passthru_irq`#`examples/baremetal/audio_passthru_irq`#' docs/advanced-guide.md
sed -i 's#`examples/additive_osc`#`examples/baremetal/additive_osc`#' crates/deluge-sdk/src/audio.rs
sed -i 's#examples-linux/#examples/linux/#g' examples/linux/README.md
```

Leave the comments in `examples/linux/rust-app/src/main.rs:1` and `examples/linux/snake/src/ui.rs:2` alone: they refer to examples in *other* repos (the C `examples/app`, `examples/launcher`).

Then rewrite the `## Repository layout` table in `README.md` (currently lines 174–185) as:

```markdown
| Path | Crate | What |
|------|-------|------|
| `crates/deluge-sdk` | `deluge-sdk` (imported as `deluge`) | the SDK facade + `#[deluge::app]` runtime |
| `crates/deluge-sdk-macros` | `deluge-sdk-macros` | the `#[deluge::app]` proc-macro |
| `crates/deluge-bsp` | `deluge-bsp` | board support: peripherals, PIC, OLED, SD, … |
| `crates/rza1l-hal` | `rza1l-hal` | RZ/A1L hardware abstraction layer |
| `crates/deluge-fixedpoint` (imported as `fixedpoint`), `crates/armv7-dsp-intrinsics` | | fixed-point DSP math + ARMv7 intrinsics |
| `crates/deluge-simulator`, `crates/deluge-sim-link`, `crates/deluge-protocol` | | the desktop simulator and its link/wire protocol |
| `crates/deluge-ui-toolkit`, `crates/deluge-grid-toolkit` | | OLED menu/text and pad-grid UI toolkits (GPL) |
| `crates/deluge-fonts` | `deluge-fonts` | bitmap fonts for the toolkit (GPL) |
| `linux/` | | the native Linux backend: `deluge-sys` (libdeluge FFI), `deluge-hal-linux`, `deluge-linux-ui` |
| `wren/` | | the Wren scripting subsystem: VM (`wren-sys`), bindings (`deluge-wren-core`), `wren-firmware`, and the web editor/debugger |
| `firmwares/` | | standalone firmware images: the app-loader (second-stage bootloader), demo, controller, MSC |
| `hw-tests/` | | single-purpose hardware bring-up / validation probes |
| `examples/baremetal/`, `examples/linux/` | | SDK example apps for each backend |
| `tools/cargo-deluge/` | | the `cargo deluge` host subcommand |
```

(The old row for `crates/deluge-fft` is dropped: that crate moved to flare.)

`CHANGELOG.md` `[Unreleased]`: add a `### Changed` section after `### Added`:

```markdown
### Changed

- Repository layout: the Wren subsystem now lives under `wren/`, the Linux
  backend crates under `linux/`, the app-loader under `firmwares/`, hardware
  probes under `hw-tests/`, the desktop simulator under `crates/`, and examples
  under `examples/baremetal/` and `examples/linux/`. Package names are unchanged.
```

Update the other repo: in `~/GitHub/deluge-ndk/docs/building-an-app.md:161`, change `` `examples-linux/` `` to `` `examples/linux/` `` in deluge-sdk. Commit it on a branch there (`docs: follow deluge-sdk's examples/linux move`).

- [ ] **Step 4: Verify**

```bash
tools/check-manifests.sh
tools/build-examples.sh
cargo test --target x86_64-unknown-linux-gnu --manifest-path tools/cargo-deluge/Cargo.toml
tools/test.sh
git grep -nP 'examples-linux|crates/deluge-(sys|hal-linux|linux-ui)|\.\./examples/(?!baremetal/|linux/)|\(examples/(?!baremetal/|linux/)[a-z]' -- ':!docs/superpowers' ':!CHANGELOG.md' ':!*.lock' ':!*.wasm'
```

Expected: every command succeeds, and the final grep prints nothing. `tools/build-examples.sh` building `blinky` and `cargo-deluge`'s tests compiling cover the `new.rs` template paths.

If `DELUGE_SDK_ROOT` points at a deluge-linux bundle on this machine, also run `cargo check --manifest-path examples/linux/snake/Cargo.toml --target armv7-unknown-linux-musleabihf`. Expected: success. Without a bundle, `check-manifests.sh` is the check for the Linux crates.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore(layout): linux/ backend crates, examples/{baremetal,linux}, refresh layout docs"
```

- [ ] **Step 6: Final sweep across all tasks**

```bash
tools/check-layering.sh && tools/check-manifests.sh
cargo build-wren && cargo build-app-loader && cargo build-fw -p demo-firmware
git status --short   # expect clean
ls -d */             # expect: crates examples firmwares hw-tests linux tools vendor wren (+ untracked target/ node_modules/)
```
