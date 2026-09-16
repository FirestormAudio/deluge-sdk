//! Deluge SDK example: a horizontal param-column menu (`HMenu`) on the OLED.
//!
//! A FILTER page of four parameter columns. The **horizontal** encoder
//! (`SCROLL_X`) moves the selected column; the **select** encoder edits the
//! focused parameter directly (`MenuInput::Edit`) — the two-axis model the
//! Deluge param view uses, no edit-mode. The page is rebuilt from `Filter` each
//! frame, so the display always reflects it.
//!
//! Needs the `deluge/alloc` global allocator (the GPL `deluge-ui-toolkit`);
//! build with `cargo build-fw-alloc -p oled_hmenu`.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use deluge::{Event, controls};
use deluge_ui_toolkit::{HMenu, MenuInput, MenuState, MenuStyle};

struct Filter {
    cut: f32,
    res: f32,
    drive: f32,
    pan: f32,
}

#[deluge::app]
async fn main(dlg: Deluge) {
    let input = dlg.input();
    let mut oled = dlg.oled().await;

    let mut nav = MenuState::new();
    // Push content below the faceplate-hidden top rows (the SDK owns this fact).
    let style = MenuStyle {
        top_inset: deluge::Oled::VISIBLE_TOP as i32,
        ..MenuStyle::default()
    };
    let mut f = Filter {
        cut: 0.7,
        res: 0.2,
        drive: 0.4,
        pan: 0.0,
    };

    let mut menu_input = MenuInput::None;
    loop {
        oled.clear();
        {
            let mut ui = HMenu::begin(&mut oled, &mut nav, menu_input, &style);
            ui.title("FILTER");
            ui.lpf("CUT", &mut f.cut, 0.0..=1.0);
            ui.knob("RES", &mut f.res, 0.0..=1.0);
            ui.knob("DRIVE", &mut f.drive, 0.0..=1.0);
            ui.pan("PAN", &mut f.pan, -1.0..=1.0);
            ui.end();
        }
        oled.flush().await;

        // Horizontal encoder moves the column; select encoder edits the param.
        menu_input = loop {
            match input.next().await {
                Event::Encoder { index, delta } if index == controls::encoder::SCROLL_X => {
                    break MenuInput::Turn(delta as i32);
                }
                Event::Encoder { index, delta } if index == controls::encoder::SELECT => {
                    break MenuInput::Edit(delta as i32);
                }
                _ => {}
            }
        };
    }
}
