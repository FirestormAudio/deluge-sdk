//! Percent display parameter control
//!
//! Displays a large percentage value using the authentic Deluge font.

use crate::Positionable;
use crate::prelude::*;
use deluge_fonts::Font;
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
};

/// Percent display control
///
/// Shows a percentage value (0-100%) using the Deluge's FontApple.
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::Percent;
/// use embedded_graphics::prelude::*;
///
/// let mut percent = Percent::new(Point::new(10, 20), 75.5);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Percent {
    point: Point,
    value: f32, // 0.0 to 100.0
}

impl Percent {
    /// Create a new percent display
    ///
    /// # Arguments
    /// * `point` - Top-left position for the text
    /// * `value` - Percentage value from 0.0 to 100.0
    pub fn new(point: Point, value: f32) -> Self {
        Self {
            point,
            value: value.clamp(0.0, 100.0),
        }
    }

    /// Update the value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 100.0);
    }

    /// Get the current value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for Percent {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let text = format!("{}%", self.value as i32);
        Font::FontApple.draw_text(display, &text, self.point)?;
        Ok(())
    }
}

impl Positionable for Percent {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for Percent {
    fn size(&self) -> Size {
        // Estimated size for "100%" text with FontApple
        Size::new(30, 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_percent_draw() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let percent = Percent::new(Point::new(10, 20), 75.0);
        percent.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut percent = Percent::new(Point::new(10, 20), 50.0);
        percent.set_value(150.0);
        assert_eq!(percent.value(), 100.0);
        percent.set_value(-10.0);
        assert_eq!(percent.value(), 0.0);
    }
}
