//! Simulator display implementing embedded-graphics DrawTarget
//!
//! This module provides a display buffer that can be drawn to using embedded-graphics
//! primitives and then rendered to an iced canvas.

use embedded_graphics::{Pixel, pixelcolor::BinaryColor, prelude::*};

/// A display buffer compatible with embedded-graphics that can be rendered with iced
#[derive(Clone)]
pub struct SimulatorDisplay {
    /// Width of the display in pixels (128 for Deluge OLED)
    width: usize,
    /// Height of the display in pixels (43 for Deluge OLED)
    height: usize,
    /// Pixel buffer (packed bits, row-major)
    buffer: Vec<u8>,
}

impl SimulatorDisplay {
    /// Create a new simulator display with Deluge OLED dimensions (128x43)
    pub fn new() -> Self {
        Self::with_size(128, 43)
    }

    /// Create a simulator display with custom dimensions
    pub fn with_size(width: usize, height: usize) -> Self {
        let buffer_size = (width * height).div_ceil(8); // Round up to nearest byte
        Self {
            width,
            height,
            buffer: vec![0u8; buffer_size],
        }
    }

    /// Get the width of the display
    pub fn width(&self) -> usize {
        self.width
    }

    /// Get the height of the display
    pub fn height(&self) -> usize {
        self.height
    }

    /// Get a pixel value at the given coordinates
    pub fn get_pixel(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }

        let index = y * self.width + x;
        let byte_index = index / 8;
        let bit_index = index % 8;

        (self.buffer[byte_index] & (1 << bit_index)) != 0
    }

    /// Set a pixel value at the given coordinates
    pub fn set_pixel(&mut self, x: usize, y: usize, value: bool) {
        if x >= self.width || y >= self.height {
            return;
        }

        let index = y * self.width + x;
        let byte_index = index / 8;
        let bit_index = index % 8;

        if value {
            self.buffer[byte_index] |= 1 << bit_index;
        } else {
            self.buffer[byte_index] &= !(1 << bit_index);
        }
    }

    /// Clear the display (set all pixels to off)
    pub fn clear_display(&mut self) {
        self.buffer.fill(0);
    }

    /// Fill the display (set all pixels to on)
    pub fn fill_display(&mut self) {
        self.buffer.fill(0xFF);
    }

    /// Get the raw pixel buffer for rendering
    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }
}

impl Default for SimulatorDisplay {
    fn default() -> Self {
        Self::new()
    }
}

// Implement embedded-graphics traits
impl OriginDimensions for SimulatorDisplay {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

impl DrawTarget for SimulatorDisplay {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if coord.x >= 0 && coord.y >= 0 {
                let x = coord.x as usize;
                let y = coord.y as usize;
                self.set_pixel(x, y, color.is_on());
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        if color.is_on() {
            self.fill_display();
        } else {
            self.clear_display();
        }
        Ok(())
    }
}
