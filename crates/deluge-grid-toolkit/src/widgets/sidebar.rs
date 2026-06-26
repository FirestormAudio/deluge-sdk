//! Status sidebar helper: the Deluge's 2-column sidebar (status indicator +
//! section colour), painted per visible row. Placed in a 2-wide sibling region
//! next to a [`clip_grid`](crate::widgets::clip_grid) /
//! [`clip_list`](crate::widgets::clip_list).
//!
//! The colours are app policy (section palette, status mapping) — this only owns
//! the 2-column layout. Pair it with
//! [`ClipRowData::status_color`](crate::widgets::clip_row::ClipRowData::status_color).

use crate::imode::Frame;
use crate::{Color, Pad};

/// One sidebar row: a status indicator (column 0) and a section colour
/// (column 1).
#[derive(Clone, Copy, Debug, Default)]
pub struct SidebarRow {
    pub status: Color,
    pub section: Color,
}

/// Paint a 2-column status sidebar into the current frame/region, one
/// [`SidebarRow`] per visible row (column 0 = status, column 1 = section).
pub fn status_sidebar<F>(f: &mut Frame, rows: usize, row: F)
where
    F: Fn(usize) -> SidebarRow,
{
    let (h, w) = f.size();
    for r in 0..rows.min(h) {
        let sr = row(r);
        f.paint(Pad::new(r, 0), sr.status);
        if w >= 2 {
            f.paint(Pad::new(r, 1), sr.section);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imode::{GridUi, PadInput};

    #[test]
    fn paints_two_columns() {
        let mut ui = GridUi::new();
        ui.run(0, PadInput::new(), |f| {
            // place the sidebar in the right-hand 2 columns
            let (_main, side) = f.split_cols(16);
            f.region(side, |s| {
                status_sidebar(s, 8, |r| SidebarRow {
                    status: Color::rgb(0, 255, 255),
                    section: Color::rgb((r * 10) as u8, 0, 0),
                });
            });
        });
        assert_eq!(ui.grid().get_pad(0, 16), Color::rgb(0, 255, 255)); // status col
        assert_eq!(ui.grid().get_pad(3, 17), Color::rgb(30, 0, 0)); // section col, row 3
    }
}
