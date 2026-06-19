//! Text styling for variable-width fonts.
//!
//! This module provides `VariTextStyle` - a counterpart to embedded-graphics' `MonoTextStyle`
//! that works with variable-width Deluge fonts.

use super::VariFont;
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    primitives::Rectangle,
    text::{
        Alignment, Baseline,
        renderer::{TextMetrics, TextRenderer},
    },
};

/// Text style configuration for variable-width fonts.
///
/// `VariTextStyle` is designed to work with `VariFont` and provides styling
/// options similar to embedded-graphics' `MonoTextStyle`, but adapted for
/// variable-width fonts.
///
/// # Example
///
/// ```no_run
/// use deluge_ui_toolkit::text::{VariFont, VariTextStyle};
/// use deluge_fonts::Font as DelugeFont;
/// use embedded_graphics::{pixelcolor::BinaryColor, text::Alignment};
///
/// let font = VariFont::new(DelugeFont::MetricBold13px);
/// let style = VariTextStyle::new(font)
///     .with_text_color(BinaryColor::On)
///     .with_background_color(BinaryColor::Off)
///     .with_alignment(Alignment::Center);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariTextStyle {
    /// The font to use
    pub font: VariFont,

    /// Text color
    pub text_color: Option<BinaryColor>,

    /// Background color (None for transparent)
    pub background_color: Option<BinaryColor>,

    /// Text alignment
    pub alignment: Alignment,
}

impl VariTextStyle {
    /// Create a new text style with default settings.
    ///
    /// Default settings:
    /// - Text color: `BinaryColor::On`
    /// - Background color: transparent (None)
    /// - Alignment: `Alignment::Left`
    pub const fn new(font: VariFont) -> Self {
        Self {
            font,
            text_color: Some(BinaryColor::On),
            background_color: None,
            alignment: Alignment::Left,
        }
    }

    /// Set the text color.
    pub const fn with_text_color(mut self, color: BinaryColor) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Set the background color.
    pub const fn with_background_color(mut self, color: BinaryColor) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Set transparent background.
    pub const fn with_transparent_background(mut self) -> Self {
        self.background_color = None;
        self
    }

    /// Set the text alignment.
    pub const fn with_alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Get the line height.
    ///
    /// Line height is the font height, which determines vertical spacing
    /// for multi-line text.
    pub const fn line_height(&self) -> u32 {
        self.font.height()
    }
}

/// Builder for creating `VariTextStyle` with a fluent API.
///
/// # Example
///
/// ```no_run
/// use deluge_ui_toolkit::text::{VariFont, VariTextStyleBuilder};
/// use deluge_fonts::Font as DelugeFont;
/// use embedded_graphics::{pixelcolor::BinaryColor, text::Alignment};
///
/// let font = VariFont::new(DelugeFont::MetricBold13px);
/// let style = VariTextStyleBuilder::new()
///     .font(font)
///     .text_color(BinaryColor::On)
///     .background_color(BinaryColor::Off)
///     .alignment(Alignment::Center)
///     .build();
/// ```
#[derive(Clone, Copy, Debug)]
pub struct VariTextStyleBuilder {
    font: Option<VariFont>,
    text_color: Option<BinaryColor>,
    background_color: Option<BinaryColor>,
    alignment: Alignment,
}

impl VariTextStyleBuilder {
    /// Create a new builder with default settings.
    pub const fn new() -> Self {
        Self {
            font: None,
            text_color: Some(BinaryColor::On),
            background_color: None,
            alignment: Alignment::Left,
        }
    }

    /// Set the font.
    pub const fn font(mut self, font: VariFont) -> Self {
        self.font = Some(font);
        self
    }

    /// Set the text color.
    pub const fn text_color(mut self, color: BinaryColor) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Set the background color.
    pub const fn background_color(mut self, color: BinaryColor) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Set transparent background.
    pub const fn transparent_background(mut self) -> Self {
        self.background_color = None;
        self
    }

    /// Set the text alignment.
    pub const fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Build the text style.
    ///
    /// # Panics
    ///
    /// Panics if no font has been set.
    pub const fn build(self) -> VariTextStyle {
        let font = match self.font {
            Some(f) => f,
            None => panic!("Font must be set before building VariTextStyle"),
        };

        VariTextStyle {
            font,
            text_color: self.text_color,
            background_color: self.background_color,
            alignment: self.alignment,
        }
    }
}

impl Default for VariTextStyleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Implement TextRenderer trait to make VariTextStyle compatible with embedded-graphics Text
impl TextRenderer for VariTextStyle {
    type Color = BinaryColor;

    fn draw_string<D>(
        &self,
        text: &str,
        position: Point,
        _baseline: Baseline,
        target: &mut D,
    ) -> Result<Point, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let color = self.text_color.unwrap_or(BinaryColor::On);
        let spacing = self.font.character_spacing as i32;

        // Use the font's draw method with our color
        let width = self
            .font
            .font()
            .draw_text_colored_with_spacing(target, text, position, color, spacing)?;

        // Return the next position for chained drawing
        Ok(position + Point::new(width, 0))
    }

    fn draw_whitespace<D>(
        &self,
        width: u32,
        position: Point,
        _baseline: Baseline,
        target: &mut D,
    ) -> Result<Point, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        // Draw background if specified
        if let Some(bg_color) = self.background_color {
            let rect = Rectangle::new(position, Size::new(width, self.font.height()));
            target.fill_solid(&rect, bg_color)?;
        }

        Ok(position + Point::new(width as i32, 0))
    }

    fn measure_string(&self, text: &str, position: Point, _baseline: Baseline) -> TextMetrics {
        let width = self.font.text_width(text) as u32;
        let height = self.font.height();

        TextMetrics {
            bounding_box: Rectangle::new(position, Size::new(width, height)),
            next_position: position + Point::new(width as i32, 0),
        }
    }

    fn line_height(&self) -> u32 {
        self.font.height()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deluge_fonts::Font as DelugeFont;

    #[test]
    fn test_style_creation() {
        let font = VariFont::new(DelugeFont::MetricBold9px);
        let style = VariTextStyle::new(font);

        assert_eq!(style.text_color, Some(BinaryColor::On));
        assert_eq!(style.background_color, None);
        assert_eq!(style.alignment, Alignment::Left);
    }

    #[test]
    fn test_builder() {
        let font = VariFont::new(DelugeFont::MetricBold9px);
        let style = VariTextStyleBuilder::new()
            .font(font)
            .text_color(BinaryColor::Off)
            .background_color(BinaryColor::On)
            .alignment(Alignment::Center)
            .build();

        assert_eq!(style.text_color, Some(BinaryColor::Off));
        assert_eq!(style.background_color, Some(BinaryColor::On));
        assert_eq!(style.alignment, Alignment::Center);
    }

    #[test]
    fn test_line_height() {
        let font = VariFont::new(DelugeFont::MetricBold13px);
        let style = VariTextStyle::new(font);
        assert_eq!(style.line_height(), 13);
    }
}
