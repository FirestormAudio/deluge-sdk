//! Horizontal bar parameter control
//!
//! Provides a bipolar horizontal bar indicator with rounded rectangle outline.
//! Used extensively throughout the Deluge for showing parameter values that can
//! be negative or positive. The bar fills from a center zero point toward the current value.

use crate::Positionable;
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Primitive, PrimitiveStyle, Rectangle, RoundedRectangle},
};

/// Horizontal bar control for bipolar parameter values
///
/// Displays a bar that fills from center toward the current value,
/// making it ideal for parameters that have both positive and negative ranges.
/// Value ranges from -1.0 (full left) to 1.0 (full right), with 0.0 at center.
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::BipolarBar;
/// use embedded_graphics::prelude::*;
/// use embedded_graphics_simulator::SimulatorDisplay;
/// use embedded_graphics::pixelcolor::BinaryColor;
///
/// let mut display = SimulatorDisplay::<BinaryColor>::new(Size::new(128, 64));
/// let mut bar = BipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.5);
/// bar.draw(&mut display).unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct BipolarBar {
    point: Point,
    size: Size,
    value: f32, // -1.0 to 1.0
}

impl BipolarBar {
    /// Create a new horizontal bar control
    ///
    /// # Arguments
    /// * `point` - Top-left position of the bar
    /// * `size` - Width and height of the bar
    /// * `value` - Current value from -1.0 (full left) to 1.0 (full right)
    pub fn new(point: Point, size: Size, value: f32) -> Self {
        Self {
            point,
            size,
            value: value.clamp(-1.0, 1.0),
        }
    }

    /// Update the value
    ///
    /// # Arguments
    /// * `value` - New value from -1.0 to 1.0
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(-1.0, 1.0);
    }

    /// Get the current value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for BipolarBar {
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

        // Calculate fill area
        let width = self.size.width as i32;
        let height = self.size.height as i32;
        let center_pos = width / 2;

        let (fill_x, fill_width) = if self.value >= 0.0 {
            // Positive value: fill from center to right
            let fill_w = (center_pos as f32 * self.value) as i32;
            (self.point.x + center_pos, fill_w.max(0))
        } else {
            // Negative value: fill from left to center
            let fill_w = (center_pos as f32 * self.value.abs()) as i32;
            (self.point.x + center_pos - fill_w, fill_w.max(0))
        };

        // Draw fill with inset (avoid overdraw on border)
        if fill_width > 2 {
            RoundedRectangle::with_equal_corners(
                Rectangle::new(
                    Point::new(fill_x + 1, self.point.y + 1),
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

impl Positionable for BipolarBar {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for BipolarBar {
    fn size(&self) -> Size {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_positive_value() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let bar = BipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.5);
        bar.draw(&mut display).unwrap();
    }

    #[test]
    fn test_negative_value() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let bar = BipolarBar::new(Point::new(10, 20), Size::new(100, 8), -0.75);
        bar.draw(&mut display).unwrap();
    }

    #[test]
    fn test_zero_value() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let bar = BipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.0);
        bar.draw(&mut display).unwrap();
    }

    #[test]
    fn test_set_value() {
        let mut bar = BipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.0);
        assert_eq!(bar.value(), 0.0);
        bar.set_value(0.5);
        assert_eq!(bar.value(), 0.5);
    }

    #[test]
    fn test_value_clamping() {
        let mut bar = BipolarBar::new(Point::new(10, 20), Size::new(100, 8), 0.0);
        bar.set_value(2.0); // Should clamp to 1.0
        assert_eq!(bar.value(), 1.0);
        bar.set_value(-2.0); // Should clamp to -1.0
        assert_eq!(bar.value(), -1.0);
    }
}
