//! Fade transition animation.
//!
//! Linear interpolation between two complete grid states.
//! Ported from `renderFade()` in Deluge `pad_leds.cpp`.

use super::Animation;
use crate::color::lerp_slice;
use crate::grid::GridRgb;
use crate::{Color, Grid};

/// Fade animation between two grids.
pub struct FadeAnimation {
    from_grid: GridRgb,
    to_grid: GridRgb,
    duration_ms: f32,
    elapsed_ms: f32,
}

impl FadeAnimation {
    /// Create a new fade animation.
    pub fn new(from: Grid, to: Grid, duration_ms: u32) -> Self {
        Self {
            from_grid: from.to_rgb(),
            to_grid: to.to_rgb(),
            duration_ms: duration_ms as f32,
            elapsed_ms: 0.0,
        }
    }

    fn render_frame(&self, progress: f32) -> Grid {
        let mut output_rgb = [[Color::BLACK; 18]; 8];
        let from_flat = self.from_grid.as_flattened();
        let to_flat = self.to_grid.as_flattened();
        let out_flat = output_rgb.as_flattened_mut();
        lerp_slice(from_flat, to_flat, progress, out_flat);
        Grid::from_rgb(output_rgb)
    }
}

impl Animation for FadeAnimation {
    fn tick(&mut self, delta_ms: f32) -> Option<Grid> {
        if self.is_complete() {
            return None;
        }
        self.elapsed_ms += delta_ms;
        let progress = (self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0);
        Some(self.render_frame(progress))
    }

    fn duration_ms(&self) -> f32 {
        self.duration_ms
    }

    fn is_complete(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorExt as _;

    #[test]
    fn test_fade_animation() {
        let mut from = Grid::new();
        let mut to = Grid::new();
        for row in 0..8 {
            for col in 0..18 {
                from.set_pad(row, col, Color::red());
                to.set_pad(row, col, Color::blue());
            }
        }

        let mut anim = FadeAnimation::new(from, to, 100);

        let frame0 = anim.tick(0.0).unwrap();
        assert_eq!(frame0.get_pad(0, 0), Color::red());

        let frame50 = anim.tick(50.0).unwrap();
        let mid_color = frame50.get_pad(0, 0);
        assert!(mid_color.r > 0 && mid_color.b > 0);

        let frame100 = anim.tick(50.0).unwrap();
        assert_eq!(frame100.get_pad(0, 0), Color::blue());

        assert!(anim.is_complete());
        assert!(anim.tick(10.0).is_none());
    }
}
