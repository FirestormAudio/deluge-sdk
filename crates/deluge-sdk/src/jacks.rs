//! Audio jack detection + speaker-amplifier control.

#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "none")]
use deluge_bsp::jacks::{self, Jack};

#[cfg(target_os = "none")]
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
///
/// On the host simulator there is no physical panel: all jacks read as not
/// inserted and the speaker control is a no-op.
pub struct Jacks {
    _private: (),
}

impl Jacks {
    pub(crate) fn new() -> Self {
        #[cfg(target_os = "none")]
        ensure_init();
        Self { _private: () }
    }

    /// `true` if the headphone jack is inserted.
    #[inline]
    pub fn headphone(&self) -> bool {
        #[cfg(target_os = "none")]
        {
            jacks::is_inserted(Jack::Headphone)
        }
        #[cfg(not(target_os = "none"))]
        {
            false
        }
    }

    /// `true` if the line-input jack is inserted.
    #[inline]
    pub fn line_in(&self) -> bool {
        #[cfg(target_os = "none")]
        {
            jacks::is_inserted(Jack::LineIn)
        }
        #[cfg(not(target_os = "none"))]
        {
            false
        }
    }

    /// `true` if the microphone jack is inserted.
    #[inline]
    pub fn mic(&self) -> bool {
        #[cfg(target_os = "none")]
        {
            jacks::is_inserted(Jack::Mic)
        }
        #[cfg(not(target_os = "none"))]
        {
            false
        }
    }

    /// `true` if the left line-output jack is inserted.
    #[inline]
    pub fn line_out_left(&self) -> bool {
        #[cfg(target_os = "none")]
        {
            jacks::is_inserted(Jack::LineOutL)
        }
        #[cfg(not(target_os = "none"))]
        {
            false
        }
    }

    /// `true` if the right line-output jack is inserted.
    #[inline]
    pub fn line_out_right(&self) -> bool {
        #[cfg(target_os = "none")]
        {
            jacks::is_inserted(Jack::LineOutR)
        }
        #[cfg(not(target_os = "none"))]
        {
            false
        }
    }

    /// Drive the speaker amplifier on (`true`) or off (`false`) directly. No-op
    /// on the host simulator.
    #[inline]
    pub fn set_speaker(&mut self, on: bool) {
        // SAFETY: GPIO write to the speaker-enable output configured by init.
        #[cfg(target_os = "none")]
        unsafe {
            jacks::set_speaker_enable(on)
        };
        #[cfg(not(target_os = "none"))]
        let _ = on;
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
