//! Deluge SDK example: light each button's indicator LED while it's held, and
//! turn the gold-knob columns up when PLAY is pressed.
//!
//! Exercises `leds()` + `input()` and the named `controls` ids.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;

#[deluge::app]
async fn main(dlg: Deluge) {
    let input = dlg.input();
    let mut leds = dlg.leds().await;
    leds.clear().await;

    loop {
        if let Event::Button { id, pressed } = input.next().await {
            // The LED id matches the button id.
            leds.set(id, pressed).await;

            // Light both gold-knob columns while PLAY is held.
            if id == controls::button::PLAY {
                let level = if pressed { [255; 4] } else { [0; 4] };
                leds.gold_knob(0, level).await;
                leds.gold_knob(1, level).await;
            }
        }
    }
}
