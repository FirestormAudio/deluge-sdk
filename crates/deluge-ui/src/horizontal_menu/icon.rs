use embedded_graphics::{
    Drawable,
    geometry::Point,
    pixelcolor::BinaryColor,
    prelude::{DrawTarget, OriginDimensions, Primitive, Size},
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};

use crate::{
    horizontal_menu::COLUMN_WIDTH, icons::IconData, positionable::Positionable, primitives::Icon,
};

/// Renders an icon centered in a horizontal menu slot
///
/// This matches the Deluge firmware's icon rendering for menu items like Toggle,
/// which displays icons (e.g., switcherIconOn/Off) centered in the menu column.
///
/// # Example
/// ```no_run
/// use deluge_ui_toolkit::horizontal_menu::IconOnly;
/// use deluge_ui_toolkit::icons::CHECKED_BOX;
/// use embedded_graphics::{prelude::*, pixelcolor::BinaryColor};
/// use embedded_graphics_simulator::SimulatorDisplay;
///
/// let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new((128, 48).into());
///
/// // Draw icon
/// let icon = IconOnly::new(&CHECKED_BOX);
/// icon.draw(&mut display).ok();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct IconOnly {
    icon_data: &'static IconData,
    position: Point,
    selected: bool,
}

impl IconOnly {
    /// Create a new horizontal menu icon
    ///
    /// # Arguments
    /// * `icon_data` - The icon data to render
    pub const fn new(icon_data: &'static IconData) -> Self {
        Self {
            icon_data,
            position: Point::zero(),
            selected: false,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Calculate centered position for the icon within its slot
    fn icon_position(&self) -> Point {
        let icon_width = self.icon_data.width as i32;
        let icon_height = self.icon_data.height as i32;

        // Center horizontally in the slot
        let icon_x = self.position.x + (COLUMN_WIDTH - icon_width) / 2;

        // Center vertically in the available menu area
        let available_height = crate::DISPLAY_HEIGHT as i32 - self.position.y;
        let icon_y = self.position.y + (available_height - icon_height) / 2;

        Point::new(icon_x, icon_y)
    }
}

impl Drawable for IconOnly {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        if self.selected {
            // Draw filled rounded rectangle around icon (fixed size: 22x22)

            RoundedRectangle::new(
                Rectangle::new(
                    Point::new(self.position.x + 1, self.position.y + 1),
                    Size::new(self.size().width - 2, self.size().height - 2),
                ),
                CornerRadii::new(Size::new(2, 2)),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;

            // Draw icon inverted (Off color on filled background)
            Icon::new(self.icon_data, self.icon_position())
                .with_color(BinaryColor::Off)
                .draw(display)?;
        } else {
            // Draw icon normally
            Icon::new(self.icon_data, self.icon_position()).draw(display)?;
        }
        Ok(())
    }
}

impl Positionable for IconOnly {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
    }
}

impl OriginDimensions for IconOnly {
    fn size(&self) -> Size {
        Size::new(
            COLUMN_WIDTH as u32,
            (crate::DISPLAY_HEIGHT as i32 - self.position.y) as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::horizontal_menu::BASE_Y;
    use crate::icons::CHECKED_BOX;
    use embedded_graphics::mock_display::MockDisplay;

    #[test]
    fn test_icon_centered() {
        let mut icon = IconOnly::new(&CHECKED_BOX);
        icon.set_position(Point::new(0, BASE_Y));
        let position = icon.icon_position();

        // With slot_width=32 and icon_width=7, should be centered at (32-7)/2 = 12
        assert_eq!(position.x, 12);

        // Vertically centered: base_y=14, available_height=43-14=29, icon_height=7
        // icon_y = 14 + (29-7)/2 = 14 + 11 = 25
        assert_eq!(position.y, 25);
    }

    #[test]
    fn test_icon_draw() {
        let mut display = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);

        let mut icon = IconOnly::new(&CHECKED_BOX);
        icon.set_position(Point::new(0, BASE_Y));
        icon.draw(&mut display).unwrap();

        // Verify some pixels were drawn
        let affected = display.affected_area();
        assert!(affected.size.width > 0 && affected.size.height > 0);
    }

    #[test]
    fn test_icon_multiple_columns() {
        let mut display: MockDisplay<BinaryColor> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);

        // Test that icons can be drawn in all 4 columns
        for col in 0..4 {
            let mut icon = IconOnly::new(&CHECKED_BOX);
            icon.set_position(Point::new(col * 32, BASE_Y));
            icon.draw(&mut display).unwrap();
        }
    }
}
