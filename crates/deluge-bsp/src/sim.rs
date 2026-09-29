//! The desktop simulator's panel, standing in for the peripherals on a host build (`sim-link`).
//!
//! On the device the PIC co-processor drives the pads and LEDs, and the OLED is on its own SPI bus. On the host
//! the peripheral modules' host arms land here instead, and this module keeps the shared panel
//! ([`deluge_sim_link::SharedPanel`]) the simulator window renders:
//!
//! - every outbound PIC command ([`crate::pic`]'s `tx`) is applied as the PIC would apply it. Pad colours go into
//!   a virtual PIC framebuffer, which a refresh (`DONE_SENDING_ROWS`) publishes, and the smooth-scroll commands
//!   shift that framebuffer as the PIC's own does;
//! - every OLED frame ([`crate::oled`]'s host capture) is copied to the panel.
//!
//! So a program on this BSP drives the simulator through the same calls it makes on the device. [`install`] is
//! called once, by whatever runs the simulator (`deluge_simulator::run_brain`), before the program starts; until
//! then, and in a host build without a panel, the host arms stay the no-ops they otherwise are.

use std::sync::{Mutex, OnceLock, PoisonError};

use deluge_sim_link::SharedPanel;
use deluge_sim_link::audio::BrainEnds;

use crate::rgb::{COLS, ROWS};

static PANEL: OnceLock<SharedPanel> = OnceLock::new();
static AUDIO: Mutex<Option<BrainEnds>> = Mutex::new(None);

/// Install the simulator's panel and the program's ends of the audio bridge. Called once, before the program
/// runs; a second call is ignored.
pub fn install(panel: SharedPanel, audio: BrainEnds) {
    if PANEL.set(panel).is_ok() {
        *AUDIO.lock().unwrap_or_else(PoisonError::into_inner) = Some(audio);
    }
}

/// The installed panel, if a simulator is running.
pub fn panel() -> Option<&'static SharedPanel> {
    PANEL.get()
}

/// Take the program's ends of the audio bridge (once).
pub fn take_audio() -> Option<BrainEnds> {
    AUDIO.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// The OLED's frame, 768 bytes page-major, as [`crate::oled::FrameBuffer::as_bytes`] lays it out.
pub(crate) fn display(frame: &[u8]) {
    if let Some(panel) = panel() {
        panel.set_display(frame);
    }
}

// ── The virtual PIC ───────────────────────────────────────────────────────────

use crate::pic::{
    CMD_DONE_SENDING_ROWS as DONE_SENDING_ROWS, CMD_LED_OFF_BASE as LED_OFF_BASE, CMD_LED_ON_BASE as LED_ON_BASE,
    CMD_SET_COLOUR_FOR_COLS_BASE as COLS_BASE, CMD_SET_GOLD_KNOB_0_INDICATORS as GOLD_KNOB_0,
    CMD_SET_GOLD_KNOB_1_INDICATORS as GOLD_KNOB_1, CMD_SET_SCROLL_DOWN as SCROLL_DOWN,
    CMD_SET_SCROLL_HORIZONTAL_BASE as SCROLL_HORIZONTAL_BASE, CMD_SET_SCROLL_ROW_BASE as SCROLL_ROW_BASE,
    CMD_SET_SCROLL_UP as SCROLL_UP,
};

/// The last byte of each ranged command: a base plus its largest argument.
const COLS_LAST: u8 = COLS_BASE + 8; // 9 column pairs
const LED_OFF_LAST: u8 = LED_OFF_BASE + 35; // 36 LEDs
const LED_ON_LAST: u8 = LED_ON_BASE + 35;
const SCROLL_ROW_LAST: u8 = SCROLL_ROW_BASE + 7; // 8 rows
const SCROLL_HORIZONTAL_LAST: u8 = SCROLL_HORIZONTAL_BASE + 3; // 2 flag bits

/// The PIC's pad framebuffer, `[col][row]`, and the horizontal scroll the last setup command started.
struct Pic {
    grid: [[[u8; 3]; ROWS]; COLS],
    /// `(direction, columns)`: +1 to scroll right, -1 left, over 16 or 18 columns.
    scroll: (i8, usize),
}

static PIC: Mutex<Pic> = Mutex::new(Pic {
    grid: [[[0; 3]; ROWS]; COLS],
    scroll: (1, 16),
});

/// Apply one outbound PIC command, as `pic::tx` sends it: exactly one whole command per call.
pub(crate) fn pic_command(bytes: &[u8]) {
    let Some(panel) = panel() else {
        return;
    };
    let Some((&cmd, args)) = bytes.split_first() else {
        return;
    };
    let mut pic = PIC.lock().unwrap_or_else(PoisonError::into_inner);
    match cmd {
        // Two columns' colours: the first 8 entries are column 2·pair, the next 8 column 2·pair+1.
        COLS_BASE..=COLS_LAST => {
            let pair = usize::from(cmd - COLS_BASE);
            for (i, rgb) in args.chunks_exact(3).take(16).enumerate() {
                let col = pair * 2 + i / 8;
                if col < COLS {
                    pic.grid[col][i % 8].copy_from_slice(rgb);
                }
            }
        }
        DONE_SENDING_ROWS => panel.set_all_pads(&all_pads(&pic.grid)),
        LED_OFF_BASE..=LED_OFF_LAST => panel.set_led(usize::from(cmd - LED_OFF_BASE), false),
        LED_ON_BASE..=LED_ON_LAST => panel.set_led(usize::from(cmd - LED_ON_BASE), true),
        GOLD_KNOB_0 | GOLD_KNOB_1 => {
            if let Ok(levels) = <[u8; 4]>::try_from(args) {
                panel.set_knob_indicator(usize::from(cmd - GOLD_KNOB_0), levels);
            }
        }
        SCROLL_HORIZONTAL_BASE..=SCROLL_HORIZONTAL_LAST => {
            let flags = cmd - SCROLL_HORIZONTAL_BASE;
            pic.scroll = (
                if flags & 1 != 0 { 1 } else { -1 },
                if flags & 2 != 0 { COLS } else { 16 },
            );
        }
        // One row of a horizontal scroll: shift that row a square and bring the colour in at the far end.
        SCROLL_ROW_BASE..=SCROLL_ROW_LAST => {
            let row = usize::from(cmd - SCROLL_ROW_BASE);
            if let Ok(rgb) = <[u8; 3]>::try_from(args) {
                let (direction, columns) = pic.scroll;
                shift_row(&mut pic.grid, row, direction, columns, rgb);
            }
        }
        // A vertical scroll: every row moves one step, and `args` is the incoming row, one colour per column.
        SCROLL_UP | SCROLL_DOWN => {
            let up = cmd == SCROLL_UP;
            for col in 0..COLS {
                if up {
                    pic.grid[col].copy_within(1.., 0);
                } else {
                    pic.grid[col].copy_within(..ROWS - 1, 1);
                }
                let incoming = args.get(col * 3..col * 3 + 3).map_or([0; 3], |c| [c[0], c[1], c[2]]);
                pic.grid[col][if up { ROWS - 1 } else { 0 }] = incoming;
            }
            panel.set_all_pads(&all_pads(&pic.grid));
        }
        // Refresh time, OLED select/DC handshakes, version and button-state requests: nothing to show.
        _ => {}
    }
}

/// Shift `row` one square in `direction` across the first `columns` columns, as the app's own image moves
/// (`pad_leds.cpp`'s `horizontal::renderScroll`), and put `incoming` in the square that opens up.
fn shift_row(grid: &mut [[[u8; 3]; ROWS]; COLS], row: usize, direction: i8, columns: usize, incoming: [u8; 3]) {
    if direction > 0 {
        for x in 0..columns - 1 {
            grid[x][row] = grid[x + 1][row];
        }
        grid[columns - 1][row] = incoming;
    } else {
        for x in (1..columns).rev() {
            grid[x][row] = grid[x - 1][row];
        }
        grid[0][row] = incoming;
    }
}

/// The grid as the panel's `set_all_pads` takes it: column-major `[r, g, b]`, `(col·ROWS + row)·3`.
fn all_pads(grid: &[[[u8; 3]; ROWS]; COLS]) -> [u8; deluge_sim_link::ALL_PADS_BYTES] {
    let mut buf = [0u8; deluge_sim_link::ALL_PADS_BYTES];
    for (col, rows) in grid.iter().enumerate() {
        for (row, rgb) in rows.iter().enumerate() {
            let o = (col * ROWS + row) * 3;
            buf[o..o + 3].copy_from_slice(rgb);
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_horizontal_scroll_row_shifts_left_and_enters_on_the_right() {
        let mut grid = [[[0u8; 3]; ROWS]; COLS];
        for (x, col) in grid.iter_mut().enumerate() {
            col[2] = [x as u8, 0, 0];
        }
        shift_row(&mut grid, 2, 1, 16, [99, 0, 0]);
        assert_eq!(grid[0][2], [1, 0, 0]);
        assert_eq!(grid[14][2], [15, 0, 0]);
        assert_eq!(grid[15][2], [99, 0, 0]);
        assert_eq!(grid[16][2], [16, 0, 0], "the sidebar is outside a 16-column scroll");
    }

    #[test]
    fn a_leftward_scroll_row_enters_on_the_left() {
        let mut grid = [[[0u8; 3]; ROWS]; COLS];
        for (x, col) in grid.iter_mut().enumerate() {
            col[0] = [x as u8, 0, 0];
        }
        shift_row(&mut grid, 0, -1, 18, [77, 0, 0]);
        assert_eq!(grid[0][0], [77, 0, 0]);
        assert_eq!(grid[1][0], [0, 0, 0]);
        assert_eq!(grid[17][0], [16, 0, 0]);
    }
}
