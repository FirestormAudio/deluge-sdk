use crate::prelude::*;
use embedded_graphics::{
    Drawable,
    geometry::Point,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
    text::Alignment,
};

use crate::{
    horizontal_menu::{BOTTOM_MARGIN, COLUMN_WIDTH},
    positionable::Positionable,
    text::{Font, TextStyle, draw_text},
};

/// Renders text value with a label below it in a horizontal menu slot
///
/// This matches the Deluge firmware's SyncLevel::renderInHorizontalMenu pattern,
/// where a text value (like "OFF" or "1/4") is displayed with a label below.
/// The text is centered in the column, and the label appears at the bottom.
///
/// # Example
/// ```no_run
/// use deluge_ui_toolkit::horizontal_menu::TextWithLabel;
/// use embedded_graphics::{prelude::*, pixelcolor::BinaryColor};
/// use embedded_graphics_simulator::SimulatorDisplay;
///
/// let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new((128, 48).into());
///
/// // Draw "OFF" with "SYNC" label
/// let text_label = TextWithLabel::new("OFF".to_string(), "SYNC".to_string());
/// text_label.draw(&mut display).ok();
/// ```
#[derive(Debug, Clone)]
pub struct TextWithLabel {
    text: String,
    label: String,
    position: Point,
    slot_width: i32,
    text_font: Font,
    label_font: Font,
    selected: bool,
}

impl TextWithLabel {
    /// Create a new horizontal menu text with label
    ///
    /// # Arguments
    /// * `text` - Text value to display (e.g., "OFF", "1/4", "50")
    /// * `label` - Label to display below the text (e.g., "SYNC", "RATE")
    pub fn new(text: String, label: String) -> Self {
        Self {
            text,
            label,
            position: Point::zero(),
            slot_width: COLUMN_WIDTH,
            text_font: Font::FontApple,
            label_font: Font::FontApple,
            selected: false,
        }
    }

    pub fn with_slot_width(mut self, slot_width: i32) -> Self {
        self.slot_width = slot_width;
        self
    }

    pub fn with_text_font(mut self, font: Font) -> Self {
        self.text_font = font;
        self
    }

    pub fn with_label_font(mut self, font: Font) -> Self {
        self.label_font = font;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Calculate the centered X position within the slot
    fn center_x(&self) -> i32 {
        self.position.x + (self.slot_width / 2)
    }

    /// Calculate the Y position for the text value
    fn text_y(&self) -> i32 {
        // Position text in the middle of the available space
        // Matches SyncLevel which draws at startY + kHorizontalMenuSlotYOffset
        self.position.y + 6
    }

    /// Calculate the Y position for the label
    fn label_y(&self) -> i32 {
        // Label at bottom, matching the layout pattern
        crate::DISPLAY_HEIGHT as i32 - BOTTOM_MARGIN
    }
}

impl Drawable for TextWithLabel {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let center_x = self.center_x();

        if self.selected {
            // Draw the main text value normally
            let text_style = TextStyle::new(self.text_font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::On);

            draw_text(
                display,
                self.text.as_str(),
                Point::new(center_x, self.text_y()),
                text_style,
            )?;

            // Draw inverted label with filled rounded rectangle background
            let label_y = self.label_y();
            let label_width = (self.label.len() * 6) as u32; // Approximate width
            let label_box_width = label_width + 4; // Add padding
            let label_box_x = center_x - (label_box_width as i32 / 2);
            let label_height = self.label_font.height();

            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(label_box_x, label_y - 1),
                    Size::new(label_box_width, label_height + 2),
                ),
                CornerRadii::new(Size::new(2, 2)),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;

            // Draw label text inverted (Off color on filled background)
            let label_style = TextStyle::new(self.label_font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::Off);

            draw_text(
                display,
                self.label.as_str(),
                Point::new(center_x, label_y),
                label_style,
            )?;
        } else {
            // Draw the main text value centered
            let text_style = TextStyle::new(self.text_font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::On);

            draw_text(
                display,
                self.text.as_str(),
                Point::new(center_x, self.text_y()),
                text_style,
            )?;

            // Draw the label below, also centered
            let label_style = TextStyle::new(self.label_font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::On);

            draw_text(
                display,
                self.label.as_str(),
                Point::new(center_x, self.label_y()),
                label_style,
            )?;
        }

        Ok(())
    }
}

impl Positionable for TextWithLabel {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
    }
}

impl OriginDimensions for TextWithLabel {
    fn size(&self) -> Size {
        Size::new(
            self.slot_width as u32,
            (self.label_y() + self.label_font.height() as i32 - self.position.y) as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::horizontal_menu::BASE_Y;
    use alloc::string::ToString;
    use embedded_graphics::mock_display::MockDisplay;

    #[test]
    fn test_text_with_label_draw() {
        let mut display = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);

        let mut text_label = TextWithLabel::new("OFF".to_string(), "SYNC".to_string());
        text_label.set_position(Point::new(0, BASE_Y));
        text_label.draw(&mut display).unwrap();

        // Verify content was drawn
        let affected = display.affected_area();
        assert!(affected.size.width > 0 && affected.size.height > 0);
    }

    #[test]
    fn test_text_with_label_positions() {
        let mut text_label = TextWithLabel::new("1/4".to_string(), "RATE".to_string());
        text_label.set_position(Point::new(64, BASE_Y)); // Position in column 2

        let center_x = text_label.center_x();
        let text_y = text_label.text_y();
        let label_y = text_label.label_y();

        // Should be centered in slot at x=64 with width=32
        assert_eq!(center_x, 64 + 16); // slot start (64) + half width (16)

        // Text should be above label
        assert!(text_y < label_y);

        // Label should be near bottom
        assert!(label_y > 30);
    }

    #[test]
    fn test_text_with_label_multiple_columns() {
        let mut display: MockDisplay<BinaryColor> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);

        // Test that text+label can be drawn in all 4 columns
        let values = [("0", "A"), ("50", "B"), ("100", "C"), ("OFF", "D")];
        for (col, (text, label)) in values.iter().enumerate() {
            let mut text_label = TextWithLabel::new(text.to_string(), label.to_string());
            text_label.set_position(Point::new((col as i32) * 32, BASE_Y));
            text_label.draw(&mut display).unwrap();
        }
    }

    #[test]
    fn test_custom_fonts() {
        let text_label = TextWithLabel::new("TEST".to_string(), "LABEL".to_string())
            .with_text_font(Font::MetricBold9px)
            .with_label_font(Font::Font5px);

        assert_eq!(text_label.text_font.height(), Font::MetricBold9px.height());
        assert_eq!(text_label.label_font.height(), Font::Font5px.height());
    }
}
