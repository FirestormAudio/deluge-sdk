//! The `#[doc(hidden)]` paths DelugeFirmware still uses until phase 9 (CHANGELOG, phase 6), with
//! the signatures it calls them by. Compiling this file is the check; nothing runs.
use rza1_hal::usb::pipe::{pipe_brdy_enable, pipe_enable, pipe_iso_in_activate, pipe_reset};
use rza1_hal::usb::regs::usb;

#[allow(dead_code)]
fn signatures() {
    let u = usb(0);
    // SAFETY: never called; this only type-checks DelugeFirmware's calls.
    unsafe {
        pipe_reset(u, 1);
        pipe_enable(u, 1);
        pipe_brdy_enable(u, 1);
        pipe_iso_in_activate(u, 1);
    }
}
