//! RGB pad grid output.

use deluge_bsp::rgb::PadLeds;

/// An RGB colour (0–255 per channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// All channels off.
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    /// Full white.
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(255, 0, 0);
    pub const GREEN: Color = Color::rgb(0, 255, 0);
    pub const BLUE: Color = Color::rgb(0, 0, 255);
    pub const YELLOW: Color = Color::rgb(255, 255, 0);
    pub const CYAN: Color = Color::rgb(0, 255, 255);
    pub const MAGENTA: Color = Color::rgb(255, 0, 255);
    pub const ORANGE: Color = Color::rgb(255, 96, 0);

    /// A colour from raw 8-bit channels.
    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
    }

    /// Scale all channels by `factor`/255 (brightness). `255` = unchanged,
    /// `0` = off, `128` ≈ half.
    #[inline]
    pub const fn scale(self, factor: u8) -> Color {
        Color {
            r: ((self.r as u16 * factor as u16) / 255) as u8,
            g: ((self.g as u16 * factor as u16) / 255) as u8,
            b: ((self.b as u16 * factor as u16) / 255) as u8,
        }
    }

    /// A colour from hue/saturation/value, each 0–255 (`h` wraps the colour
    /// wheel). Handy for rainbows.
    pub fn hsv(h: u8, s: u8, v: u8) -> Color {
        if s == 0 {
            return Color { r: v, g: v, b: v };
        }
        let h6 = (h as u32 * 6) >> 8; // sector 0–5
        let f = (h as u32 * 6) & 0xFF; // fractional part 0–255
        let p = ((v as u32 * (255 - s as u32)) >> 8) as u8;
        let q = ((v as u32 * (255 - ((s as u32 * f) >> 8))) >> 8) as u8;
        let t = ((v as u32 * (255 - ((s as u32 * (255 - f)) >> 8))) >> 8) as u8;
        let (r, g, b) = match h6 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Color { r, g, b }
    }

    #[inline]
    fn to_rgb(self) -> [u8; 3] {
        [self.r, self.g, self.b]
    }
}

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
