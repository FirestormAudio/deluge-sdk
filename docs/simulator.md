# The Deluge Desktop Simulator

The simulator is a desktop front panel for the Synthstrom Deluge. It renders the
OLED, RGB pad grid, buttons, encoders and LEDs, plays audio through your
speakers, bridges MIDI, and lets you build and debug Deluge SDK apps **without
hardware**.

It runs in two modes:

| Mode | Launched by | What drives it |
| --- | --- | --- |
| **In-process** | `cargo deluge sim` | An SDK app built for the host, sharing state with the panel in one process (no sockets). |
| **External** | `deluge-simulator --connect <target>` | A separate core process, communicating over the `deluge-protocol` wire (TCP / Unix socket), e.g. DelugeFirmware's native `deluge_host`. |

Most app developers only need the first mode. The rest of this manual assumes
in-process (`cargo deluge sim`) unless noted.

---

## 1. Quick start

From inside a Deluge app crate (one made by `cargo deluge new`):

```sh
cargo deluge sim
```

This builds your app for the **host** target (overriding the workspace's bare-
metal `armv7a-none-eabihf`), where the SDK swaps the real peripherals for the
in-process panel, and opens the simulator window. Your `async fn main` runs on a
worker thread; the window owns the main thread (the "panel").

Close the window to quit.

> **Tip:** `RUST_LOG=info cargo deluge sim` surfaces the SDK's `info!`/`warn!`
> logs (the host logger mirrors the device's RTT/USB logger to stderr).

---

## 2. The faceplate

The window shows the Deluge front panel over its SVG artwork: the **128×48 OLED**,
the **18×8 RGB pad grid** (16 main columns + 2 sidebar columns), the buttons, the
six encoders, and the indicator LEDs.

### Mouse input

- **Pads** — click (and drag) on the grid. Dragging off a pad releases it.
- **Buttons** — click. The button's LED lights when the app turns it on.
- **Encoders** — **scroll** over a knob to turn it (mouse wheel = one detent per
  notch; trackpad = smooth). **Click** a knob to push it.

All input is delivered to the app exactly as on hardware — your code sees owned
capabilities, never raw events.

### Sticky keys

Press **`S`** to toggle *sticky keys*: held buttons stay held until clicked
again, so you can hold `SHIFT` (or any modifier) with one hand on the mouse.

### The Volume knob = master output level

The faceplate **Volume** knob is wired to the **simulator's master output
volume**, not to your app. It behaves like a real potentiometer: a fixed ~270°
travel mapping **0–100 %**, starting at full. Scroll it down to attenuate what
reaches your speakers. (The audio scopes always show your app's *true* output,
unaffected by this knob.)

---

## 3. The instrument rack (CV / gate / MIDI / audio strip)

Above the faceplate is a strip that visualises the back-panel jacks — the things
that have no representation on the front panel. Each indicator sits over the port
it represents.

| Indicator | Shows |
| --- | --- |
| **CV 1–2** | Vertical bar meters, 0–10 V (the MAX5136 DAC, ~6552 codes/V), with a volt readout. |
| **GATE 1–4** | Lit dots — amber when the gate is asserted. |
| **MIDI IN / OUT** | Activity dots that flash on traffic and fade over ~0.3 s. |
| **OUT L / R** | Two oscilloscopes — the stereo audio output, one trace per channel. |

The oscilloscopes are ported from spark's analyzer scope: a faint centre line, a
soft glow under the trace, and a rising-edge trigger that holds a periodic signal
still. Each shows a fixed, scrolling time window.

### Two display modes — click to toggle

Click anywhere on the **body** of the strip to toggle the CV/gate indicators
between:

- **Meters** (default) — bar meters + lit dots (at-a-glance state).
- **Scopes** — a rolling oscilloscope trace per CV and gate channel.

(The audio OUT scopes are always shown.)

### Collapsing the rack

Click the small **triangle handle** at the **bottom-left** of the strip to
collapse it to just the handle (and again to restore it). This hides the rack
contents while keeping the faceplate fixed in place.

> On a Wayland compositor the window can't resize/reposition itself, so
> collapsing hides the rack rather than shrinking the window.

---

## 4. Audio

The output callback is the audio clock that paces your app's DSP loop, at the
device's rate: **44.1 kHz, 128-frame stereo blocks**.

- **Output** → your default speakers (scaled by the Volume knob).
- **Input** → your default mic / line-in (or silence if none), *unless* you feed
  a file (below).

### Audio file I/O

For deterministic, repeatable DSP runs (and CI), bridge files instead of live
devices:

```sh
cargo deluge sim --audio-in tone.wav      # feed a WAV as the codec input (looped)
cargo deluge sim --audio-out render.wav   # record the codec output to a WAV
cargo deluge sim --audio-in in.wav --audio-out out.wav   # both
```

- **`--audio-in <file.wav>`** replaces the mic with the WAV, looped. Any
  bit-depth/format and mono or stereo is accepted; a warning is logged if the
  file isn't 44.1 kHz (it plays without resampling). Playback is paced by your
  app's consumption, so it runs at the codec rate.
- **`--audio-out <file.wav>`** records your app's output (pre-Volume-knob) to a
  32-bit-float stereo WAV. The file is **finalised when you close the window**.

Paths are resolved against your shell's working directory.

---

## 5. MIDI

In-process mode opens two **virtual MIDI ports** (ALSA on Linux, CoreMIDI on
macOS):

| Port | Direction | Use |
| --- | --- | --- |
| **Deluge Sim In** | host → app | Connect a keyboard / DAW *to* this port; bytes arrive at your app's `midi().recv()`. The **MIDI IN** dot flashes. |
| **Deluge Sim Out** | app → host | Your app's `midi().send()` bytes appear here for a DAW to receive. The **MIDI OUT** dot flashes. |

No device picking is needed — you wire *your* gear to the sim's ports.

### Example (Linux / ALSA)

```sh
aconnect -l                       # list ports; find "Deluge Sim In"
aplaymidi -p <client>:0 song.mid  # play a MIDI file into the app
aconnect 'My Keyboard':0 'deluge-sim':0   # wire a hardware keyboard in
```

This is what makes the `midi_cv` example (a MIDI→CV/gate converter) run in the
simulator: a note-on raises a gate and sets CV to the note's pitch, visible in
the rack.

> MIDI virtual ports require a Unix platform. On other platforms the bridge is a
> no-op (a log line says so).

---

## 6. SD card

File access (the `sd` capability) is backed by a **host directory**:

```sh
DELUGE_SIM_SD=./my-sd-card cargo deluge sim
```

Defaults to `./sim-sd`. Your app's reads/writes hit real files under that root,
so you can stage `/APPS`, samples, etc.

---

## 7. Headless mode & golden-frame testing

Run an app with **no window**, replay a **script** of input, and dump
**snapshots** — for CI and regression tests.

```sh
cargo deluge sim --headless --script test.txt --out got/
```

- `--headless` runs the scripted driver instead of the GUI.
- `--script <file>` is the timeline to replay (optional; without it the driver
  snapshots once at 500 ms as `frame` and quits).
- `--out <dir>` is where snapshots land (default `.`).

An audio app's DSP loop still runs (a null pacer drains output / feeds silence),
so headless works for audio and UI apps alike. Timing is wall-clock.

### Script format

One step per line: `<at_ms> <command> <args…>`. `#` starts a comment.

```text
# columns: time(ms)  command  args
0     button 35 down        # press PLAY
0     button 35 up
120   encoder 5 +1          # turn SELECT one detent
150   pad 3 5 down          # press pad (x=3, y=5)
300   snapshot after-edit   # dump after-edit.png + after-edit.state
600   quit
```

| Command | Meaning |
| --- | --- |
| `button <id> down\|up` | Press/release a button (raw id — see table below). |
| `pad <x> <y> down\|up` | Press/release a pad. `x` 0–17 (16–17 = sidebar), `y` 0–7. |
| `encoder <index> <delta>` | Turn an encoder by signed detents (`+1`, `-2`, …). |
| `snapshot <name>` | Write `<name>.png` (OLED) and `<name>.state`. |
| `quit` | Stop replaying. |

**Encoder indices**

| index | encoder |
| --- | --- |
| 0 | Horizontal (◄►) |
| 1 | Tempo |
| 2 | Lower gold |
| 3 | Upper gold |
| 4 | Vertical (▲▼) |
| 5 | Select |

(The Volume knob is the sim's master output and isn't scriptable as an app
encoder.)

**Button ids** (for `button <id>`)

| id | button | id | button | id | button |
| --- | --- | --- | --- | --- | --- |
| 1 | Enc-fn 1 | 14 | Quantize | 26 | Record |
| 2 | Enc-fn 5 | 15 | Load | 28 | Enc-fn 4 |
| 3 | Scope | 16 | Back/Undo | 29 | Enc-fn 8 |
| 5 | Time | 17 | Select | 30 | Keyboard |
| 6 | Scale | 19 | Enc-fn 3 | 32 | Transform |
| 7 | Copy | 20 | Enc-fn 7 | 33 | Save |
| 8 | Shift | 21 | Clip | 34 | Tap tempo |
| 10 | Enc-fn 2 | 23 | Automation | 35 | Play |
| 11 | Enc-fn 6 | 24 | Loop | | |
| 12 | Session | 25 | Fill | | |

### Snapshots

Each `snapshot <name>` writes two files under `--out`:

- **`<name>.png`** — the OLED framebuffer as a scaled grayscale PNG (512×192,
  lit pixels white). Byte-for-byte stable for the same frame, so it works as a
  golden image.
- **`<name>.state`** — a diffable text dump of the rest of the panel:

  ```text
  cv 32760 13104 0 0      # the four CV DAC codes
  gate 1 0 0 0            # the four gate states
  synced 0               # the SYNC LED
  led 47                 # each lit LED, by raw index
  pad 3 5 255 0 0        # each non-black pad: x y r g b
  ```

### Using it in CI

```sh
cargo deluge sim --headless --script tests/menu.txt --out got/
diff -r got/ tests/golden/      # fail the build on any mismatch
```

Commit a `tests/golden/` directory of known-good `.png` / `.state` files;
regenerate them with the same command into the golden dir when behaviour changes
intentionally.

---

## 8. External-process mode

The same renderer can drive a separate process over the wire — e.g. DelugeFirmware's
native build:

```sh
DELUGE_HOST_LINK=127.0.0.1:9000 ./build-sim/deluge_host   # the process (TCP)
deluge-simulator --connect 127.0.0.1:9000                 # the panel
```

`--connect` accepts `host:port`, `:port`, `tcp://host:port`, or (on Unix) a
domain-socket path. Omit it for a passive, blank panel. This mode does not run an
SDK app and has no in-process audio/MIDI/file bridges.

---

## 9. Reference

### `cargo deluge sim` flags

| Flag | Effect |
| --- | --- |
| `--release` | Optimised build of the app. |
| `--audio-in <file.wav>` | Feed a WAV as the codec input (looped). |
| `--audio-out <file.wav>` | Record the codec output to a WAV. |
| `--headless` | Run the scripted driver with no window. |
| `--script <file>` | Headless input/snapshot script. |
| `--out <dir>` | Headless snapshot output directory. |

### Environment variables

These are set for you by the `cargo deluge sim` flags above, but you can also set
them directly (e.g. when running a host-built app binary yourself).

| Variable | Meaning | Default |
| --- | --- | --- |
| `DELUGE_SIM_SD` | SD-card root directory | `./sim-sd` |
| `DELUGE_SIM_AUDIO_IN` | Input WAV (looped) | mic |
| `DELUGE_SIM_AUDIO_OUT` | Output WAV to record | — |
| `DELUGE_HEADLESS` | Any value → headless mode | unset (GUI) |
| `DELUGE_SIM_SCRIPT` | Headless script path | snapshot-once |
| `DELUGE_SIM_OUT` | Snapshot output dir | `.` |
| `DELUGE_HOST_LINK` | External process' listen target (external mode) | — |
| `RUST_LOG` | Log level (`error`/`warn`/`info`/`debug`) | quiet |

---

## 10. Platform notes & limitations

- **Linux / macOS** are first-class: audio (cpal), MIDI virtual ports (ALSA /
  CoreMIDI), and headless all work. On other platforms the MIDI bridge is a
  no-op.
- **Wayland**: the window can't move or resize itself, so collapsing the rack
  hides its contents rather than reclaiming space, and the window may be tiled by
  tiling compositors.
- **MIDI input** in your app uses `recv()`, which on the host polls the panel at
  ~1 ms when idle — inaudible for DIN-rate MIDI.
- WAV `--audio-in` does **not** resample: use 44.1 kHz files for correct pitch.

---

## 11. Troubleshooting

| Symptom | Likely cause / fix |
| --- | --- |
| "host panel not initialised" panic | The app was run directly without the simulator runtime — use `cargo deluge sim`. |
| No sound | Check the Volume knob isn't at 0; check `RUST_LOG=info` for an audio-device warning; another app may own the audio device. |
| MIDI dots never light | Nothing is connected to the virtual ports — see `aconnect -l`; on non-Unix the bridge is disabled. |
| `--audio-out` WAV is empty/short | The file is finalised on window close — quit the window cleanly (don't `kill` the process). |
| Headless snapshot is blank | The app may not have rendered yet — snapshot at a later `at_ms`. |
| Window is letterboxed / wrong size | A tiling Wayland compositor tiled it — float the window (compositor rule). |
