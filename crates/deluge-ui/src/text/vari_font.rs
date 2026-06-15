//! Variable-width bitmap fonts for Deluge display.
//!
//! This module provides `VariFont` - a counterpart to embedded-graphics' `MonoFont`
//! that works with variable-width fonts from embedded-fonts-deluge.

use embedded_fonts_deluge::{Font as DelugeFont, GlyphDescriptor};

/// Variable-width bitmap font.
///
/// `VariFont` is designed to work with the Deluge variable-width fonts from
/// `embedded-fonts-deluge`. Unlike monospaced fonts where every glyph has the same width,
/// variable-width fonts store individual width information for each glyph.
///
/// # Example
///
/// ```no_run
/// use deluge_ui_toolkit::text::VariFont;
/// use embedded_fonts_deluge::Font as DelugeFont;
/// use embedded_graphics::prelude::*;
///
/// let font = VariFont::new(DelugeFont::MetricBold13px);
/// let height = font.height();
/// let width = font.text_width("Hello");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariFont {
    /// The underlying Deluge font
    pub font: DelugeFont,

    /// Spacing between characters in pixels
    pub character_spacing: u32,

    /// The baseline offset from the top of the glyph
    pub baseline: u32,
}

impl VariFont {
    /// Create a new variable-width font from a Deluge font.
    ///
    /// Uses default character spacing of 1 pixel.
    pub const fn new(font: DelugeFont) -> Self {
        Self {
            font,
            character_spacing: 1,
            baseline: font.baseline() as u32,
        }
    }

    /// Create a new variable-width font with custom character spacing.
    pub const fn with_character_spacing(deluge_font: DelugeFont, spacing: u32) -> Self {
        Self {
            font: deluge_font,
            character_spacing: spacing,
            baseline: deluge_font.baseline() as u32,
        }
    }

    /// Get the underlying Deluge font.
    pub const fn font(&self) -> DelugeFont {
        self.font
    }

    /// Get the height of the font in pixels.
    pub const fn height(&self) -> u32 {
        self.font.height() as u32
    }

    /// Calculate the width of a text string in pixels.
    ///
    /// This takes into account the actual glyph widths and character spacing.
    pub fn text_width(&self, text: &str) -> i32 {
        self.font
            .text_width_with_spacing(text, self.character_spacing as i32)
    }

    /// Get the glyph descriptor for a character.
    ///
    /// Returns `None` if the character is not in the font's character set.
    pub fn glyph(&self, c: char) -> Option<&GlyphDescriptor> {
        let ch = c.to_ascii_uppercase();
        let char_index = if (' '..='~').contains(&ch) {
            (ch as usize) - (' ' as usize)
        } else if ch == '♭' {
            95
        } else {
            return None;
        };

        let descriptors = self.font.descriptors();
        descriptors.get(char_index)
    }
}

impl Default for VariFont {
    fn default() -> Self {
        Self::new(DelugeFont::MetricBold13px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_creation() {
        let font = VariFont::new(DelugeFont::MetricBold9px);
        assert_eq!(font.height(), 9);
        assert_eq!(font.character_spacing, 1);
    }

    #[test]
    fn test_custom_spacing() {
        let font = VariFont::with_character_spacing(DelugeFont::MetricBold9px, 2);
        assert_eq!(font.character_spacing, 2);
    }

    #[test]
    fn test_text_width() {
        let font = VariFont::new(DelugeFont::MetricBold9px);
        let width = font.text_width("A");
        assert!(width > 0);
    }

    #[test]
    fn test_glyph_lookup() {
        let font = VariFont::new(DelugeFont::MetricBold9px);

        // Valid ASCII character
        assert!(font.glyph('A').is_some());
        assert!(font.glyph(' ').is_some());

        // Invalid character (outside range)
        assert!(font.glyph('\u{0000}').is_none());
    }
}
