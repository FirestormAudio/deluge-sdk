//! Float value editor
//!
//! Provides an editor for floating-point values with min/max bounds and precision control.

use crate::prelude::*;
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
    text::Text,
};

use crate::VariTextStyle;

/// Editor for floating-point values
///
/// Displays the value with configurable precision and optional suffix
#[derive(Clone, Debug)]
pub struct FloatEditor {
    value: f32,
    precision: usize,
    suffix: Option<String>,
    selected_digit: usize,
}

impl FloatEditor {
    /// Create a new float editor
    pub fn new(value: f32) -> Self {
        Self {
            value,
            precision: 2,
            suffix: None,
            selected_digit: 0,
        }
    }

    /// Set the decimal precision for display
    pub fn with_precision(mut self, precision: usize) -> Self {
        self.precision = precision;
        self
    }

    /// Set the suffix to display after the value
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// Get the formatted display string
    pub fn display_string(&self) -> String {
        let formatted = format!("{:.prec$}", self.value, prec = self.precision);

        if let Some(ref suffix) = self.suffix {
            format!("{} {}", formatted, suffix)
        } else {
            formatted
        }
    }

    pub fn with_selected_digit(mut self, digit: usize) -> Self {
        self.selected_digit = digit;
        self
    }

    /// Get the total number of displayed digits (excluding decimal point and suffix)
    pub fn digits(&self) -> usize {
        self.display_string()
            .chars()
            .filter(|ch| ch.is_ascii_digit())
            .count()
    }

    /// Get the character index for a given digit position, counting from the right
    /// digit_index 0 = rightmost digit, 1 = second from right, etc.
    fn get_char_index_for_digit(&self, digit_index: usize) -> Option<usize> {
        let text = self.display_string();

        // Collect all digit positions
        let digit_positions: Vec<usize> = text
            .chars()
            .enumerate()
            .filter(|(_, ch)| ch.is_ascii_digit())
            .map(|(idx, _)| idx)
            .collect();

        // Count from the right (reverse the index)
        if digit_index < digit_positions.len() {
            Some(digit_positions[digit_positions.len() - 1 - digit_index])
        } else {
            None
        }
    }
}

impl Drawable for FloatEditor {
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

        let text = self.display_string();
        let style = VariTextStyle::new(FONT_METRIC_BOLD_20PX);

        // Measure text width to center it horizontally
        let text_width = style
            .measure_string(&text, Point::zero(), Baseline::Top)
            .bounding_box
            .size
            .width;
        let x = (DISPLAY_WIDTH as i32 - text_width as i32) / 2;

        // Center vertically in the space below the header
        // Header is ~10px, display is 43px, font is 20px
        // Available space: 43 - 10 = 33px
        // Center: 10 + (33 - 20) / 2 = 16px
        let y = 16;

        Text::new(&text, Point::new(x, y), style).draw(display)?;

        // draw a cursor under the selected digit (automatically skips decimal point)
        if let Some(char_index) = self.get_char_index_for_digit(self.selected_digit) {
            let before_cursor = &text[..char_index];
            let cursor_x_offset = style
                .measure_string(before_cursor, Point::zero(), Baseline::Top)
                .bounding_box
                .size
                .width as i32;

            // Measure the width of the selected character
            let selected_char = &text[char_index..char_index + 1];
            let char_width = style
                .measure_string(selected_char, Point::zero(), Baseline::Top)
                .bounding_box
                .size
                .width as i32;

            let cursor_x = x + cursor_x_offset;
            let cursor_y = y + 22; // position below the text

            // Draw underline matching the character width
            let cursor_start = Point::new(cursor_x, cursor_y);
            let cursor_end = Point::new(cursor_x + char_width, cursor_y);

            Line::new(cursor_start, cursor_end)
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                .draw(display)?;
        }

        Ok(())
    }
}
