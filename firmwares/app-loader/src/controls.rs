//! The loader's controls, from the SDK's [`Input`]: the SELECT knob (turns and its push), BACK
//! and LOAD. [`run`] drains every event into the state the screens poll, as `pic_rx_task` did
//! before the loader stood on the SDK: whether SELECT is held, whether BACK or LOAD has been
//! pressed, and the knob's detents not yet taken.

use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use deluge::controls::{button, encoder, encoder_button};
use deluge::{Event, Input};

/// Whether the SELECT knob's push is held.
pub static SELECT_DOWN: AtomicBool = AtomicBool::new(false);
/// Latched by any LOAD press since reset, the one held at power-on included: the recovery gesture
/// that forces the boot menu.
pub static LOAD_SEEN: AtomicBool = AtomicBool::new(false);
/// Latched by a BACK press; whoever acts on it clears it.
pub static BACK_PRESSED: AtomicBool = AtomicBool::new(false);
/// SELECT knob detents not yet taken (positive = clockwise).
static DETENTS: AtomicI32 = AtomicI32::new(0);

/// The SELECT knob's detents since the last call (saturating at `i8`'s range).
pub fn take_detents() -> i8 {
    DETENTS.swap(0, Ordering::AcqRel).clamp(-128, 127) as i8
}

/// Pump the controls, forever.
#[embassy_executor::task]
pub async fn run(input: Input) {
    loop {
        match input.next().await {
            Event::Encoder { index, delta } if index == encoder::SELECT => {
                DETENTS.fetch_add(i32::from(delta), Ordering::AcqRel);
            }
            Event::Button { id, pressed } if id == encoder_button::SELECT => {
                SELECT_DOWN.store(pressed, Ordering::Release)
            }
            Event::Button { id, pressed: true } if id == button::LOAD => {
                LOAD_SEEN.store(true, Ordering::Release)
            }
            Event::Button { id, pressed: true } if id == button::BACK => {
                BACK_PRESSED.store(true, Ordering::Release)
            }
            _ => {}
        }
    }
}
