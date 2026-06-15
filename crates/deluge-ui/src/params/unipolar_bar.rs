//! Unipolar horizontal bar parameter control
//!
//! Provides a horizontal bar that fills from left to right (0.0 to 1.0).
//! Unlike the bipolar HorizontalBar, this doesn't have a center point.

use crate::Positionable;
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Primitive, PrimitiveStyle, Rectangle, RoundedRectangle},
};

/// Unipolar horizontal bar control
///
/// Displays a bar that fills from left to right based on the value.
/// Ideal for parameters that range from 0 to 100% (volume, brightness, etc).
/// Value ranges from 0.0 (empty) to 1.0 (full).
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::UnipolarBar;
/// use embedded_graphics::prelude::*;
/// use embedded_graphics_simulator::SimulatorDisplay;
/// use embedded_graphics::pixelcolor::BinaryColor;
///
/// let mut display = SimulatorDisplay::<BinaryColor>::new(Size::new(128, 64));
/// let mut bar = UnipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.75);
/// bar.draw(&mut display).unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct UnipolarBar {
    point: Point,
    size: Size,
    value: f32, // 0.0 to 1.0
}

impl UnipolarBar {
    /// Create a new unipolar horizontal bar control
    ///
    /// # Arguments
    /// * `point` - Top-left position of the bar
    /// * `size` - Width and height of the bar
    /// * `value` - Current value from 0.0 (empty) to 1.0 (full)
    pub fn new(point: Point, size: Size, value: f32) -> Self {
        Self {
            point,
            size,
            value: value.clamp(0.0, 1.0),
        }
    }

    /// Update the value
    ///
    /// # Arguments
    /// * `value` - New value from 0.0 to 1.0
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Get the current value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for UnipolarBar {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        // Draw outer rounded rectangle border
        RoundedRectangle::with_equal_corners(
            Rectangle::new(self.point, self.size),
            Size::new(2, 2),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Calculate fill area from left to right
        let width = self.size.width as i32;
        let height = self.size.height as i32;
        let fill_width = (width as f32 * self.value) as i32;

        // Draw fill with inset (avoid overdraw on border)
        if fill_width > 2 {
            RoundedRectangle::with_equal_corners(
                Rectangle::new(
                    Point::new(self.point.x + 1, self.point.y + 1),
                    Size::new((fill_width - 1).max(0) as u32, (height - 2).max(0) as u32),
                ),
                Size::new(1, 1),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }

        Ok(())
    }
}

impl Positionable for UnipolarBar {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for UnipolarBar {
    fn size(&self) -> Size {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_empty_bar() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let bar = UnipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.0);
        bar.draw(&mut display).unwrap();
    }

    #[test]
    fn test_half_full_bar() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let bar = UnipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.5);
        bar.draw(&mut display).unwrap();
    }

    #[test]
    fn test_full_bar() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let bar = UnipolarBar::new(Point::new(10, 20), Size::new(100, 8), 1.0);
        bar.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut bar = UnipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.5);
        bar.set_value(2.0); // Should clamp to 1.0
        assert_eq!(bar.value(), 1.0);
        bar.set_value(-0.5); // Should clamp to 0.0
        assert_eq!(bar.value(), 0.0);
    }

    #[test]
    fn test_positionable_trait() {
        use crate::Positionable;

        let mut bar = UnipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.5);

        // Test position
        assert_eq!(bar.position(), Point::new(10, 20));
        bar.set_position(Point::new(20, 30));
        assert_eq!(bar.position(), Point::new(20, 30));
    }
}
