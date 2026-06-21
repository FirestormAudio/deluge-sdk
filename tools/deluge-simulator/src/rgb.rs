//! Minimal 24-bit RGB colour — a tiny stand-in for spark's `spark_grid::RGB`,
//! carrying only the surface the simulator's pad grid and renderer use.

use iced::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RGB {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RGB {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn black() -> Self {
        Self { r: 0, g: 0, b: 0 }
    }

    /// Scale each channel by `factor` (used to dim a pad colour when pressed).
    pub fn dim_float(&self, factor: f32) -> Self {
        let s = |c: u8| (c as f32 * factor).clamp(0.0, 255.0) as u8;
        Self::new(s(self.r), s(self.g), s(self.b))
    }

    /// Blend each channel toward white by `factor` (used to highlight a lit LED).
    pub fn brighten(&self, factor: f32) -> Self {
        let s = |c: u8| (c as f32 + (255.0 - c as f32) * factor).clamp(0.0, 255.0) as u8;
        Self::new(s(self.r), s(self.g), s(self.b))
    }
}

/// Convert an [`RGB`] to an iced [`Color`].
pub trait ToIcedColor {
    fn to_iced_color(&self) -> Color;
}

impl ToIcedColor for RGB {
    fn to_iced_color(&self) -> Color {
        Color::from_rgb8(self.r, self.g, self.b)
    }
}
