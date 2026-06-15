use embedded_graphics::{
    Drawable,
    geometry::Point,
    pixelcolor::BinaryColor,
    prelude::{DrawTarget, OriginDimensions, Primitive, Size},
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
    text::Alignment,
};

use crate::{
    horizontal_menu::BOTTOM_MARGIN,
    params::{
        Attack, BipolarBar, HighPassFilter, LengthSlider, LowPassFilter, Pan, Percent, Release,
        SidechainDucking, Slider, UnipolarBar, UnipolarKnob,
    },
    positionable::Positionable,
    text::{Font, TextStyle, draw_text},
};

/// Enum representing different parameter visualization types
#[derive(Debug, Clone)]
pub enum ParamType {
    Attack(Attack),
    BipolarBar(BipolarBar),
    HighPassFilter(HighPassFilter),
    Knob(UnipolarKnob),
    LengthSlider(LengthSlider),
    LowPassFilter(LowPassFilter),
    Pan(Pan),
    Percent(Percent),
    Release(Release),
    SidechainDucking(SidechainDucking),
    Slider(Slider),
    UnipolarBar(UnipolarBar),
}

impl ParamType {
    /// Draw the parameter to the display
    fn draw<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        match self {
            ParamType::Attack(p) => p.draw(display),
            ParamType::BipolarBar(p) => p.draw(display),
            ParamType::HighPassFilter(p) => p.draw(display),
            ParamType::Knob(p) => p.draw(display),
            ParamType::LengthSlider(p) => p.draw(display),
            ParamType::LowPassFilter(p) => p.draw(display),
            ParamType::Pan(p) => p.draw(display),
            ParamType::Percent(p) => p.draw(display),
            ParamType::Release(p) => p.draw(display),
            ParamType::SidechainDucking(p) => p.draw(display),
            ParamType::Slider(p) => p.draw(display),
            ParamType::UnipolarBar(p) => p.draw(display),
        }
    }

    /// Get the fixed size of the parameter visualization
    fn size(&self) -> Option<Size> {
        match self {
            ParamType::Attack(p) => Some(p.size()),
            ParamType::BipolarBar(p) => Some(p.size()),
            ParamType::HighPassFilter(p) => Some(p.size()),
            ParamType::Knob(p) => Some(p.size()),
            ParamType::LengthSlider(p) => Some(p.size()),
            ParamType::LowPassFilter(p) => Some(p.size()),
            ParamType::Pan(p) => Some(p.size()),
            ParamType::Percent(p) => Some(p.size()),
            ParamType::Release(p) => Some(p.size()),
            ParamType::SidechainDucking(p) => Some(p.size()),
            ParamType::Slider(p) => Some(p.size()),
            ParamType::UnipolarBar(p) => Some(p.size()),
        }
    }

    /// Set the position of the parameter
    fn set_position(&mut self, point: Point) {
        match self {
            ParamType::Attack(p) => p.set_position(point),
            ParamType::BipolarBar(p) => p.set_position(point),
            ParamType::HighPassFilter(p) => p.set_position(point),
            ParamType::Knob(p) => p.set_position(point),
            ParamType::LengthSlider(p) => p.set_position(point),
            ParamType::LowPassFilter(p) => p.set_position(point),
            ParamType::Pan(p) => p.set_position(point),
            ParamType::Percent(p) => p.set_position(point),
            ParamType::Release(p) => p.set_position(point),
            ParamType::SidechainDucking(p) => p.set_position(point),
            ParamType::Slider(p) => p.set_position(point),
            ParamType::UnipolarBar(p) => p.set_position(point),
        }
    }
}

/// Renders a parameter visualization with a text label below it in a horizontal menu slot
///
/// This matches the Deluge firmware's pattern for parameters in horizontal menus,
/// where a visual representation (knob, slider, bar, etc.) is shown with a label below.
/// This is used extensively in Number::renderInHorizontalMenu with different rendering styles.
///
/// # Example
/// ```no_run
/// use deluge_ui_toolkit::horizontal_menu::{Value, ParamType};
/// use deluge_ui_toolkit::params::Pan;
/// use embedded_graphics::{prelude::*, pixelcolor::BinaryColor, geometry::Point};
/// use embedded_graphics_simulator::SimulatorDisplay;
///
/// let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new((128, 48).into());
///
/// // Create a pan control
/// let pan = Pan::new(Point::new(0, 0), -0.5);
///
/// // Wrap it with a label for horizontal menu display
/// let value = Value::new(ParamType::Pan(pan), "PAN");
/// value.draw(&mut display).ok();
/// ```
#[derive(Debug, Clone)]
pub struct Value {
    param: ParamType,
    label: &'static str,
    position: Point,
    slot_width: i32,
    font: Font,
    selected: bool,
}

impl Value {
    /// Create a new horizontal menu value with parameter visualization and label
    ///
    /// # Arguments
    /// * `param` - Parameter control (will be positioned when added to menu)
    /// * `label` - Text label to display below the parameter
    pub fn new(param: ParamType, label: &'static str) -> Self {
        Self {
            param,
            label,
            position: Point::zero(),
            slot_width: crate::horizontal_menu::COLUMN_WIDTH,
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
        self
    }

    /// Set whether this value is selected (shows selection indicator)
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Update internal param positioning based on slot position
    fn update_param_position(&mut self) {
        // Center param horizontally in slot
        let param_x = if let Some(param_size) = self.param.size() {
            self.position.x + (self.slot_width - param_size.width as i32) / 2
        } else {
            self.position.x
        };

        // Calculate vertical centering in the space between header and label
        let label_y = crate::DISPLAY_HEIGHT as i32 - BOTTOM_MARGIN;
        let available_height = label_y - self.position.y;
        let param_height = self.param.size().map(|s| s.height as i32).unwrap_or(20);
        let param_y = self.position.y + (available_height - param_height) / 2;

        self.param.set_position(Point::new(param_x, param_y));
    }

    /// Calculate the position where the label should be drawn
    fn label_position(&self) -> Point {
        let label_x = self.position.x + (self.slot_width / 2);
        let label_y = crate::DISPLAY_HEIGHT as i32 - BOTTOM_MARGIN;

        Point::new(label_x, label_y)
    }
}

impl Drawable for Value {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        // Draw the parameter visualization
        self.param.draw(display)?;

        // Draw the label centered below
        let label_pos = self.label_position();

        if self.selected {
            // Draw inverted label with filled rounded rectangle background
            let label_width = (self.label.len() * 6) as u32; // Approximate width
            let label_box_width = label_width + 4; // Add padding
            let label_box_x = label_pos.x - (label_box_width as i32 / 2);
            let label_height = self.font.height();

            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(label_box_x, label_pos.y - 1),
                    Size::new(label_box_width, label_height + 2),
                ),
                CornerRadii::new(Size::new(2, 2)),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;

            // Draw label text inverted (Off color on filled background)
            let text_style = TextStyle::new(self.font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::Off);

            draw_text(display, self.label, label_pos, text_style)?;
        } else {
            // Draw label normally
            let text_style = TextStyle::new(self.font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::On);

            draw_text(display, self.label, label_pos, text_style)?;
        }

        Ok(())
    }
}

impl Positionable for Value {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
        self.update_param_position();
    }
}

impl OriginDimensions for Value {
    fn size(&self) -> Size {
        Size::new(
            self.slot_width as u32,
            (crate::DISPLAY_HEIGHT as i32 - self.position.y) as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        horizontal_menu::{BASE_Y, Value},
        params::Pan,
    };
    use embedded_graphics::{mock_display::MockDisplay, pixelcolor::BinaryColor, prelude::Point};

    #[test]
    fn test_value_with_pan() {
        let mut display = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        display.set_allow_overdraw(true); // Pan may draw over same pixels

        let pan = Pan::new(Point::new(0, 0), 0.5);
        let mut value = Value::new(ParamType::Pan(pan), "PAN");
        value.set_position(Point::new(0, BASE_Y));
        value.draw(&mut display).unwrap();

        // Verify content was drawn
        let affected = display.affected_area();
        assert!(affected.size.width > 0 && affected.size.height > 0);
    }

    #[test]
    fn test_value_positions() {
        let pan = Pan::new(Point::new(0, 0), 0.0);
        let mut value = Value::new(ParamType::Pan(pan), "TEST");
        value.set_position(Point::new(32, BASE_Y)); // Position in column 1

        let label_pos = value.label_position();

        // Label should be centered in slot at x=32 with width=32
        assert_eq!(label_pos.x, 32 + 16); // slot start + half width

        // Label should be near bottom
        assert!(label_pos.y > 30);
    }

    #[test]
    fn test_value_multiple_columns() {
        let mut display: MockDisplay<BinaryColor> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        display.set_allow_overdraw(true); // Drawing multiple columns may overlap

        // Test that value can be drawn in all 4 columns
        for col in 0..4 {
            let pan = Pan::new(Point::new(0, 0), 0.5);
            let mut value = Value::new(ParamType::Pan(pan), "COL");
            value.set_position(Point::new(col * 32, BASE_Y));
            value.draw(&mut display).unwrap();
        }
    }
}
