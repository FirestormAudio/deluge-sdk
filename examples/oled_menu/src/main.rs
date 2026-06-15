//! Deluge SDK example: an immediate-mode settings menu on the OLED.
//!
//! Turn the **select** encoder to move the highlight; **press** it to edit a
//! value (turn to change, press again to confirm) or to enter a submenu; press
//! **BACK** to leave a submenu / cancel an edit.
//!
//! The whole screen is rebuilt every frame from `App` (the source of truth), so
//! the display always matches the state — that is the immediate-mode model. The
//! `alloc` feature on `deluge` registers the SRAM global allocator the GPL
//! `deluge-ui-toolkit` needs; build with `-Zbuild-std=core,alloc`
//! (`cargo build-fw-alloc -p oled_menu`).

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use deluge::{Event, controls};
use deluge_ui_toolkit::{Menu, MenuEnum, MenuInput, MenuState, MenuStyle};

#[derive(Clone, Copy, PartialEq)]
enum Wave {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl MenuEnum for Wave {
    fn variants() -> &'static [Self] {
        &[Wave::Sine, Wave::Triangle, Wave::Saw, Wave::Square]
    }
    fn name(&self) -> &'static str {
        match self {
            Wave::Sine => "SINE",
            Wave::Triangle => "TRI",
            Wave::Saw => "SAW",
            Wave::Square => "SQR",
        }
    }
}

/// The single source of truth the menu binds to.
struct App {
    freq: i32,
    wave: Wave,
    mono: bool,
    drive: f32,
    brightness: i32,
}

#[deluge::app]
async fn main(dlg: Deluge) {
    let input = dlg.input();
    let mut oled = dlg.oled().await;

    let mut nav = MenuState::new();
    let style = MenuStyle::default(); // top_inset = 5 → content in the visible 128×43
    let mut app = App {
        freq: 440,
        wave: Wave::Sine,
        mono: false,
        drive: 0.5,
        brightness: 8,
    };

    let mut menu_input = MenuInput::None;
    loop {
        // Rebuild the whole screen from `app`.
        oled.clear();
        {
            let mut ui = Menu::begin(&mut oled, &mut nav, menu_input, &style);
            ui.title("SOUND");
            ui.int("FREQ", &mut app.freq, 20..=20000);
            ui.enumv("WAVE", &mut app.wave);
            ui.toggle("MONO", &mut app.mono);
            ui.submenu("ADVANCED", |ui| {
                ui.float("DRIVE", &mut app.drive, 0.0..=1.0);
                ui.int("BRIGHT", &mut app.brightness, 0..=15);
            });
            ui.end();
        }
        oled.flush().await;

        // Wait for the next event the menu cares about, mapping it to MenuInput.
        menu_input = loop {
            match input.next().await {
                Event::Encoder { index, delta } if index == controls::encoder::SELECT => {
                    break MenuInput::Turn(delta as i32);
                }
                Event::Button { id, pressed: true } if id == controls::encoder_button::SELECT => {
                    break MenuInput::Press;
                }
                Event::Button { id, pressed: true } if id == controls::button::BACK => {
                    break MenuInput::Back;
                }
                _ => {} // ignore pads, other buttons, releases
            }
        };
    }
}
