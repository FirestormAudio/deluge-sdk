//! Integer value editor
//!
//! Provides an editor for integer values with min/max bounds and optional suffix.

use crate::prelude::*;
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*, text::Text};

use crate::VariTextStyle;

/// Editor for integer values
///
/// Displays the value with optional suffix (e.g., "440 Hz", "120 BPM")
#[derive(Clone, Debug)]
pub struct BasicEditor {
    value: String,
}

impl BasicEditor {
    /// Create a new integer editor
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }
}

impl Drawable for BasicEditor {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        use crate::DISPLAY_WIDTH;
        use crate::text::fonts::FONT_METRIC_BOLD_20PX;
        use embedded_graphics::text::Baseline;
        use embedded_graphics::text::renderer::TextRenderer;

        let text = &self.value;
        let style = VariTextStyle::new(FONT_METRIC_BOLD_20PX);

        // Measure text width to center it horizontally
        let text_width = style
            .measure_string(text, Point::zero(), Baseline::Top)
            .bounding_box
            .size
            .width;
        let x = (DISPLAY_WIDTH as i32 - text_width as i32) / 2;

        // Center vertically in the space below the header
        // Header is ~10px, display is 43px, font is 20px
        // Available space: 43 - 10 = 33px
        // Center: 10 + (33 - 20) / 2 = 16px
        let y = 16;

        Text::new(text, Point::new(x, y), style).draw(display)?;

        Ok(())
    }
}
