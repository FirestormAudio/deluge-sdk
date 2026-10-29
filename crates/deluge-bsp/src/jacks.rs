//! Audio jack-detect inputs and the speaker-enable output.
//!
//! The Deluge senses five rear/side audio jacks through mechanical switch
//! contacts wired to GPIO inputs (a present jack reads high), and gates its
//! on-board speaker amplifier through one GPIO output.  These pins are plain
//! GPIO level reads — no ADC, no PIC involvement — so this module holds the board's
//! [`JackPins`] as owned `rza1_hal::gpio` pins.
//!
//! The speaker-enable *policy* (when to mute the amp) is product-specific and
//! intentionally left to the caller; see [`set_speaker_enable`].

use core::cell::RefCell;

use critical_section::Mutex;
use rza1_hal::gpio::{Input, Level, Output};

use crate::board::JackPins;

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

/// The jack detects and the speaker enable, once [`init`] has run.
struct Lines {
    headphone: Input<'static>,
    line_in: Input<'static>,
    mic: Input<'static>,
    line_out_l: Input<'static>,
    line_out_r: Input<'static>,
    speaker_enable: Output<'static>,
}

static LINES: Mutex<RefCell<Option<Lines>>> = Mutex::new(RefCell::new(None));

/// Make the five jack-detect pins inputs and the speaker-enable pin an output, leaving the
/// amplifier disabled. Runs once; a second call replaces nothing (its pins are already owned).
pub fn init(pins: JackPins) {
    let lines = Lines {
        headphone: Input::new(pins.headphone),
        line_in: Input::new(pins.line_in),
        mic: Input::new(pins.mic),
        line_out_l: Input::new(pins.line_out_l),
        line_out_r: Input::new(pins.line_out_r),
        speaker_enable: Output::new(pins.speaker_enable, Level::Low),
    };
    critical_section::with(|cs| LINES.borrow_ref_mut(cs).replace(lines));
}

/// Returns `true` if `jack` is currently inserted; `false` before [`init`].
pub fn is_inserted(jack: Jack) -> bool {
    critical_section::with(|cs| {
        LINES.borrow_ref(cs).as_ref().is_some_and(|l| {
            match jack {
                Jack::Headphone => &l.headphone,
                Jack::LineIn => &l.line_in,
                Jack::Mic => &l.mic,
                Jack::LineOutL => &l.line_out_l,
                Jack::LineOutR => &l.line_out_r,
            }
            .is_high()
        })
    })
}

/// Drive the speaker-amplifier enable output (`true` = amplifier on). Does nothing before
/// [`init`].
///
/// The caller owns the muting policy; the standard one is "enable only when no
/// headphone or line-out is inserted".
pub fn set_speaker_enable(on: bool) {
    critical_section::with(|cs| {
        if let Some(l) = LINES.borrow_ref_mut(cs).as_mut() {
            l.speaker_enable.set_level(on.into());
        }
    });
}
