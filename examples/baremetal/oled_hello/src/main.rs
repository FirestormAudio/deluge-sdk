//! Deluge SDK example: draw to the OLED with `embedded-graphics`.
//!
//! Shows text on the 128×48 panel and blinks the SYNC LED. The `Oled` handle is
//! an `embedded-graphics` `DrawTarget`, so the whole ecosystem (fonts, shapes)
//! works directly; `dlg.oled().await` also brings up the PIC service the
//! display's chip-select handshake needs.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use embassy_time::Timer;
use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    text::Text,
};

#[deluge::app]
async fn main(dlg: Deluge) {
    let mut led = dlg.sync_led();
    let mut oled = dlg.oled().await;

    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);

    loop {
        led.toggle();

        oled.clear();
        Text::new("hello deluge", Point::new(2, 12), style)
            .draw(&mut oled)
            .ok();
        oled.flush().await;

        Timer::after_millis(500).await;
    }
}
