// cv_lfo.wren — a triangle LFO on CV jack 1 and a clock pulse on gate 1.
//
// A metro fires 100x/second; each tick nudges output[1] along a slow triangle
// ramp. Slew smooths between updates so the CV is continuous. Gate 1 blinks.

var phase = 0.0
var dir = 1.0

output[1].slew = 0.01   // 10 ms smoothing between metro updates

var lfo = Metro.new()
lfo.start(Fn.new { |stage|
  phase = phase + dir * 0.02
  if (phase >= 1.0) { phase = 1.0; dir = -1.0 }
  if (phase <= 0.0) { phase = 0.0; dir =  1.0 }
  output[1].volts = phase * 5.0      // 0..5 V triangle

  gate[1].on = stage % 50 == 0       // ~2 Hz gate blink
}, 0.01)

// lfo.stop()   // run this later to halt
