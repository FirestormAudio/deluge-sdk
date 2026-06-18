//! Deluge SDK example: a DIN MIDI → CV/gate converter.
//!
//! Note-on sets CV channel 0 to the note's pitch (1 V/oct) and raises gate 0;
//! note-off (or note-on with velocity 0) drops the gate. Exercises `midi()`,
//! `cv()`, and `gate()` together.
//!
//! The MIDI parser here is intentionally minimal: it handles note-on/note-off
//! on any channel and skips other messages. It does not implement running
//! status, so a real synth would want a fuller parser.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;

/// MIDI note → volts at 1 V/octave (note 0 = 0 V, +1 V per 12 semitones).
fn note_to_volts(note: u8) -> f32 {
    note as f32 / 12.0
}

#[deluge::app]
async fn main(dlg: Deluge) {
    let midi = dlg.midi();
    let mut cv = dlg.cv();
    let mut gate = dlg.gate();

    loop {
        let status = midi.recv().await;
        match status & 0xF0 {
            0x90 => {
                // Note on (velocity 0 means note off).
                let note = midi.recv().await;
                let velocity = midi.recv().await;
                if velocity > 0 {
                    cv.set_volts(0, note_to_volts(note)).await;
                    gate.set(0, true);
                } else {
                    gate.set(0, false);
                }
            }
            0x80 => {
                // Note off: consume note + velocity, drop the gate.
                let _note = midi.recv().await;
                let _velocity = midi.recv().await;
                gate.set(0, false);
            }
            _ => {
                // Other messages are ignored (their data bytes are not consumed;
                // a fuller parser would track message lengths / running status).
            }
        }
    }
}
