//! The DVU playback path's controls: SCUX channel 0 (FFD0 → SRC0 → DVU0 → SSIF0), which
//! [`crate::audio::init_with_scux`] builds. Its ring is [`crate::audio::with_tx`].
//!
//! Every function does nothing before that `init`.

use core::cell::RefCell;

use critical_section::Mutex;
use rza1_hal::scux::{Control, RampConfig};

static CONTROL: Mutex<RefCell<Option<Control<'static>>>> = Mutex::new(RefCell::new(None));

pub(crate) fn install(control: Control<'static>) {
    critical_section::with(|cs| *CONTROL.borrow(cs).borrow_mut() = Some(control));
}

fn with_control(f: impl FnOnce(&mut Control<'static>)) {
    crate::audio::with_slot(&CONTROL, f);
}

/// Set audio channel `audio_ch`'s volume (Q20: 0x0010_0000 is unity).
pub fn set_volume(audio_ch: u8, vol: u32) {
    with_control(|c| c.set_volume(audio_ch, vol));
}

/// Set both channels' volume (Q20).
pub fn set_volume_stereo(vol: u32) {
    with_control(|c| c.set_volume_all(vol));
}

/// Ramp both channels to `vol` with `ramp`.
pub fn fade_to(vol: u32, ramp: RampConfig) {
    with_control(|c| c.fade_to(vol, ramp));
}

/// Change the engine rate the SRC converts from. Nothing on a path without a converter
/// ([`crate::audio::init_with_scux`]), as before; only [`crate::audio::init_with_scux_src`] has one.
pub fn update_engine_rate(engine_hz: u32) {
    with_control(|c| {
        if c.converts() {
            c.set_input_rate(engine_hz);
        }
    });
}
