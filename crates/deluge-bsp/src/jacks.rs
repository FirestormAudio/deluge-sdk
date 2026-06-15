//! Audio jack-detect inputs and the speaker-enable output.
//!
//! The Deluge senses five rear/side audio jacks through mechanical switch
//! contacts wired to GPIO inputs (a present jack reads high), and gates its
//! on-board speaker amplifier through one GPIO output.  These pins are plain
//! GPIO level reads — no ADC, no PIC involvement — so this module is a thin map
//! over [`rza1l_hal::gpio`].
//!
//! The speaker-enable *policy* (when to mute the amp) is product-specific and
//! intentionally left to the caller; see [`set_speaker_enable`].

use rza1l_hal::gpio;

/// The audio jacks whose insertion can be sensed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jack {
    /// Headphone output.
    Headphone,
    /// Line input.
    LineIn,
    /// Microphone input.
    Mic,
    /// Line output, left.
    LineOutL,
    /// Line output, right.
    LineOutR,
}

// ── Pin map (port, pin) ────────────────────────────────────────────────────────

const HEADPHONE: (u8, u8) = (6, 5);
const LINE_IN: (u8, u8) = (6, 6);
const MIC: (u8, u8) = (7, 9);
const LINE_OUT_L: (u8, u8) = (6, 3);
const LINE_OUT_R: (u8, u8) = (6, 4);

/// Speaker-amplifier enable output (high = amplifier on).
const SPEAKER_ENABLE: (u8, u8) = (4, 1);

const fn pin_of(jack: Jack) -> (u8, u8) {
    match jack {
        Jack::Headphone => HEADPHONE,
        Jack::LineIn => LINE_IN,
        Jack::Mic => MIC,
        Jack::LineOutL => LINE_OUT_L,
        Jack::LineOutR => LINE_OUT_R,
    }
}

/// Configure the five jack-detect pins as inputs and the speaker-enable pin as
/// an output, leaving the amplifier disabled.
///
/// # Safety
/// Writes to GPIO mode/direction registers; call once during bring-up.
pub unsafe fn init() {
    unsafe {
        gpio::set_as_input(HEADPHONE.0, HEADPHONE.1);
        gpio::set_as_input(LINE_IN.0, LINE_IN.1);
        gpio::set_as_input(MIC.0, MIC.1);
        gpio::set_as_input(LINE_OUT_L.0, LINE_OUT_L.1);
        gpio::set_as_input(LINE_OUT_R.0, LINE_OUT_R.1);
        gpio::set_as_output(SPEAKER_ENABLE.0, SPEAKER_ENABLE.1);
        gpio::write(SPEAKER_ENABLE.0, SPEAKER_ENABLE.1, false);
    }
}

/// Returns `true` if `jack` is currently inserted.
///
/// [`init`] must have run first (the pin is configured as an input there).
pub fn is_inserted(jack: Jack) -> bool {
    let (port, pin) = pin_of(jack);
    // SAFETY: reads the read-only Port Pin Read register for a pin `init`
    // configured as an input.
    unsafe { gpio::read_pin(port, pin) }
}

/// Drive the speaker-amplifier enable output (`true` = amplifier on).
///
/// The caller owns the muting policy; the standard one is "enable only when no
/// headphone or line-out is inserted".
///
/// # Safety
/// Writes to a GPIO output [`init`] configured.
pub unsafe fn set_speaker_enable(on: bool) {
    unsafe { gpio::write(SPEAKER_ENABLE.0, SPEAKER_ENABLE.1, on) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_map_matches_hardware() {
        assert_eq!(pin_of(Jack::Headphone), (6, 5));
        assert_eq!(pin_of(Jack::LineIn), (6, 6));
        assert_eq!(pin_of(Jack::Mic), (7, 9));
        assert_eq!(pin_of(Jack::LineOutL), (6, 3));
        assert_eq!(pin_of(Jack::LineOutR), (6, 4));
        assert_eq!(SPEAKER_ENABLE, (4, 1));
    }
}
