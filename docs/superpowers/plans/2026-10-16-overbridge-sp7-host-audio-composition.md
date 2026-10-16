# Overbridge sub-project 7: host audio composition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One firmware image chooses between UAC and Overbridge at runtime. A `HostAudioMode`
(Auto / UAC only / Overbridge only / Off) decides which audio driver binds a root-port device.
Changing it while a device is attached re-binds the device through a port reset. Apps read
whichever driver is bound through one `HostAudio` handle.

**Architecture:**
- **BSP features:**
  - `usb-host-core`: supervisor, hubs, MIDI.
  - `usb-host-uac`: core + UAC.
  - `usb-host-overbridge`: core + Overbridge.
  - `usb-host` = `usb-host-uac`, today's meaning, kept for one release.
- **Policy:** a pure `audio_drivers(mode, vid) -> &'static [AudioDriver]` gives the binding order.
  The supervisor tries each driver the build includes, in that order.
- **Re-bind:** root-device tasks (its MIDI task and its audio task) each also wait on a stop
  signal. On a mode change, the supervisor:
  1. signals them and gives them 20 ms to drop their pipes;
  2. frees the address;
  3. calls `Rusb1HostDriver::bus_reset`, which returns the device's speed;
  4. enumerates again under the new mode.

  A hub on the root port ignores mode changes, since its children only bind MIDI.
- **App API:** `host_audio() -> HostAudio` (None / Uac / Overbridge) and
  `host_audio_connection() -> u32`, bumped on every audio bind.

**Spec:** `docs/superpowers/specs/2026-10-15-overbridge-design.md` §5.6.

## Global Constraints

- **Auto** = Overbridge when its match succeeds, otherwise UAC. **Uac only** and **Overbridge
  only** never try the other. **Off** binds no audio, and MIDI still binds.
- A mode the build can't honour behaves as Off and logs once.
- Binding order on the root port: MIDI, then the audio drivers in policy order.
- Changing the mode with a device attached re-binds it through a port reset, not in place.
- `usb-host` keeps enabling UAC for one release (CHANGELOG note).

## Spec deviation (ruling, recorded here and in the ledger)

§5.6's shared `HostAudioArena` is **not** built in this plan.
- **What it would save:** the UAC capture and playback rings are 2 × 512 frames × 8 channels ×
  4 bytes = 32 KB, and sharing them with Overbridge's 100 KB rings saves at most those 32 KB.
- **What it would cost:** borrowed-storage variants of both ring types, and unsafe aliasing of
  one static between two drivers' types.
- **Cost if wrong:** a board with both drivers carries ~32 KB more static RAM than necessary.
  Revisit if UAC's ring grows.

## Review Focus

1. **A mode change while a hub is on the root port** must not reset the hub or touch its
   children. Pinned by review (supervisor code); the policy doesn't apply to hubs.
2. **A mode change while nothing is attached** must just record the new mode, so the next attach
   uses it. Covered: the supervisor only watches for mode changes after binding.
3. **A sink-only MIDI device's task** (write loop, no read) must still stop on a re-bind. Pinned
   by review (it gets the stop branch too).
4. **A mode the build lacks** (Overbridge only, without the feature) must bind no audio, never
   fall back to UAC. Pinned by `a_mode_the_build_lacks_binds_no_audio`.
5. **`host_audio()` while nothing is bound** must return `None` and never stale data from the
   previous device. Covered by the drivers' `end()` calls on task exit.

---

### Task 1: Feature split

**Files:**
- Modify: `crates/deluge-bsp/Cargo.toml`, `crates/deluge-bsp/src/usb/mod.rs`,
  `crates/deluge-bsp/src/usb/host.rs`, `tools/test.sh`

- [ ] **Step 1: Features**

Replace the `usb-host` and `usb-host-overbridge` feature lines with:

```toml
## The USB host stack's core (`usb::host`): the supervisor, hubs and the USB-MIDI class.
usb-host-core = ["dep:embassy-usb-host", "usb-classes/host-midi"]
## The UAC host class on top of `usb-host-core`: a class-compliant audio interface streams to
## and from the Deluge.
usb-host-uac = ["usb-host-core", "usb-classes/host-uac"]
## The Elektron Overbridge 2 host class on top of `usb-host-core` (`usb::host::overbridge()`):
## a Digitakt or another Overbridge device streams its stems to the Deluge.
usb-host-overbridge = ["usb-host-core", "usb-classes/host-overbridge"]
## The host stack with UAC, as before `usb-host-core` existed. Kept for one release; it will
## become `usb-host-core`'s meaning, so name `usb-host-uac` if you want UAC.
usb-host = ["usb-host-uac"]
```

- [ ] **Step 2: Gates**

- In `usb/mod.rs`: `#[cfg(feature = "usb-host")]` → `#[cfg(feature = "usb-host-core")]`.
- In `usb/host.rs`: gate everything UAC-specific on `#[cfg(feature = "usb-host-uac")]`. That
  covers `set_uac_sample_rate`, `uac_sample_rate` and their static, `bind_uac`,
  `uac_capture_task`, the UAC test, and the `bind_uac` call. Leave the rest under the module's
  core gate.

In the supervisor, the composite bind becomes the policy loop that Task 2 adds. For now, keep
the existing expressions with `#[cfg]`s so that each of the three features builds alone.

- [ ] **Step 3: Build every combination**

```bash
for f in usb-host-core usb-host-uac usb-host-overbridge usb-host "usb-host-uac,usb-host-overbridge"; do
  echo "== $f"
  cargo build -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem --release -p deluge-bsp --features "$f" --target armv7a-none-eabihf 2>&1 | grep -E "^error|^warning: unused|Finished"
done
```
Expected: each finishes with no new warnings. The existing `RegisterError` unused-import warning
is pre-existing.

In `tools/test.sh`, change the deluge-bsp test features to
`usb-host-uac,usb-host-overbridge`. Run that line.

- [ ] **Step 4: Commit**

```bash
git add crates/deluge-bsp tools/test.sh
git commit -m "deluge-bsp: split usb-host into usb-host-core, usb-host-uac and usb-host-overbridge"
```

---

### Task 2: `HostAudioMode` and the binding policy

**Files:**
- Modify: `crates/deluge-bsp/src/usb/host.rs`

**Interfaces:**
- Produces:
  - `pub enum HostAudioMode { Auto, Uac, Overbridge, Off }`, `#[repr(u8)]`, `Default = Auto`;
  - `pub fn set_host_audio_mode(HostAudioMode)` and `pub fn host_audio_mode() -> HostAudioMode`;
  - `pub enum AudioDriver { Uac, Overbridge }`;
  - `pub fn audio_drivers(mode, vid) -> &'static [AudioDriver]`, the policy before the build's
    features filter it;
  - `pub fn compiled(d: AudioDriver) -> bool`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn auto_offers_elektron_devices_to_overbridge_first() {
        assert_eq!(audio_drivers(HostAudioMode::Auto, 0x1935), &[AudioDriver::Overbridge, AudioDriver::Uac]);
        assert_eq!(audio_drivers(HostAudioMode::Auto, 0x1234), &[AudioDriver::Uac]);
    }

    #[test]
    fn a_forced_mode_never_tries_the_other_driver() {
        assert_eq!(audio_drivers(HostAudioMode::Uac, 0x1935), &[AudioDriver::Uac]);
        assert_eq!(audio_drivers(HostAudioMode::Overbridge, 0x1234), &[AudioDriver::Overbridge]);
        assert!(audio_drivers(HostAudioMode::Off, 0x1935).is_empty());
    }

    #[test]
    fn a_mode_the_build_lacks_binds_no_audio() {
        let usable: std::vec::Vec<_> = audio_drivers(HostAudioMode::Overbridge, 0x1935)
            .iter()
            .filter(|d| compiled(**d))
            .collect();
        assert_eq!(usable.is_empty(), !cfg!(feature = "usb-host-overbridge"));
    }

    #[test]
    fn the_mode_round_trips_and_defaults_to_auto() {
        assert_eq!(HostAudioMode::default(), HostAudioMode::Auto);
        for m in [HostAudioMode::Off, HostAudioMode::Uac, HostAudioMode::Overbridge, HostAudioMode::Auto] {
            set_host_audio_mode(m);
            assert_eq!(host_audio_mode(), m);
        }
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host-uac,usb-host-overbridge --lib usb::host 2>&1 | grep -E "^error" | sort | uniq -c
```
Expected: compile errors.

- [ ] **Step 3: Implement**

In `usb/host.rs`, outside `runtime`:

```rust
/// Which audio class driver binds a root-port device (spec §5.6). Change it any time; with a
/// device attached the supervisor re-binds it.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostAudioMode {
    /// Overbridge when the device matches it, UAC otherwise.
    #[default]
    Auto = 0,
    /// Only UAC.
    Uac = 1,
    /// Only Overbridge.
    Overbridge = 2,
    /// No audio driver; MIDI still binds.
    Off = 3,
}

/// An audio class driver the supervisor can bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDriver {
    Uac,
    Overbridge,
}

static HOST_AUDIO_MODE: AtomicU8 = AtomicU8::new(HostAudioMode::Auto as u8);

/// Set the host audio mode. With a device attached, it is re-bound under the new mode.
pub fn set_host_audio_mode(mode: HostAudioMode) {
    if HOST_AUDIO_MODE.swap(mode as u8, Ordering::AcqRel) != mode as u8 {
        #[cfg(target_os = "none")]
        runtime::MODE_CHANGED.signal(());
    }
}

/// The current host audio mode.
pub fn host_audio_mode() -> HostAudioMode {
    match HOST_AUDIO_MODE.load(Ordering::Acquire) {
        1 => HostAudioMode::Uac,
        2 => HostAudioMode::Overbridge,
        3 => HostAudioMode::Off,
        _ => HostAudioMode::Auto,
    }
}

/// The audio drivers to try for a device with vendor ID `vid` under `mode`, in order, before
/// the build's features filter them ([`compiled`]).
pub fn audio_drivers(mode: HostAudioMode, vid: u16) -> &'static [AudioDriver] {
    const ELEKTRON: u16 = 0x1935;
    match mode {
        HostAudioMode::Auto if vid == ELEKTRON => &[AudioDriver::Overbridge, AudioDriver::Uac],
        HostAudioMode::Auto | HostAudioMode::Uac => &[AudioDriver::Uac],
        HostAudioMode::Overbridge => &[AudioDriver::Overbridge],
        HostAudioMode::Off => &[],
    }
}

/// Whether this build includes `d`.
pub fn compiled(d: AudioDriver) -> bool {
    match d {
        AudioDriver::Uac => cfg!(feature = "usb-host-uac"),
        AudioDriver::Overbridge => cfg!(feature = "usb-host-overbridge"),
    }
}
```

`is_overbridge_candidate` becomes redundant with the policy. Remove it and its test, and keep
the handle-size test. Import `AtomicU8` and `Ordering` from `core::sync::atomic`, as the file
does for the other atomics.

In `runtime`, add `pub(super) static MODE_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();`
(`embassy_sync::signal::Signal`).

Replace the composite bind's audio expression with:

```rust
                        let midi = bind_midi(&handle, spawner, &dev_info, &cfg, Some(&ROOT_MIDI_STOP));
                        let mode = super::host_audio_mode();
                        let vid = dev_info.device_desc.vendor_id;
                        let mut audio = false;
                        for d in super::audio_drivers(mode, vid) {
                            if !super::compiled(*d) {
                                warn_once_missing(mode);
                                continue;
                            }
                            audio = match d {
                                #[cfg(feature = "usb-host-overbridge")]
                                super::AudioDriver::Overbridge => bind_overbridge(&handle, spawner, &dev_info, &cfg),
                                #[cfg(feature = "usb-host-uac")]
                                super::AudioDriver::Uac => bind_uac(&handle, spawner, &dev_info, &cfg),
                                #[allow(unreachable_patterns)]
                                _ => false,
                            };
                            if audio {
                                break;
                            }
                        }
                        midi || audio
```

`ROOT_MIDI_STOP` comes in Task 3. For this task, pass nothing extra: keep `bind_midi`'s current
signature, and add the stop argument in Task 3. `warn_once_missing` is a small helper with a
`static AtomicBool` that logs "host audio mode {:?} needs a driver this build doesn't include"
once.

- [ ] **Step 4: Run the tests, build, commit**

Run the Step 2 command with `| tail -3`. Expected: pass. Then rebuild the five feature
combinations from Task 1 Step 3. Expected: they all build.

```bash
git add crates/deluge-bsp/src/usb/host.rs
git commit -m "deluge-bsp: HostAudioMode and the audio binding policy"
```

---

### Task 3: Re-bind on a mode change

**Files:**
- Modify: `crates/deluge-bsp/src/usb/host.rs` (`runtime`)
- Modify: `docs/hardware-tasks.md`

- [ ] **Step 1: Stop signals**

In `runtime`:

```rust
    /// Stops the root-port device's MIDI task, so the device can be re-bound.
    static ROOT_MIDI_STOP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
    /// Stops the root-port device's audio task.
    static ROOT_AUDIO_STOP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
```

- **`bind_midi`** gains `stop: Option<&'static Signal<CriticalSectionRawMutex, ()>>` and passes
  it to `midi_device_task`. The root port passes `Some(&ROOT_MIDI_STOP)`; hub ports pass `None`.
- **`midi_device_task`** gains the same parameter. Wrap each of its two awaits (the sink-only
  `tx.receive()`, and the `select(read, receive)`) in a `select` with
  `async { match stop { Some(s) => s.wait().await, None => core::future::pending().await } }`.
  When the stop wins, return.
- **`uac_capture_task` and `overbridge_task`:** wrap their pump calls in
  `select(pump, ROOT_AUDIO_STOP.wait())`. On the stop, break out of the loop; each task's
  existing end-of-stream cleanup (`shared::end()` / `stems.end()`) still runs.
- **At root bind time,** before binding, call `ROOT_MIDI_STOP.reset()` and
  `ROOT_AUDIO_STOP.reset()` so a stale signal doesn't stop the new tasks.

- [ ] **Step 2: The supervisor**

Restructure the supervisor's loop head and its post-bind wait:

```rust
        let mut rebind: Option<Speed> = None;
        loop {
            let speed = match rebind.take() {
                Some(s) => s,
                None => controller.wait_for_connection().await,
            };
            ...
            // Wait for detach, or for a mode change to re-bind a non-hub device.
            loop {
                match select(controller.wait_for_device_event(), MODE_CHANGED.wait()).await {
                    Either::First(DeviceEvent::Disconnected) => {
                        info!("usb_host: device disconnected");
                        handle.free_address(addr);
                        break;
                    }
                    Either::First(_) => {}
                    Either::Second(()) if is_hub => {
                        info!("usb_host: host audio mode changed; a hub is attached, nothing to re-bind");
                    }
                    Either::Second(()) => {
                        info!("usb_host: host audio mode now {:?}; re-binding", super::host_audio_mode());
                        ROOT_MIDI_STOP.signal(());
                        ROOT_AUDIO_STOP.signal(());
                        // The tasks drop their pipes on their next poll.
                        embassy_time::Timer::after_millis(20).await;
                        handle.free_address(addr);
                        rebind = controller.controller().bus_reset().await;
                        break;
                    }
                }
            }
        }
```

Here `is_hub` is the `has_hub_interface` result, kept in a `let`. `Speed` is
`embassy_usb_driver::Speed`; use whatever type `wait_for_connection` returns.
`controller.controller()` is `&Rusb1HostDriver`, whose inherent `bus_reset(&self)` returns
`Option<Speed>`. Check the exact name; the trait method of the same name returns `()`, so call
it as `Rusb1HostDriver::bus_reset(controller.controller())`. `MODE_CHANGED` set while idle is
consumed at the next post-bind wait. Call `MODE_CHANGED.reset()` right after each bind, so a
change made before the attach doesn't re-bind at once.

- [ ] **Step 3: Build every combination**

Run Task 1 Step 3's loop. Expected: all build.

- [ ] **Step 4: Hardware task, commit**

Add to `docs/hardware-tasks.md`:

```markdown
## Host audio mode switching (Overbridge sub-project 7)

- **Setup:** a host build with `usb-host-uac,usb-host-overbridge` and a way to call
  `set_host_audio_mode` (a button handler or a debugger write). An Elektron box in Overbridge
  mode; then a class-compliant audio interface.
- **Pass:** with the Elektron box in Auto, Overbridge binds. Switching to Uac only re-binds
  within about a second, with no audio (its alt 3 isn't UAC); back to Auto re-binds Overbridge.
  Off drops audio but keeps MIDI. With the audio interface, Uac and Auto bind it and Overbridge
  only and Off don't. Switching while a hub is attached changes nothing.
- **Record:** the re-bind time, and anything that needed a replug.
```

```bash
git add crates/deluge-bsp/src/usb/host.rs docs/hardware-tasks.md
git commit -m "deluge-bsp: a host audio mode change re-binds the root-port device through a port reset"
```

---

### Task 4: The `HostAudio` handle

**Files:**
- Modify: `crates/deluge-bsp/src/usb/host.rs`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Produces:
  - `pub enum HostAudio { None, #[cfg(feature = "usb-host-uac")] Uac(UacInput), #[cfg(feature = "usb-host-overbridge")] Overbridge(&'static OverbridgeStems) }`;
  - `pub struct UacInput` with `channels()`, `read(&mut [f32]) -> usize`, `playback_channels()`,
    `write(&[f32]) -> usize`;
  - `pub fn host_audio() -> HostAudio`;
  - `pub fn host_audio_connection() -> u32`.

- [ ] **Step 1: Implement** (device-only; `usb_classes::host::uac::shared` exists only on the target)

```rust
/// Counts audio binds, so an app can tell the device or driver changed between two reads.
static HOST_AUDIO_CONNECTION: AtomicU32 = AtomicU32::new(0);

/// The bound audio driver's input, for an app built once to follow a mode change live.
#[cfg(target_os = "none")]
pub enum HostAudio {
    None,
    #[cfg(feature = "usb-host-uac")]
    Uac(UacInput),
    #[cfg(feature = "usb-host-overbridge")]
    Overbridge(&'static OverbridgeStems),
}

/// A bound UAC device: interleaved `f32` at the engine rate (its resamplers convert).
#[cfg(all(target_os = "none", feature = "usb-host-uac"))]
pub struct UacInput;

#[cfg(all(target_os = "none", feature = "usb-host-uac"))]
impl UacInput {
    pub fn channels(&self) -> u8 { usb_classes::host::uac::shared::capture_channels() }
    pub fn read(&self, out: &mut [f32]) -> usize { usb_classes::host::uac::shared::capture_read(out) }
    pub fn playback_channels(&self) -> u8 { usb_classes::host::uac::shared::playback_channels() }
    pub fn write(&self, samples: &[f32]) -> usize { usb_classes::host::uac::shared::playback_write(samples) }
}

/// Whichever audio driver is bound now.
#[cfg(target_os = "none")]
pub fn host_audio() -> HostAudio {
    #[cfg(feature = "usb-host-overbridge")]
    if overbridge().info().is_some() {
        return HostAudio::Overbridge(overbridge());
    }
    #[cfg(feature = "usb-host-uac")]
    if usb_classes::host::uac::shared::capture_channels() > 0 {
        return HostAudio::Uac(UacInput);
    }
    HostAudio::None
}

/// Bumped on every audio bind.
pub fn host_audio_connection() -> u32 {
    HOST_AUDIO_CONNECTION.load(Ordering::Acquire)
}
```

Bump `HOST_AUDIO_CONNECTION` (`fetch_add(1, Release)`) in `bind_uac` and `bind_overbridge`
when they return `true`.

- [ ] **Step 2: Build, CHANGELOG, commit**

Run Task 1 Step 3's loop. Expected: all build. CHANGELOG `### Changed`:

```markdown
- `deluge-bsp`: `usb-host` is split into `usb-host-core` (supervisor, hubs, MIDI),
  `usb-host-uac` and `usb-host-overbridge`. `usb-host` still means `usb-host-uac` for this
  release; it will become `usb-host-core`, so name `usb-host-uac` if you want UAC.
```

Under `### Added`:

```markdown
- `deluge-bsp` `usb::host`: `HostAudioMode` (Auto / Uac / Overbridge / Off), set at runtime with
  `set_host_audio_mode`; changing it re-binds an attached device. `host_audio()` returns
  whichever audio driver is bound, and `host_audio_connection()` counts binds.
```

```bash
git add crates/deluge-bsp/src/usb/host.rs CHANGELOG.md
git commit -m "deluge-bsp: HostAudio, one handle for whichever host audio driver is bound"
```

- [ ] **Step 3: Suites**

Run `tools/test.sh`. The known flare exception applies; confirm the deluge-bsp line passed.
