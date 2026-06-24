// midi_synth.wren — a monophonic MIDI synth using the native audio engine.
//
// Incoming MIDI notes set the oscillator pitch and trigger an amplitude
// envelope; the saw is run through a low-pass filter. Also mirrors pitch to
// CV jack 1 (1V/oct-ish) and gate 1, so it drives both audio and Eurorack.

var pitch = Osc.saw(110)          // oscillator (frequency set per note)
var filt  = pitch.lpf(1200)       // low-pass filter (cutoff swept by mod wheel)
var amp   = Env.ar(0.005, 0.3)    // attack 5 ms, release 300 ms
Out.patch(filt * amp)             // filtered saw, shaped by the envelope

// MIDI note number → frequency (A4 = 69 = 440 Hz).
var noteToHz = Fn.new { |n| 440.0 * (2.0).pow((n - 69) / 12.0) }

Midi.onNoteOn = Fn.new { |ch, note, vel|
  pitch.freq = noteToHz.call(note)
  amp.gate(true)
  output[1].volts = note / 12.0   // ~1V/oct on CV jack 1
  gate[1].on = true
}

Midi.onNoteOff = Fn.new { |ch, note, vel|
  amp.gate(false)
  gate[1].on = false
}

// Sweep the filter from a control-change knob (CC 1 = mod wheel).
Midi.onCC = Fn.new { |ch, num, val|
  if (num == 1) { filt.cutoff = 200 + val * 40 }   // ~200..5300 Hz
}
