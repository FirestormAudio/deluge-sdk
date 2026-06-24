# Wren scripting on the Deluge

`wren-firmware` turns the Synthstrom Deluge into a live-codable instrument: you
write [Wren](https://wren.io) scripts that drive the Deluge's CV/gate jacks, MIDI,
the pad/encoder/OLED surface, and a native audio synthesis engine.

The upstream Wren VM (compiler included) runs **on the device**, so scripts compile
and run live. All hardware is exposed through `foreign` classes backed by native
Rust; the `prelude.wren` defining them is compiled into the `main` module at boot,
so everything below is always in scope.

## Connecting

The device enumerates as a USB-CDC serial port (e.g. `/dev/ttyACM0`). Connect with
any terminal — local echo recommended:

```
picocom -c -b 115200 /dev/ttyACM0
```

### REPL protocol

- A plain line is evaluated immediately in the `main` module. Module-level `var`s
  persist across lines, so state accumulates (it's a stateful REPL).
- `^^s` — begin a multi-line script upload; subsequent lines accumulate.
- `^^e` — end upload and run (no save).
- `^^w` — end upload, run, and save to `/MAIN.WREN` on the SD card.
- `^^c` — cancel/clear the upload buffer.
- `^^v` — print the version banner.

`/MAIN.WREN` (if present) is loaded and run at boot.

## CV / Gate

`output[1]` and `output[2]` are the two CV jacks; `gate[1]`..`gate[4]` the gates.

```wren
output[1].volts = 5.0      // set output voltage (0..~10 V)
output[1].volts            // read current (post-slew) voltage
output[1].slew = 0.25      // seconds to ramp to future targets (0 = instant)
gate[1].on = true          // gate high / low
```

## Timing — `Metro`

```wren
var m = Metro.new()
m.start(Fn.new { |stage| ... }, 0.25)   // callback every 0.25 s; stage = 1,2,3,…
m.time = 0.5                            // change interval while running
m.stop()
```

Up to 8 metros. Callbacks run cooperatively in the VM task.

## MIDI (DIN) — `Midi`

Channels are `1..16`. Transmit:

```wren
Midi.noteOn(1, 60, 100)
Midi.noteOff(1, 60, 0)
Midi.cc(1, 74, 64)
Midi.send(0xF8, 0, 0)        // raw bytes (length inferred from status)
```

Receive (handlers get `channel, data1, data2`; note-on with velocity 0 arrives as
note-off):

```wren
Midi.onNoteOn  = Fn.new { |ch, note, vel| ... }
Midi.onNoteOff = Fn.new { |ch, note, vel| ... }
Midi.onCC      = Fn.new { |ch, num, val| ... }
```

## Surface — pads, buttons, encoders, LEDs, OLED

```wren
Pads.onPress   = Fn.new { |x, y| ... }   // x: 0..17, y: 0..7
Pads.onRelease = Fn.new { |x, y| ... }
Buttons.onPress   = Fn.new { |id| ... }  // id: 0..35
Buttons.onRelease = Fn.new { |id| ... }
Enc.onTurn = Fn.new { |index, delta| ... } // index: 0..5, delta in detents

Led.on(0)            // indicator LED by id
Led.off(0)

Oled.clear()
Oled.text(0, 0, "hello")   // x,y in pixels, 5x7 font (128x48 display)
Oled.pixel(10, 20, true)
Oled.show()                // push the buffer to the screen
```

## Audio — native DSP graph

DSP runs natively at 44.1 kHz; these objects are lightweight handles to native
nodes. **Numbers or nodes** may be used wherever a value is expected, so any
parameter can be modulated by another node.

```wren
Out.patch(Osc.sine(220))                    // 220 Hz sine to the output

var env = Env.ar(0.01, 0.4)                 // attack 10 ms, release 400 ms
Out.patch(Osc.saw(110) * env)               // saw shaped by the envelope
env.trigger()                               // one-shot AR
env.gate(true) / env.gate(false)            // or sustain via gate

Out.patch(Osc.saw(110).lpf(800))            // low-pass filtered saw

var lfo = Osc.sine(5)                        // 5 Hz LFO
var o = Osc.sine(440)
o.freq = lfo                                 // vibrato (param ← node)

Out.reset()                                  // clear the whole graph
```

Sources: `Osc.sine/saw/square/tri(freq)`, `Noise.new()`.
Shaping: `Env.ar(attack, release)`, `node.lpf(cutoff)`.
Combining: `a * b`, `a + b`, `a - b` (operands may be nodes or numbers).
Params: `node.freq = …`, `node.cutoff = …`, `node.gate(bool)`, `node.trigger()`.
Output is mono, duplicated to both channels.

## Examples

See `wren-firmware/examples/`:

- `hello.wren` — print + OLED text.
- `cv_lfo.wren` — metro-driven CV triangle LFO + gate blink.
- `midi_synth.wren` — MIDI → native synth (saw + filter + envelope) and CV/gate.
- `pads.wren` — pad/encoder input → LED, OLED, CV.

## Authoring on a computer

`wren-firmware/wren/prelude.wren` is the single source of truth for the scripting
API — every `foreign` class + signature is declared there. Point the `wren-rs`
toolchain (`~/GitHub/wren-rs`: LSP, analyzer, formatter) at it as a library/context
to get completion, go-to-definition, and diagnostics while editing `.wren` files,
then load them via `^^w` (saved to `/MAIN.WREN`) or the SD card.

## Known limitations (v1)

- **No script watchdog.** An accidental infinite loop in a script/callback hangs
  the VM (and everything it cooperatively shares — MIDI/UI/audio). Recovery is a
  reset. (A future fix adds a deadline check to the interpreter loop.)
- **Audio glitches during big compiles.** Rendering is cooperative; a long
  `wrenInterpret` (large `^^w` upload) can briefly starve the audio task. Small
  REPL edits are fine.
- **Mono audio, single patch.** No polyphony / per-voice allocation yet.
- **MIDI is DIN only** (no USB-MIDI bridge yet). Pad **RGB** colors not yet exposed
  (LEDs are).
