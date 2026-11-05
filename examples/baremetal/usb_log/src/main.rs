//! Deluge SDK example: stream `log` output over USB CDC serial — no probe.
//!
//! Build with the `usb-log` feature (already enabled in this crate's
//! `Cargo.toml`), flash it, then open the resulting USB serial port on your host
//! (e.g. `/dev/ttyACM0`, or any serial terminal) to watch the log lines arrive.
//!
//! The runtime registers the USB logger automatically; the app just calls the
//! normal `log` macros (re-exported from the prelude).

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use embassy_time::Timer;

#[deluge::app]
async fn main(_dlg: Deluge) {
    info!("usb_log example started");

    let mut tick: u32 = 0;
    loop {
        info!("tick {}", tick);
        if tick.is_multiple_of(5) {
            warn!("every fifth tick is a warning ({})", tick);
        }
        tick = tick.wrapping_add(1);
        Timer::after_millis(500).await;
    }
}
