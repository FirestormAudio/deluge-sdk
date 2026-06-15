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
    horizontal_menu::COLUMN_WIDTH,
    icons::IconData,
    positionable::Positionable,
    primitives::Icon,
    text::{Font, TextStyle, draw_text},
};

/// Renders an icon with a text label below it in a horizontal menu slot
///
/// This matches the Deluge firmware's pattern for menu items that display
/// an icon above a label. The component calculates its own layout with
/// proper positioning and spacing between icon and label.
///
/// # Example
/// ```no_run
/// use deluge_ui_toolkit::horizontal_menu::IconWithLabel;
/// use deluge_ui_toolkit::icons::SINE;
/// use embedded_graphics::{prelude::*, pixelcolor::BinaryColor};
/// use embedded_graphics_simulator::SimulatorDisplay;
///
/// let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new((128, 48).into());
///
/// // Draw icon with "SHAPE" label
/// let icon_label = IconWithLabel::new(&SINE, "SHAPE");
/// icon_label.draw(&mut display).ok();
/// ```
#[derive(Debug, Clone)]
pub struct IconWithLabel {
    icon_data: &'static IconData,
    label: String,
    position: Point,
    slot_width: i32,
    label_height: u32,
    spacing: u32,
    font: Font,
    selected: bool,
}

impl IconWithLabel {
    /// Create a new horizontal menu icon with label
    ///
    /// # Arguments
    /// * `icon_data` - The icon data to render
    /// * `label` - Text label to display below the icon
    pub fn new(icon_data: &'static IconData, label: impl Into<String>) -> Self {
        Self {
            icon_data,
            label: label.into(),
            position: Point::zero(),
            slot_width: COLUMN_WIDTH,
            label_height: 9, // Font height for FontApple
            spacing: 2,      // Spacing between icon and label
            font: Font::FontApple,
            selected: false,
        }
    }

    pub fn with_slot_width(mut self, slot_width: i32) -> Self {
        self.slot_width = slot_width;
        self
    }

    pub fn with_font(mut self, font: Font) -> Self {
        self.font = font;
        self.label_height = font.height();
        self
    }

    pub fn with_spacing(mut self, spacing: u32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Calculate the position where the icon should be drawn
    fn icon_position(&self) -> Point {
        const BOX_HEIGHT: i32 = 25;

        let icon_width = self.icon_data.width as i32;
        let icon_height = self.icon_data.height as i32;

        // Center icon horizontally in the slot
        let icon_x = self.position.x + (self.slot_width - icon_width) / 2;

        // Calculate label Y position: position_y + box_height - label_height
        let label_y = self.position.y + BOX_HEIGHT - self.label_height as i32;

        // Position icon above label with spacing
        let icon_y = label_y - self.spacing as i32 - icon_height;

        Point::new(icon_x, icon_y)
    }

    /// Calculate the position where the label should be drawn
    fn label_position(&self) -> Point {
        const BOX_HEIGHT: i32 = 25;

        let label_x = self.position.x + (self.slot_width / 2); // Center for text alignment
        let label_y = self.position.y + BOX_HEIGHT - self.label_height as i32;

        Point::new(label_x, label_y)
    }
}

impl Drawable for IconWithLabel {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        if self.selected {
            // Draw icon normally
            Icon::new(self.icon_data, self.icon_position()).draw(display)?;

            // Draw inverted label with filled rounded rectangle background
            let label_pos = self.label_position();
            let label_width = (self.label.len() * 6) as u32; // Approximate width
            let label_box_width = label_width + 4; // Add padding
            let label_box_x = label_pos.x - (label_box_width as i32 / 2);

            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(label_box_x, label_pos.y - 1),
                    Size::new(label_box_width, self.label_height + 2),
                ),
                CornerRadii::new(Size::new(2, 2)),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;

            // Draw label text inverted (Off color on filled background)
            let text_style = TextStyle::new(self.font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::Off);

            draw_text(display, &self.label, label_pos, text_style)?;
        } else {
            // Draw the icon at calculated position
            Icon::new(self.icon_data, self.icon_position()).draw(display)?;

            // Draw the label centered below the icon
            let text_style = TextStyle::new(self.font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::On);

            draw_text(display, &self.label, self.label_position(), text_style)?;
        }

        Ok(())
    }
}

impl Positionable for IconWithLabel {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
    }
}

impl OriginDimensions for IconWithLabel {
    fn size(&self) -> Size {
        Size::new(
            self.slot_width as u32,
            34, // Standard menu slot height
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{horizontal_menu::BASE_Y, icons::SINE};
    use embedded_graphics::mock_display::MockDisplay;

    #[test]
    fn test_icon_with_label_positions() {
        let mut icon_label = IconWithLabel::new(&SINE, "SHAPE");
        icon_label.set_position(Point::new(0, BASE_Y));
        let icon_pos = icon_label.icon_position();
        let label_pos = icon_label.label_position();

        // Verify icon is centered horizontally
        assert!(icon_pos.x >= 0);

        // Verify label is below icon
        assert!(label_pos.y > icon_pos.y);
    }

    #[test]
    fn test_icon_with_label_draw() {
        let mut display = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);

        let mut icon_label = IconWithLabel::new(&SINE, "WAVE");
        icon_label.set_position(Point::new(0, BASE_Y));
        icon_label.draw(&mut display).unwrap();

        // Verify content was drawn
        let affected = display.affected_area();
        assert!(affected.size.width > 0 && affected.size.height > 0);
    }

    #[test]
    fn test_icon_with_label_multiple_columns() {
        let mut display: MockDisplay<BinaryColor> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);

        // Test that icon+label can be drawn in all 4 columns
        let labels = ["COL0", "COL1", "COL2", "COL3"];
        for (col, label) in labels.iter().enumerate() {
            let mut icon_label = IconWithLabel::new(&SINE, *label);
            icon_label.set_position(Point::new((col as i32) * 32, BASE_Y));
            icon_label.draw(&mut display).unwrap();
        }
    }

    #[test]
    fn test_custom_font() {
        let icon_label = IconWithLabel::new(&SINE, "TEST").with_font(Font::MetricBold9px);

        // Verify font height was updated
        assert_eq!(icon_label.label_height, Font::MetricBold9px.height());
    }
}
