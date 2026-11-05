//! Explode/implode transition animation.
//!
//! Expands a single grid cell to the full grid or collapses the full grid to a
//! cell, via bilinear interpolation with fixed-point arithmetic.
//! Ported from Deluge `pad_leds.cpp` (`renderExplodeAnimation`).

use super::Animation;
use crate::grid::GridRgb;
use crate::{Color, Grid};

/// Direction for the explode animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplodeDirection {
    /// Grid cell expands to the full grid.
    Explode,
    /// Full grid collapses to a grid cell.
    Implode,
}

/// Explode/implode animation with bilinear interpolation.
pub struct ExplodeAnimation {
    from_grid: GridRgb,
    to_grid: GridRgb,
    direction: ExplodeDirection,
    origin_x: f32,
    origin_y: f32,
    duration_ms: f32,
    elapsed_ms: f32,
}

impl ExplodeAnimation {
    /// Create a new explode animation.
    pub fn new(
        from: Grid,
        to: Grid,
        direction: ExplodeDirection,
        origin_x: f32,
        origin_y: f32,
        duration_ms: u32,
    ) -> Self {
        Self {
            from_grid: from.to_rgb(),
            to_grid: to.to_rgb(),
            direction,
            origin_x,
            origin_y,
            duration_ms: duration_ms as f32,
            elapsed_ms: 0.0,
        }
    }

    fn render_frame(&self, explodedness: f32) -> Grid {
        let mut output_rgb = [[Color::BLACK; 18]; 8];

        let source = match self.direction {
            ExplodeDirection::Explode => &self.from_grid,
            ExplodeDirection::Implode => &self.to_grid,
        };

        let actual_explodedness = match self.direction {
            ExplodeDirection::Explode => explodedness,
            ExplodeDirection::Implode => 1.0 - explodedness,
        };

        let origin_x_big = (self.origin_x * 65536.0) as i32;
        let origin_y_big = (self.origin_y * 65536.0) as i32;

        for (y_source, source_row) in source.iter().enumerate() {
            for (x_source, &source_color) in source_row.iter().enumerate() {
                if source_color == Color::BLACK {
                    continue;
                }

                let x_source_big = (x_source as i32) << 16;
                let y_source_big = (y_source as i32) << 16;

                let x_offset = x_source_big - origin_x_big;
                let y_offset = y_source_big - origin_y_big;

                let x_dest_big = origin_x_big
                    + ((x_offset as i64 * (actual_explodedness * 65536.0) as i64) >> 16) as i32;
                let y_dest_big = origin_y_big
                    + ((y_offset as i64 * (actual_explodedness * 65536.0) as i64) >> 16) as i32;

                let x_dest = x_dest_big >> 16;
                let y_dest = y_dest_big >> 16;
                let x_frac = (x_dest_big & 0xFFFF) as u32;
                let y_frac = (y_dest_big & 0xFFFF) as u32;

                let x_intensity = [65536 - x_frac, x_frac];
                let y_intensity = [65536 - y_frac, y_frac];

                for y_offset_idx in 0..2 {
                    let y_now = y_dest + y_offset_idx;
                    if !(0..8).contains(&y_now) {
                        continue;
                    }

                    for x_offset_idx in 0..2 {
                        let x_now = x_dest + x_offset_idx;
                        if !(0..18).contains(&x_now) {
                            continue;
                        }

                        let intensity = ((y_intensity[y_offset_idx as usize] as u64
                            * x_intensity[x_offset_idx as usize] as u64)
                            >> 16) as u32;

                        let existing = output_rgb[y_now as usize][x_now as usize];
                        let blended = Self::blend_pixel(source_color, existing, intensity);
                        output_rgb[y_now as usize][x_now as usize] = blended;
                    }
                }
            }
        }

        Grid::from_rgb(output_rgb)
    }

    fn blend_pixel(source: Color, dest: Color, intensity: u32) -> Color {
        let r = ((source.r as u32 * intensity) >> 16) + dest.r as u32;
        let g = ((source.g as u32 * intensity) >> 16) + dest.g as u32;
        let b = ((source.b as u32 * intensity) >> 16) + dest.b as u32;
        Color::rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8)
    }
}

impl Animation for ExplodeAnimation {
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
    fn test_explode_animation() {
        let mut from = Grid::new();
        let to = Grid::new();

        from.set_pad(4, 9, Color::red());
        from.set_pad(3, 9, Color::green());
        from.set_pad(5, 9, Color::blue());
        from.set_pad(4, 8, Color::rgb(255, 255, 0));
        from.set_pad(4, 10, Color::rgb(0, 255, 255));

        let mut anim = ExplodeAnimation::new(from, to, ExplodeDirection::Explode, 9.0, 4.0, 300);

        let frame0 = anim.tick(0.0).unwrap();
        assert!(frame0.get_pad(4, 9).r > 0);

        let frame_mid = anim.tick(150.0).unwrap();
        let mut lit_pixels = 0;
        for row in 0..8 {
            for col in 0..18 {
                if frame_mid.get_pad(row, col) != Color::BLACK {
                    lit_pixels += 1;
                }
            }
        }
        assert!(lit_pixels >= 5);
    }
}
