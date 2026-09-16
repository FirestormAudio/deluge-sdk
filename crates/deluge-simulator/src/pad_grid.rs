//! RGB LED pad grid for the Deluge hardware simulator
//!
//! The Deluge has a 16x8 grid of RGB LED pads that can display different colors
//! and respond to touch input.

// Re-export so existing `pad_grid::{RGB, ToIcedColor}` paths keep resolving.
pub use crate::rgb::{RGB, ToIcedColor};

/// The 18x8 RGB LED pad grid (16 main + 2 audition/mute columns)
#[derive(Clone)]
pub struct PadGrid {
    /// Grid of RGB colors (18 columns x 8 rows)
    pads: [[RGB; 18]; 8],
    /// Currently pressed pads (for interaction feedback)
    pressed: [[bool; 18]; 8],
}

impl PadGrid {
    /// Create a new pad grid with all pads off
    pub fn new() -> Self {
        Self {
            pads: [[RGB::black(); 18]; 8],
            pressed: [[false; 18]; 8],
        }
    }

    /// Get the color of a pad at the given position
    pub fn get(&self, col: usize, row: usize) -> RGB {
        if col < 18 && row < 8 {
            self.pads[row][col]
        } else {
            RGB::black()
        }
    }

    /// Set the color of a pad at the given position
    pub fn set(&mut self, col: usize, row: usize, color: RGB) {
        if col < 18 && row < 8 {
            self.pads[row][col] = color;
        }
    }

    /// Check if a pad is currently pressed
    pub fn is_pressed(&self, col: usize, row: usize) -> bool {
        if col < 18 && row < 8 {
            self.pressed[row][col]
        } else {
            false
        }
    }

    /// Set the pressed state of a pad
    pub fn set_pressed(&mut self, col: usize, row: usize, pressed: bool) {
        if col < 18 && row < 8 {
            self.pressed[row][col] = pressed;
        }
    }

    /// Clear all pad colors (set to black)
    pub fn clear(&mut self) {
        self.pads = [[RGB::black(); 18]; 8];
    }

    /// Fill all pads with a single color
    pub fn fill(&mut self, color: RGB) {
        self.pads = [[color; 18]; 8];
    }

    /// Set a rainbow pattern across the grid
    pub fn rainbow_pattern(&mut self) {
        for row in 0..8 {
            for col in 0..18 {
                let hue = (col as f32 / 18.0 + row as f32 / 16.0) * 360.0;
                let color = Self::hue_to_rgb(hue);
                self.set(col, row, color);
            }
        }
    }

    /// Convert HSV hue (0-360) to RGB color
    fn hue_to_rgb(hue: f32) -> RGB {
        let h = hue % 360.0;
        let c = 1.0;
        let x = 1.0 - ((h / 60.0) % 2.0 - 1.0).abs();

        let (r, g, b) = if h < 60.0 {
            (c, x, 0.0)
        } else if h < 120.0 {
            (x, c, 0.0)
        } else if h < 180.0 {
            (0.0, c, x)
        } else if h < 240.0 {
            (0.0, x, c)
        } else if h < 300.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };

        RGB::new((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }

    /// Create a checkerboard pattern
    pub fn checkerboard(&mut self, color1: RGB, color2: RGB) {
        for row in 0..8 {
            for col in 0..16 {
                let color = if (row + col) % 2 == 0 { color1 } else { color2 };
                self.set(col, row, color);
            }
        }
    }
}

impl Default for PadGrid {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pad_grid_creation() {
        let grid = PadGrid::new();
        // All pads should be black initially
        assert_eq!(grid.get(0, 0), RGB::black());
        assert_eq!(grid.get(15, 7), RGB::black());
    }

    #[test]
    fn test_pad_set_get() {
        let mut grid = PadGrid::new();
        grid.set(5, 3, RGB::new(255, 0, 0)); // Red
        assert_eq!(grid.get(5, 3), RGB::new(255, 0, 0));
    }

    #[test]
    fn test_pad_pressed() {
        let mut grid = PadGrid::new();
        assert!(!grid.is_pressed(5, 3));
        grid.set_pressed(5, 3, true);
        assert!(grid.is_pressed(5, 3));
    }

    #[test]
    fn test_clear_fill() {
        let mut grid = PadGrid::new();
        grid.fill(RGB::new(0, 255, 0)); // Green
        assert_eq!(grid.get(0, 0), RGB::new(0, 255, 0));

        grid.clear();
        assert_eq!(grid.get(0, 0), RGB::black());
    }
}
