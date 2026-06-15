use embedded_graphics::{
    Drawable, Pixel,
    geometry::Point,
    pixelcolor::BinaryColor,
    prelude::{DrawTarget, OriginDimensions, Size},
};

use crate::{DISPLAY_HEIGHT, Positionable};

/// A placeholder for an empty horizontal menu slot
///
/// This draws a dotted rectangle outline to indicate an unused slot in the menu.
/// Matches the Deluge firmware's placeholder rendering.
///
/// # Example
/// ```no_run
/// use deluge_ui_toolkit::horizontal_menu::Placeholder;
/// use embedded_graphics::{prelude::*, pixelcolor::BinaryColor};
/// use embedded_graphics_simulator::SimulatorDisplay;
///
/// let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new((128, 48).into());
///
/// // Draw placeholder
/// let placeholder = Placeholder::new();
/// placeholder.draw(&mut display).ok();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placeholder {
    point: Point,
}

impl Default for Placeholder {
    fn default() -> Self {
        Self::new()
    }
}

impl Placeholder {
    /// Create a new horizontal menu placeholder
    ///
    /// # Arguments
    /// * `column_index` - Which column (0-3) to draw the placeholder in
    /// * `column_width` - Width of each column (typically 32 pixels)
    /// * `base_y` - Starting Y position (typically 14)
    pub const fn new() -> Self {
        Self {
            point: Point::new(0, 0),
        }
    }

    fn bounds(&self) -> (i32, i32, i32, i32) {
        let start_x = self.point.x + 7;
        let end_x = start_x + 17;
        let start_y = self.point.y + 1;
        let end_y = DISPLAY_HEIGHT as i32 - 7;

        (start_x, end_x, start_y, end_y)
    }
}

impl Drawable for Placeholder {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        const DOT_INTERVAL: i32 = 5;

        let (start_x, end_x, start_y, end_y) = self.bounds();

        // Draw horizontal dotted lines (top and bottom)
        for x in ((start_x + 1)..end_x).step_by(DOT_INTERVAL as usize) {
            Pixel(Point::new(x, start_y), BinaryColor::On).draw(display)?;
            Pixel(Point::new(x, end_y), BinaryColor::On).draw(display)?;
        }

        // Draw vertical dotted lines (left and right)
        for y in ((start_y + 3)..end_y).step_by(DOT_INTERVAL as usize) {
            Pixel(Point::new(start_x - 2, y), BinaryColor::On).draw(display)?;
            Pixel(Point::new(end_x + 2, y), BinaryColor::On).draw(display)?;
        }

        Ok(())
    }
}

impl Positionable for Placeholder {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for Placeholder {
    fn size(&self) -> Size {
        let (start_x, end_x, start_y, end_y) = self.bounds();
        Size::new((end_x - start_x + 5) as u32, (end_y - start_y) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::mock_display::MockDisplay;

    #[test]
    fn test_draw_placeholder() {
        let mut display = MockDisplay::new();

        // Draw placeholder in column 0
        let placeholder = Placeholder::new();
        placeholder.draw(&mut display).unwrap();

        // Verify some pixels were drawn
        let affected = display.affected_area();
        assert!(affected.size.width > 0 && affected.size.height > 0);
    }

    #[test]
    fn test_placeholder_dimensions() {
        let placeholder = Placeholder::new();
        let (start_x, end_x, start_y, end_y) = placeholder.bounds();

        // Verify the placeholder dimensions
        // start_x = point.x + 7 = 0 + 7 = 7
        assert_eq!(start_x, 7);
        // end_x = start_x + 17 = 7 + 17 = 24
        assert_eq!(end_x, 24);

        // Y positions: start_y = point.y + 1 = 0 + 1 = 1
        assert_eq!(start_y, 1);
        // end_y = DISPLAY_HEIGHT - 7 = 43 - 7 = 36
        assert_eq!(end_y, 36);
    }

    #[test]
    fn test_placeholder_size() {
        let placeholder = Placeholder::new();
        let size = placeholder.size();

        // Width: (end_x - start_x + 5) = (24 - 7 + 5) = 22
        assert_eq!(size.width, 22);

        // Height: end_y - start_y = 36 - 1 = 35
        assert_eq!(size.height, 35);
    }
}
