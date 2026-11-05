//! Slider parameter control
//!
//! Provides a vertical indicator on a horizontal track showing a normalized value (0.0 to 1.0).

use crate::Positionable;
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Slider control for normalized parameter values
///
/// Displays a horizontal track with a vertical indicator showing the current position.
/// Value ranges from 0.0 (left) to 1.0 (right).
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::Slider;
/// use embedded_graphics::prelude::*;
///
/// let mut slider = Slider::new(Point::new(10, 20), 0.5);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Slider {
    point: Point,
    value: f32, // 0.0 to 1.0
}

impl Slider {
    const WIDTH: i32 = 23;
    const HEIGHT: i32 = 11;

    /// Create a new slider control
    ///
    /// # Arguments
    /// * `point` - Top-left position
    /// * `value` - Current value from 0.0 to 1.0
    pub fn new(point: Point, value: f32) -> Self {
        Self {
            point,
            value: value.clamp(0.0, 1.0),
        }
    }

    /// Update the value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Get the current value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for Slider {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let x = self.point.x;
        let y = self.point.y;

        // Draw horizontal track line (center)
        let track_y = y + Self::HEIGHT / 2;
        Line::new(
            Point::new(x, track_y),
            Point::new(x + Self::WIDTH - 1, track_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Calculate indicator position based on value
        let indicator_x = x + (self.value * (Self::WIDTH - 1) as f32) as i32;

        // Draw vertical indicator line
        Line::new(
            Point::new(indicator_x, y),
            Point::new(indicator_x, y + Self::HEIGHT - 1),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        Ok(())
    }
}

impl Positionable for Slider {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for Slider {
    fn size(&self) -> Size {
        Size::new(Self::WIDTH as u32, Self::HEIGHT as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_slider_draw() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let slider = Slider::new(Point::new(10, 20), 0.5);
        slider.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut slider = Slider::new(Point::new(10, 20), 0.5);
        slider.set_value(2.0);
        assert_eq!(slider.value(), 1.0);
        slider.set_value(-0.5);
        assert_eq!(slider.value(), 0.0);
    }
}
