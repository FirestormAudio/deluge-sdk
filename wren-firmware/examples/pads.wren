// pads.wren — light an indicator LED while a pad is held, show the last pad on
// the OLED, and bend CV jack 1 with the first encoder.
//
// Demonstrates the input callbacks (pads, buttons, encoders) and output (LED,
// OLED, CV) working together.

Oled.clear()
Oled.text(0, 0, "press a pad")
Oled.show()

Pads.onPress = Fn.new { |x, y|
  Led.on(0)
  Oled.clear()
  Oled.text(0, 0, "pad %(x),%(y)")
  Oled.show()
}

Pads.onRelease = Fn.new { |x, y| Led.off(0) }

// First encoder nudges CV jack 1 by 0.1 V per detent (clamped 0..5 V).
var cv = 0.0
Enc.onTurn = Fn.new { |index, delta|
  if (index == 0) {
    cv = (cv + delta * 0.1).max(0.0).min(5.0)
    output[1].volts = cv
  }
}
