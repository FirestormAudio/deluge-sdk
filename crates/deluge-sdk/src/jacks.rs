//! Audio jack detection + speaker-amplifier control.

use core::sync::atomic::{AtomicBool, Ordering};

use deluge_bsp::jacks::{self, Jack};

fn ensure_init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: runs once. Configures the five jack-detect inputs and the
    // speaker-enable output (left disabled).
    unsafe { jacks::init() };
}

/// Audio jack-detect inputs and the speaker-amplifier enable.
///
/// Taken once from [`Deluge::jacks`](crate::Deluge::jacks). Reads tell you which
/// jacks are inserted; [`set_speaker`](Jacks::set_speaker) drives the amp, and
/// [`apply_speaker_mute`](Jacks::apply_speaker_mute) applies the stock policy.
pub struct Jacks {
    _private: (),
}

impl Jacks {
    pub(crate) fn new() -> Self {
        ensure_init();
        Self { _private: () }
    }

    /// `true` if the headphone jack is inserted.
    #[inline]
    pub fn headphone(&self) -> bool {
        jacks::is_inserted(Jack::Headphone)
    }

    /// `true` if the line-input jack is inserted.
    #[inline]
    pub fn line_in(&self) -> bool {
        jacks::is_inserted(Jack::LineIn)
    }

    /// `true` if the microphone jack is inserted.
    #[inline]
    pub fn mic(&self) -> bool {
        jacks::is_inserted(Jack::Mic)
    }

    /// `true` if the left line-output jack is inserted.
    #[inline]
    pub fn line_out_left(&self) -> bool {
        jacks::is_inserted(Jack::LineOutL)
    }

    /// `true` if the right line-output jack is inserted.
    #[inline]
    pub fn line_out_right(&self) -> bool {
        jacks::is_inserted(Jack::LineOutR)
    }

    /// Drive the speaker amplifier on (`true`) or off (`false`) directly.
    #[inline]
    pub fn set_speaker(&mut self, on: bool) {
        // SAFETY: GPIO write to the speaker-enable output configured by init.
        unsafe { jacks::set_speaker_enable(on) };
    }

    /// Apply the stock speaker-mute policy once: enable the amplifier only when
    /// neither the headphone nor either line-output jack is inserted.
    ///
    /// Call this whenever jack state may have changed (e.g. on a poll). Returns
    /// the value written, so callers can log/observe it.
    pub fn apply_speaker_mute(&mut self) -> bool {
        let enable = !(self.headphone() || self.line_out_left() || self.line_out_right());
        self.set_speaker(enable);
        enable
    }
}
