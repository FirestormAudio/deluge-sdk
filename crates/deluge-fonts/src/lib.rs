#![no_std]

//! Font family for embedded-graphics from Deluge firmware.
//!
//! This crate provides multiple fonts extracted from the Synthstrom Audible Deluge firmware:
//!
//! ## Available Fonts
//!
//! - **font_5px**: Original 5px font (U+0020 to U+005A)
//! - **font_apple**: Apple II 7px font (U+0020 to U+005A)
//! - **metric_bold_9px**: Metric Bold 9px (U+0020 to U+007E)
//! - **metric_bold_13px**: Metric Bold 13px (U+0020 to U+007E)
//! - **metric_bold_20px**: Metric Bold 20px (U+0020 to U+007F)
//!
//! ## Seven-Segment Display Emulation
//!
//! The `seven_segment` module provides rendering of the Deluge's 7-segment LED display
//! as graphics on an OLED screen, matching the hardware appearance.
//!
//! ## Licensing
//!
//! The "Metric" font is a proprietary font licensed to Synthstrom Audible Limited
//! from Klim Type Foundry (https://klim.co.nz/). This font is NOT free to use
//! in other projects.
//!
//! ## Font Format
//!
//! All fonts are stored in **row-major** format, rotated from the original column-major
//! OLED format during build time. Each row's pixels are packed into bytes, left-to-right,
//! making them easier to work with in standard graphics libraries.
//!
//! For each glyph:
//! - `bytes_per_row = (width + 7) / 8` (rounded up to nearest byte)
//! - Rows are stored sequentially from top to bottom
//! - Within each byte, bit 0 is the leftmost pixel
//!
//! ## Usage
//!
//! ```no_run
//! use embedded_fonts_deluge::*;
//! use embedded_graphics::prelude::*;
//!
//! // Use the convenient Font enum
//! let font = Font::MetricBold9px;
//!
//! // Or access font data directly
//! let descriptors = &METRIC_BOLD_9PX_DESCRIPTORS;
//! let bitmap = &METRIC_BOLD_9PX_BITMAP;
//! let height = METRIC_BOLD_9PX_HEIGHT;
//! ```

#[cfg(feature = "embedded-graphics")]
use embedded_graphics::{Pixel, pixelcolor::BinaryColor, prelude::*};

pub mod seven_segment;

/// Glyph descriptor for variable-width fonts.
///
/// Each glyph has:
/// - Variable width (stored in `w_px`)
/// - Fixed height (depends on font variant)
/// - Bitmap data starting at `glyph_index` in the font data array
#[derive(Debug, Clone, Copy)]
pub struct GlyphDescriptor {
    /// Width of this glyph in pixels
    pub w_px: u8,
    /// Starting index in the font bitmap array for this glyph
    pub glyph_index: u16,
}

/// Available font variants from the Deluge firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Original 5px font (U+0020 to U+005A)
    Font5px,
    /// Apple II 7px font (U+0020 to U+005A)
    FontApple,
    /// Metric Bold 9px (U+0020 to U+007E)
    MetricBold9px,
    /// Metric Bold 13px (U+0020 to U+007E)
    MetricBold13px,
    /// Metric Bold 20px (U+0020 to U+007F)
    MetricBold20px,
}

impl Font {
    /// Get the bitmap data for this font.
    pub const fn bitmap(&self) -> &'static [u8] {
        match self {
            Font::Font5px => FONT_5PX_BITMAP,
            Font::FontApple => FONT_APPLE_BITMAP,
            Font::MetricBold9px => METRIC_BOLD_9PX_BITMAP,
            Font::MetricBold13px => METRIC_BOLD_13PX_BITMAP,
            Font::MetricBold20px => METRIC_BOLD_20PX_BITMAP,
        }
    }

    /// Get the glyph descriptors for this font.
    pub const fn descriptors(&self) -> &'static [GlyphDescriptor] {
        match self {
            Font::Font5px => FONT_5PX_DESCRIPTORS,
            Font::FontApple => FONT_APPLE_DESCRIPTORS,
            Font::MetricBold9px => METRIC_BOLD_9PX_DESCRIPTORS,
            Font::MetricBold13px => METRIC_BOLD_13PX_DESCRIPTORS,
            Font::MetricBold20px => METRIC_BOLD_20PX_DESCRIPTORS,
        }
    }

    /// Get the height of this font in pixels.
    pub const fn height(&self) -> u8 {
        match self {
            Font::Font5px => FONT_5PX_HEIGHT,
            Font::FontApple => FONT_APPLE_HEIGHT,
            Font::MetricBold9px => METRIC_BOLD_9PX_HEIGHT,
            Font::MetricBold13px => METRIC_BOLD_13PX_HEIGHT,
            Font::MetricBold20px => METRIC_BOLD_20PX_HEIGHT,
        }
    }

    /// Get the baseline offset for this font.
    pub const fn baseline(&self) -> u8 {
        match self {
            Font::Font5px => FONT_5PX_BASELINE,
            Font::FontApple => FONT_APPLE_BASELINE,
            Font::MetricBold9px => METRIC_BOLD_9PX_BASELINE,
            Font::MetricBold13px => METRIC_BOLD_13PX_BASELINE,
            Font::MetricBold20px => METRIC_BOLD_20PX_BASELINE,
        }
    }

    /// Draw a single glyph at the specified position with custom color.
    ///
    /// Returns the width of the drawn glyph in pixels.
    #[cfg(feature = "embedded-graphics")]
    pub fn draw_glyph_colored<D>(
        &self,
        target: &mut D,
        glyph: &GlyphDescriptor,
        position: Point,
        color: BinaryColor,
    ) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let bitmap = self.bitmap();
        let height = self.height();
        let width = glyph.w_px as usize;
        let glyph_index = glyph.glyph_index as usize;

        let bytes_per_row = width.div_ceil(8);

        for row in 0..height as usize {
            let row_start = glyph_index + row * bytes_per_row;

            for byte_idx in 0..bytes_per_row {
                if row_start + byte_idx >= bitmap.len() {
                    break;
                }

                let byte = bitmap[row_start + byte_idx];

                for bit in 0..8 {
                    let x = byte_idx * 8 + bit;
                    if x < width && (byte & (1 << bit)) != 0 {
                        Pixel(
                            Point::new(position.x + x as i32, position.y + row as i32),
                            color,
                        )
                        .draw(target)?;
                    }
                }
            }
        }

        Ok(width as i32)
    }

    /// Draw a single glyph at the specified position.
    ///
    /// Returns the width of the drawn glyph in pixels.
    #[cfg(feature = "embedded-graphics")]
    pub fn draw_glyph<D>(
        &self,
        target: &mut D,
        glyph: &GlyphDescriptor,
        position: Point,
    ) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        self.draw_glyph_colored(target, glyph, position, BinaryColor::On)
    }

    /// Draw text at the specified position with custom color and spacing.
    ///
    /// Returns the total width of the drawn text in pixels.
    /// Note: Text is automatically converted to uppercase since these fonts only contain uppercase glyphs.
    #[cfg(feature = "embedded-graphics")]
    pub fn draw_text_colored_with_spacing<D>(
        &self,
        target: &mut D,
        text: &str,
        position: Point,
        color: BinaryColor,
        spacing: i32,
    ) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let descriptors = self.descriptors();
        let mut x_offset = 0;

        // Convert to uppercase since the font only has uppercase glyphs
        for ch in text.chars() {
            let ch = ch.to_ascii_uppercase();
            let char_index = if (' '..='~').contains(&ch) {
                (ch as usize) - (' ' as usize)
            } else if ch == '♭' {
                95
            } else {
                continue;
            };

            if char_index >= descriptors.len() {
                continue;
            }

            let descriptor = &descriptors[char_index];
            let glyph_width = self.draw_glyph_colored(
                target,
                descriptor,
                Point::new(position.x + x_offset, position.y),
                color,
            )?;

            x_offset += glyph_width + spacing;
        }

        Ok(x_offset)
    }

    /// Draw text at the specified position with 1px spacing between glyphs.
    ///
    /// Returns the total width of the drawn text in pixels.
    #[cfg(feature = "embedded-graphics")]
    pub fn draw_text<D>(&self, target: &mut D, text: &str, position: Point) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        self.draw_text_colored_with_spacing(target, text, position, BinaryColor::On, 1)
    }

    /// Draw text at the specified position with custom spacing between glyphs.
    ///
    /// Returns the total width of the drawn text in pixels.
    #[cfg(feature = "embedded-graphics")]
    pub fn draw_text_with_spacing<D>(
        &self,
        target: &mut D,
        text: &str,
        position: Point,
        spacing: i32,
    ) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        self.draw_text_colored_with_spacing(target, text, position, BinaryColor::On, spacing)
    }

    /// Calculate the width of text in pixels with 1px spacing between glyphs.
    ///
    /// This accurately calculates the width based on actual glyph widths, not an approximation.
    pub fn text_width(&self, text: &str) -> i32 {
        self.text_width_with_spacing(text, 1)
    }

    /// Calculate the width of text in pixels with custom spacing between glyphs.
    ///
    /// This accurately calculates the width based on actual glyph widths, not an approximation.
    pub fn text_width_with_spacing(&self, text: &str, spacing: i32) -> i32 {
        let descriptors = self.descriptors();
        let mut width = 0;
        let mut first = true;

        // Convert to uppercase since the font only has uppercase glyphs
        for ch in text.chars() {
            let ch = ch.to_ascii_uppercase();
            let char_index = if (' '..='~').contains(&ch) {
                (ch as usize) - (' ' as usize)
            } else if ch == '♭' {
                95
            } else {
                continue;
            };

            if char_index >= descriptors.len() {
                continue;
            }

            let descriptor = &descriptors[char_index];

            if !first {
                width += spacing;
            }
            width += descriptor.w_px as i32;
            first = false;
        }

        width
    }
}

pub mod font_5px;
pub mod font_apple;
pub mod metric_bold_13px;
pub mod metric_bold_20px;
pub mod metric_bold_9px;

pub use font_5px::*;
pub use font_apple::*;
pub use metric_bold_9px::*;
pub use metric_bold_13px::*;
pub use metric_bold_20px::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_data_available() {
        // Basic test to ensure font data is generated
        assert!(METRIC_BOLD_9PX_BITMAP.len() > 0);
        assert!(METRIC_BOLD_9PX_DESCRIPTORS.len() > 0);
    }

    #[test]
    fn check_space_glyph() {
        // Space character is first (index 0)
        let space_desc = METRIC_BOLD_9PX_DESCRIPTORS[0];
        assert_eq!(space_desc.w_px, 3); // Space is 3 pixels wide in the 9px font
    }
}
