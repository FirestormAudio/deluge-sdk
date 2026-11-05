//! Scroll transition animations.
//!
//! Horizontal and vertical scrolling between grid states.
//! Ported from Deluge `pad_leds.cpp` (horizontal/vertical `renderScroll`).

use super::Animation;
use crate::grid::GridRgb;
use crate::{Color, Grid};

/// Direction for scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Horizontal scroll animation.
pub struct HorizontalScrollAnimation {
    from_grid: GridRgb,
    to_grid: GridRgb,
    direction: ScrollDirection,
    scroll_to_black: bool,
    squares_scrolled: u8,
    area_to_scroll: u8,
    duration_ms: f32,
    elapsed_ms: f32,
}

impl HorizontalScrollAnimation {
    /// Create a new horizontal scroll animation.
    pub fn new(
        from: Grid,
        to: Grid,
        direction: ScrollDirection,
        scroll_to_black: bool,
        duration_ms: u32,
    ) -> Self {
        let area_to_scroll = 18;
        Self {
            from_grid: from.to_rgb(),
            to_grid: to.to_rgb(),
            direction,
            scroll_to_black,
            squares_scrolled: 0,
            area_to_scroll,
            duration_ms: duration_ms as f32,
            elapsed_ms: 0.0,
        }
    }

    fn render_frame(&mut self) -> Grid {
        let mut output_rgb = self.from_grid;
        let scroll_dir = match self.direction {
            ScrollDirection::Left => -1i8,
            ScrollDirection::Right => 1i8,
            _ => 0,
        };

        let copy_col = if scroll_dir > 0 {
            self.squares_scrolled as i8 - 1
        } else {
            self.area_to_scroll as i8 - self.squares_scrolled as i8
        };

        let start_square = if scroll_dir > 0 {
            0i8
        } else {
            self.area_to_scroll as i8 - 1
        };
        let end_square = if scroll_dir > 0 {
            self.area_to_scroll as i8 - 1
        } else {
            0i8
        };

        for (row, row_pixels) in output_rgb.iter_mut().enumerate() {
            let mut x = start_square;
            while x != end_square {
                let next_x = x + scroll_dir;
                if (0..18).contains(&next_x) && (0..18).contains(&x) {
                    let color = row_pixels[next_x as usize];
                    row_pixels[x as usize] = color;
                }
                x += scroll_dir;
            }

            if (0..18).contains(&end_square) {
                let new_color = if self.scroll_to_black {
                    Color::BLACK
                } else if (0..18).contains(&copy_col) {
                    self.to_grid[row][copy_col as usize]
                } else {
                    Color::BLACK
                };
                row_pixels[end_square as usize] = new_color;
            }
        }

        self.squares_scrolled += 1;
        Grid::from_rgb(output_rgb)
    }
}

impl Animation for HorizontalScrollAnimation {
    fn tick(&mut self, delta_ms: f32) -> Option<Grid> {
        if self.is_complete() {
            return None;
        }

        self.elapsed_ms += delta_ms;
        let progress = (self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0);
        let target_squares = (progress * self.area_to_scroll as f32) as u8;

        if self.squares_scrolled < target_squares {
            Some(self.render_frame())
        } else if self.squares_scrolled >= self.area_to_scroll {
            None
        } else {
            Some(Grid::from_rgb(self.from_grid))
        }
    }

    fn duration_ms(&self) -> f32 {
        self.duration_ms
    }

    fn is_complete(&self) -> bool {
        self.squares_scrolled >= self.area_to_scroll
    }
}

/// Vertical scroll animation.
pub struct VerticalScrollAnimation {
    from_grid: GridRgb,
    to_grid: GridRgb,
    direction: ScrollDirection,
    scroll_to_black: bool,
    squares_scrolled: u8,
    duration_ms: f32,
    elapsed_ms: f32,
}

impl VerticalScrollAnimation {
    /// Create a new vertical scroll animation.
    pub fn new(
        from: Grid,
        to: Grid,
        direction: ScrollDirection,
        scroll_to_black: bool,
        duration_ms: u32,
    ) -> Self {
        Self {
            from_grid: from.to_rgb(),
            to_grid: to.to_rgb(),
            direction,
            scroll_to_black,
            squares_scrolled: 0,
            duration_ms: duration_ms as f32,
            elapsed_ms: 0.0,
        }
    }

    fn render_frame(&mut self) -> Grid {
        let mut output_rgb = [[Color::BLACK; 18]; 8];
        let scroll_dir = match self.direction {
            ScrollDirection::Up => -1i8,
            ScrollDirection::Down => 1i8,
            _ => 0,
        };

        let copy_row = if scroll_dir > 0 {
            self.squares_scrolled as i8 - 1
        } else {
            8 - self.squares_scrolled as i8
        };

        let start_square = if scroll_dir > 0 { 0i8 } else { 1i8 };
        let end_square = if scroll_dir > 0 { 7i8 } else { 0i8 };

        let mut y = start_square;
        while (scroll_dir > 0 && y < end_square) || (scroll_dir < 0 && y > end_square) {
            let source_row = (y - scroll_dir) as usize;
            if source_row < 8 && (0..8).contains(&y) {
                output_rgb[y as usize] = self.from_grid[source_row];
            }
            y += scroll_dir;
        }

        if (0..8).contains(&end_square) {
            let new_row = if self.scroll_to_black {
                [Color::BLACK; 18]
            } else if (0..8).contains(&copy_row) {
                self.to_grid[copy_row as usize]
            } else {
                [Color::BLACK; 18]
            };
            output_rgb[end_square as usize] = new_row;
        }

        self.squares_scrolled += 1;
        Grid::from_rgb(output_rgb)
    }
}

impl Animation for VerticalScrollAnimation {
    fn tick(&mut self, delta_ms: f32) -> Option<Grid> {
        if self.is_complete() {
            return None;
        }

        self.elapsed_ms += delta_ms;
        let progress = (self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0);
        let target_squares = (progress * 8.0) as u8;

        if self.squares_scrolled < target_squares {
            Some(self.render_frame())
        } else if self.squares_scrolled >= 8 {
            None
        } else {
            Some(Grid::from_rgb(self.from_grid))
        }
    }

    fn duration_ms(&self) -> f32 {
        self.duration_ms
    }

    fn is_complete(&self) -> bool {
        self.squares_scrolled >= 8
    }
}
