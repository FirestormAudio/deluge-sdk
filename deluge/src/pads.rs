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
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    /// Full white.
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
    };

    /// A colour from raw 8-bit channels.
    #[inline]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b }
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

    /// Push the buffer to the pad LEDs (only changed columns are sent).
    #[inline]
    pub async fn flush(&mut self) {
        self.leds.flush().await;
    }

    /// Set the global LED refresh interval (`0`–`25`); lower is brighter. This is
    /// a single PIC-wide setting, not per-pad.
    #[inline]
    pub async fn set_brightness_interval(&self, interval: u8) {
        deluge_bsp::pic::set_refresh_time(interval).await;
    }
}
