//! Deluge SDK example: press pads to paint the RGB grid.
//!
//! Each pad press toggles that pad between off and a position-based hue. Ties the
//! input event stream and the RGB pad output together — `Event::Pad { x, y }`
//! coordinates feed straight into `pads.set(x, y, …)`.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;

const COLS: usize = Pads::COLS;
const ROWS: usize = Pads::ROWS;

#[deluge::app]
async fn main(dlg: Deluge) {
    let input = dlg.input();
    let mut pads = dlg.pads().await;

    // Track which pads are lit so a press toggles.
    let mut lit = [[false; ROWS]; COLS];

    pads.clear();
    pads.flush().await;

    loop {
        if let Event::Pad {
            x,
            y,
            pressed: true,
        } = input.next().await
        {
            let (xu, yu) = (x as usize, y as usize);
            if xu < COLS && yu < ROWS {
                lit[xu][yu] = !lit[xu][yu];
                let color = if lit[xu][yu] {
                    // Hue spread across the grid width.
                    Color::hsv((xu as u16 * 256 / COLS as u16) as u8, 255, 200)
                } else {
                    Color::BLACK
                };
                pads.set(xu, yu, color);
                pads.flush().await;
            }
        }
    }
}
