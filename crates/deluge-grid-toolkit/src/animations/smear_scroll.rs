//! Smooth smear scroll animations.
//!
//! Hardware-PIC-like smooth scrolling with sub-pixel interpolation and motion
//! blur. A smooth "smear" effect is produced by blending colours between pixel
//! positions, rather than scrolling pixel-by-pixel.

use super::{Animation, ScrollDirection};
use crate::color::ColorExt as _;
#[allow(unused_imports)] // needed on targets whose `core` lacks inherent f32 math
use crate::float_ext::F32Ext as _;
use crate::grid::GridRgb;
use crate::{Color, Grid};

/// Horizontal smear scroll animation with sub-pixel interpolation.
pub struct HorizontalSmearScrollAnimation {
    from_grid: GridRgb,
    to_grid: GridRgb,
    direction: ScrollDirection,
    scroll_to_black: bool,
    duration_ms: f32,
    elapsed_ms: f32,
    area_width: usize,
}

impl HorizontalSmearScrollAnimation {
    /// Create a new horizontal smear scroll animation.
    ///
    /// # Panics
    /// If `direction` is not `Left`/`Right`.
    pub fn new(
        from: Grid,
        to: Grid,
        direction: ScrollDirection,
        scroll_to_black: bool,
        duration_ms: u32,
    ) -> Self {
        assert!(
            direction == ScrollDirection::Left || direction == ScrollDirection::Right,
            "HorizontalSmearScroll only supports Left/Right"
        );

        Self {
            from_grid: from.to_rgb(),
            to_grid: to.to_rgb(),
            direction,
            scroll_to_black,
            duration_ms: duration_ms as f32,
            elapsed_ms: 0.0,
            area_width: 18,
        }
    }

    fn render_frame(&self, progress: f32) -> Grid {
        let mut output_rgb = [[Color::BLACK; 18]; 8];

        let scroll_distance = progress * self.area_width as f32;
        let scroll_dir = match self.direction {
            ScrollDirection::Left => -1.0,
            ScrollDirection::Right => 1.0,
            _ => 0.0,
        };

        for (row, row_out) in output_rgb.iter_mut().enumerate() {
            for (x, pixel) in row_out[..self.area_width].iter_mut().enumerate() {
                let source_x = x as f32 - (scroll_distance * scroll_dir);
                let source_x_int = source_x.floor() as i32;
                let source_x_frac = source_x.fract();

                let color_a = self.get_source_color(row, source_x_int);
                let color_b = self.get_source_color(row, source_x_int + 1);

                let blend_amount = (source_x_frac * 65535.0) as u16;
                *pixel = Color::blend_static(color_a, color_b, blend_amount);
            }
        }

        Grid::from_rgb(output_rgb)
    }

    fn get_source_color(&self, row: usize, x: i32) -> Color {
        if x < 0 || x >= self.area_width as i32 {
            if self.scroll_to_black {
                Color::BLACK
            } else {
                let target_x = if x < 0 {
                    (self.area_width as i32 + x) as usize
                } else {
                    (x - self.area_width as i32) as usize
                };
                if target_x < self.area_width {
                    self.to_grid[row][target_x]
                } else {
                    Color::BLACK
                }
            }
        } else {
            self.from_grid[row][x as usize]
        }
    }
}

impl Animation for HorizontalSmearScrollAnimation {
    fn tick(&mut self, delta_ms: f32) -> Option<Grid> {
        if self.is_complete() {
            return None;
        }

        self.elapsed_ms += delta_ms;
        let progress = (self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0);

        if progress >= 1.0 {
            if self.scroll_to_black {
                Some(Grid::from_rgb([[Color::BLACK; 18]; 8]))
            } else {
                Some(Grid::from_rgb(self.to_grid))
            }
        } else {
            Some(self.render_frame(progress))
        }
    }

    fn duration_ms(&self) -> f32 {
        self.duration_ms
    }

    fn is_complete(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }
}

/// Vertical smear scroll animation with sub-pixel interpolation.
pub struct VerticalSmearScrollAnimation {
    from_grid: GridRgb,
    to_grid: GridRgb,
    direction: ScrollDirection,
    scroll_to_black: bool,
    duration_ms: f32,
    elapsed_ms: f32,
}

impl VerticalSmearScrollAnimation {
    /// Create a new vertical smear scroll animation.
    ///
    /// # Panics
    /// If `direction` is not `Up`/`Down`.
    pub fn new(
        from: Grid,
        to: Grid,
        direction: ScrollDirection,
        scroll_to_black: bool,
        duration_ms: u32,
    ) -> Self {
        assert!(
            direction == ScrollDirection::Up || direction == ScrollDirection::Down,
            "VerticalSmearScroll only supports Up/Down"
        );

        Self {
            from_grid: from.to_rgb(),
            to_grid: to.to_rgb(),
            direction,
            scroll_to_black,
            duration_ms: duration_ms as f32,
            elapsed_ms: 0.0,
        }
    }

    fn render_frame(&self, progress: f32) -> Grid {
        let mut output_rgb = [[Color::BLACK; 18]; 8];

        let scroll_distance = progress * 8.0;
        let scroll_dir = match self.direction {
            ScrollDirection::Up => -1.0,
            ScrollDirection::Down => 1.0,
            _ => 0.0,
        };

        for (row, row_out) in output_rgb.iter_mut().enumerate() {
            for (x, pixel) in row_out.iter_mut().enumerate() {
                let source_y = row as f32 - (scroll_distance * scroll_dir);
                let source_y_int = source_y.floor() as i32;
                let source_y_frac = source_y.fract();

                let color_a = self.get_source_color(source_y_int, x);
                let color_b = self.get_source_color(source_y_int + 1, x);

                let blend_amount = (source_y_frac * 65535.0) as u16;
                *pixel = Color::blend_static(color_a, color_b, blend_amount);
            }
        }

        Grid::from_rgb(output_rgb)
    }

    fn get_source_color(&self, y: i32, x: usize) -> Color {
        if !(0..8).contains(&y) {
            if self.scroll_to_black {
                Color::BLACK
            } else {
                let target_y = if y < 0 {
                    (8 + y) as usize
                } else {
                    (y - 8) as usize
                };
                if target_y < 8 {
                    self.to_grid[target_y][x]
                } else {
                    Color::BLACK
                }
            }
        } else {
            self.from_grid[y as usize][x]
        }
    }
}

impl Animation for VerticalSmearScrollAnimation {
    fn tick(&mut self, delta_ms: f32) -> Option<Grid> {
        if self.is_complete() {
            return None;
        }

        self.elapsed_ms += delta_ms;
        let progress = (self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0);

        if progress >= 1.0 {
            if self.scroll_to_black {
                Some(Grid::from_rgb([[Color::BLACK; 18]; 8]))
            } else {
                Some(Grid::from_rgb(self.to_grid))
            }
        } else {
            Some(self.render_frame(progress))
        }
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

    #[test]
    fn test_horizontal_smear_scroll_creation() {
        let anim = HorizontalSmearScrollAnimation::new(
            Grid::default(),
            Grid::default(),
            ScrollDirection::Right,
            false,
            300,
        );
        assert_eq!(anim.duration_ms(), 300.0);
        assert!(!anim.is_complete());
    }

    #[test]
    #[should_panic(expected = "only supports Left/Right")]
    fn test_horizontal_rejects_vertical_direction() {
        HorizontalSmearScrollAnimation::new(
            Grid::default(),
            Grid::default(),
            ScrollDirection::Up,
            false,
            300,
        );
    }

    #[test]
    #[should_panic(expected = "only supports Up/Down")]
    fn test_vertical_rejects_horizontal_direction() {
        VerticalSmearScrollAnimation::new(
            Grid::default(),
            Grid::default(),
            ScrollDirection::Left,
            false,
            300,
        );
    }
}
