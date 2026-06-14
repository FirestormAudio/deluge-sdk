//! Deluge SDK example: SD-card write/read round-trip.
//!
//! Writes a small file to the card root, reads it back, and signals the result
//! on the SYNC LED: solid = round-trip OK, fast blink = no card / mismatch.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use embassy_time::Timer;

const FILE: &str = "SDKTEST.TXT";
const PAYLOAD: &[u8] = b"deluge sdk";

#[deluge::app]
async fn main(dlg: Deluge) {
    let mut led = dlg.sync_led();

    let ok = match dlg.sd().await {
        Ok(mut sd) => {
            let mut buf = [0u8; 32];
            sd.write(FILE, PAYLOAD).is_ok()
                && matches!(sd.read(FILE, &mut buf), Ok(n) if &buf[..n] == PAYLOAD)
        }
        Err(_) => false,
    };

    loop {
        if ok {
            led.on();
            Timer::after_millis(1000).await;
        } else {
            led.toggle();
            Timer::after_millis(120).await;
        }
    }
}
