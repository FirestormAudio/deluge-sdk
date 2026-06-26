//! RGB pad grid output.

use deluge_bsp::rgb::PadLeds;

/// An RGB colour (0–255 per channel).
///
/// Re-exported from [`deluge_bsp::rgb`] so the canonical colour type is shared
/// with the BSP (and the GPL `deluge-grid-toolkit`, which extends it). The public
/// path `deluge::Color` is unchanged.
pub use deluge_bsp::rgb::Color;

/// The RGB pad grid (18 × 8), taken once from [`Deluge::pads`](crate::Deluge::pads).
///
/// Set pad colours into the buffer, then [`flush`](Pads::flush) to the panel.
/// Coordinates match [`Event::Pad`](crate::Event::Pad): `x` 0–17, `y` 0–7.
pub struct Pads {
    leds: PadLeds,
}

impl Pads {
    /// Number of pad columns (16 main + 2 sidebar).
    pub const COLS: usize = deluge_bsp::rgb::COLS;
    /// Number of pad rows.
    pub const ROWS: usize = deluge_bsp::rgb::ROWS;

    pub(crate) fn new() -> Self {
        Self {
            leds: PadLeds::new(),
        }
    }

    /// Set the pad at (`x`, `y`) to `color` (next [`flush`](Pads::flush)).
    /// Out-of-range coordinates are ignored.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, color: Color) {
        self.leds.set(x, y, color.to_rgb());
    }

    /// Turn all pads off (next [`flush`](Pads::flush)).
    #[inline]
    pub fn clear(&mut self) {
        self.leds.clear();
    }

    /// Set every pad to `color` (next [`flush`](Pads::flush)).
    pub fn fill(&mut self, color: Color) {
        let rgb = color.to_rgb();
        for x in 0..Self::COLS {
            for y in 0..Self::ROWS {
                self.leds.set(x, y, rgb);
            }
        }
    }

    /// Push the buffer to the pad LEDs (only changed columns are sent). On the
    /// host simulator the whole grid is copied into the shared panel.
    #[inline]
    pub async fn flush(&mut self) {
        #[cfg(target_os = "none")]
        self.leds.flush().await;
        #[cfg(not(target_os = "none"))]
        {
            let grid = self.leds.grid();
            let mut buf = [0u8; deluge_sim_link::ALL_PADS_BYTES];
            for col in 0..Self::COLS {
                for row in 0..Self::ROWS {
                    let o = (col * Self::ROWS + row) * 3;
                    buf[o..o + 3].copy_from_slice(&grid[col][row]);
                }
            }
            crate::host::panel().set_all_pads(&buf);
        }
    }

    /// Set the global LED refresh interval (`0`–`25`); lower is brighter. This is
    /// a single PIC-wide setting, not per-pad. No-op on the host simulator.
    #[inline]
    pub async fn set_brightness_interval(&self, interval: u8) {
        #[cfg(target_os = "none")]
        deluge_bsp::pic::set_refresh_time(interval).await;
        #[cfg(not(target_os = "none"))]
        let _ = interval;
    }
}
