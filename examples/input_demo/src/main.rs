//! Deluge SDK example: react to the unified input event stream.
//!
//! The SYNC LED follows pad presses, and any encoder turn toggles it — a
//! minimal demonstration of `dlg.input()` merging pad/button (PIC) and encoder
//! (GPIO IRQ) sources into one `async` queue.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;

#[deluge::app]
async fn main(dlg: Deluge) {
    let input = dlg.input();
    let mut led = dlg.sync_led();

    loop {
        match input.next().await {
            Event::Pad { pressed, .. } => led.set(pressed),
            Event::Encoder { .. } => led.toggle(),
            _ => {}
        }
    }
}
