//! RGB pad-LED surface for the Deluge's 18 × 8 pad grid.
//!
//! Mirrors [`crate::oled`]: an in-memory frame buffer plus a `flush` that streams
//! it to the hardware. The pad LEDs are driven by the PIC co-processor, which
//! accepts colour data as nine "column-pairs" — each message carries two adjacent
//! columns (16 × RGB) — followed by a refresh trigger. The board-specific packing
//! (pair layout, row order) lives here; product policy (what colours to show)
//! stays in the app, and the ergonomic capability wrapper lives in the `deluge`
//! SDK.

#[cfg(target_os = "none")]
use crate::pic;

/// Pad columns (16 main + 2 sidebar).
pub const COLS: usize = 18;
/// Pad rows.
pub const ROWS: usize = 8;
/// Column-pairs sent to the PIC (`COLS / 2`).
const PAIRS: usize = COLS / 2;

/// An RGB pad-LED frame buffer.
///
/// Index with [`set`](PadLeds::set) using `x` 0–17, `y` 0–7 (matching
/// [`pic::pad_coords`] / the input event coordinates). Call
/// [`flush`](PadLeds::flush) to push changes to the panel.
pub struct PadLeds {
    /// `grid[col][row]` = `[r, g, b]`.
    grid: [[[u8; 3]; ROWS]; COLS],
    /// Per-pair cache of the last-sent colours, so unchanged pairs are skipped.
    last_sent: [[[u8; 3]; 16]; PAIRS],
    /// Forces the next flush to send every pair (e.g. first frame).
    dirty_all: bool,
}

impl PadLeds {
    /// A blank surface (all LEDs off).
    pub const fn new() -> Self {
        Self {
            grid: [[[0u8; 3]; ROWS]; COLS],
            last_sent: [[[0u8; 3]; 16]; PAIRS],
            dirty_all: true,
        }
    }

    /// Set the LED at (`x`, `y`) to `rgb`. Out-of-range coordinates are ignored.
    /// Takes effect on the next [`flush`](PadLeds::flush).
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, rgb: [u8; 3]) {
        if x < COLS && y < ROWS {
            self.grid[x][y] = rgb;
        }
    }

    /// Turn all LEDs off (next flush).
    #[inline]
    pub fn clear(&mut self) {
        self.grid = [[[0u8; 3]; ROWS]; COLS];
    }

    /// Pack one column-pair into the PIC wire layout: rows 0–7 of the left column
    /// (`2*pair`) then rows 0–7 of the right column (`2*pair + 1`).
    fn pack_pair(&self, pair: usize) -> [[u8; 3]; 16] {
        let mut out = [[0u8; 3]; 16];
        let left = pair * 2;
        let right = left + 1;
        out[..ROWS].copy_from_slice(&self.grid[left]);
        out[8..8 + ROWS].copy_from_slice(&self.grid[right]);
        out
    }
}

impl Default for PadLeds {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "none")]
impl PadLeds {
    /// Stream the buffer to the pad LEDs.
    ///
    /// Sends only the column-pairs that changed since the last flush, then
    /// triggers the PIC's display refresh. All traffic goes over the serialized
    /// PIC transport (see the Advanced developer guide, `docs/advanced-guide.md`
    /// §7 — *Dropping down to the BSP & HAL*).
    pub async fn flush(&mut self) {
        for pair in 0..PAIRS {
            let colours = self.pack_pair(pair);
            if self.dirty_all || colours != self.last_sent[pair] {
                pic::set_column_pair_rgb(pair as u8, &colours).await;
                self.last_sent[pair] = colours;
            }
        }
        self.dirty_all = false;
        pic::done_sending_rows().await;
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn pack_pair_splits_left_and_right_columns() {
        let mut leds = PadLeds::new();
        leds.set(0, 0, [1, 2, 3]); // left column of pair 0, row 0
        leds.set(1, 7, [4, 5, 6]); // right column of pair 0, row 7
        let packed = leds.pack_pair(0);
        assert_eq!(packed[0], [1, 2, 3]);
        assert_eq!(packed[8 + 7], [4, 5, 6]);
    }

    #[test]
    fn set_ignores_out_of_range() {
        let mut leds = PadLeds::new();
        leds.set(COLS, 0, [9, 9, 9]);
        leds.set(0, ROWS, [9, 9, 9]);
        assert_eq!(leds.pack_pair(0)[0], [0, 0, 0]);
    }
}
