//! Text rendering utilities

pub mod fonts;
mod vari_font;
mod vari_text_style;

pub use fonts::{
    FONT_5PX, FONT_APPLE, FONT_METRIC_BOLD_9PX, FONT_METRIC_BOLD_13PX, FONT_METRIC_BOLD_20PX,
};
pub use vari_font::VariFont;
pub use vari_text_style::{VariTextStyle, VariTextStyleBuilder};

use deluge_fonts::Font as DelugeFont;
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*, text::Alignment};

/// Available fonts for the Deluge display
#[derive(Debug, Clone, Copy)]
pub enum Font {
    /// Original 5px font
    Font5px,
    /// Apple II 7px font
    FontApple,
    /// Metric Bold 9px
    MetricBold9px,
    /// Metric Bold 13px
    MetricBold13px,
    /// Metric Bold 20px (may be too large for some use cases)
    MetricBold20px,
}

impl Font {
    /// Get the deluge-fonts Font
    pub fn deluge_font(&self) -> DelugeFont {
        match self {
            Font::Font5px => DelugeFont::Font5px,
            Font::FontApple => DelugeFont::FontApple,
            Font::MetricBold9px => DelugeFont::MetricBold9px,
            Font::MetricBold13px => DelugeFont::MetricBold13px,
            Font::MetricBold20px => DelugeFont::MetricBold20px,
        }
    }

    /// Get font height in pixels
    pub fn height(&self) -> u32 {
        self.deluge_font().height() as u32
    }
}

/// Text style configuration
#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub font: Font,
    pub alignment: Alignment,
    pub color: BinaryColor,
}

impl TextStyle {
    pub fn new(font: Font) -> Self {
        Self {
            font,
            alignment: Alignment::Left,
            color: BinaryColor::On,
        }
    }

    pub fn with_alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    pub fn with_color(mut self, color: BinaryColor) -> Self {
        self.color = color;
        self
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::new(Font::MetricBold13px)
    }
}

/// Draw text at specified position
pub fn draw_text<D>(
    display: &mut D,
    text: &str,
    position: Point,
    style: TextStyle,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let deluge_font = style.font.deluge_font();

    // Adjust position based on alignment using actual text width
    let text_width = deluge_font.text_width(text);
    let x = match style.alignment {
        Alignment::Left => position.x,
        Alignment::Center => position.x - text_width / 2,
        Alignment::Right => position.x - text_width,
    };

    // Draw text using deluge-fonts with custom color
    deluge_font.draw_text_colored_with_spacing(
        display,
        text,
        embedded_graphics::prelude::Point::new(x, position.y),
        style.color,
        1, // 1px spacing between glyphs
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_heights() {
        assert_eq!(Font::Font5px.height(), 5);
        assert_eq!(Font::FontApple.height(), 7);
        assert_eq!(Font::MetricBold9px.height(), 9);
        assert_eq!(Font::MetricBold13px.height(), 13);
        assert_eq!(Font::MetricBold20px.height(), 20);
    }

    #[test]
    fn test_text_style() {
        let style = TextStyle::new(Font::MetricBold13px)
            .with_alignment(Alignment::Center)
            .with_color(BinaryColor::Off);

        assert_eq!(style.alignment, Alignment::Center);
        assert_eq!(style.color, BinaryColor::Off);
    }
}
