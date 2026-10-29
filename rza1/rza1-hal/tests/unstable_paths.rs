//! The `#[doc(hidden)]` paths DelugeFirmware still uses until phase 9 (CHANGELOG, phase 6).
//! Compiling this file is the check.
#[allow(unused_imports)]
use rza1_hal::usb::pipe::{pipe_brdy_enable, pipe_enable, pipe_iso_in_activate, pipe_reset};
#[allow(unused_imports)]
use rza1_hal::usb::regs::usb;
