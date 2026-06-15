//! Length slider parameter control
//!
//! Provides a filled horizontal bar from left to a position indicator.

use crate::Positionable;
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Length slider control for length/duration parameters
///
/// Displays a filled bar from the left edge to the current position,
/// with an optional minimum position indicator.
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::LengthSlider;
/// use embedded_graphics::prelude::*;
///
/// let mut slider = LengthSlider::new(Point::new(10, 20), 0.7, false);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LengthSlider {
    point: Point,
    value: f32, // 0.0 to 1.0
    min_pos_active: bool,
}

impl LengthSlider {
    const WIDTH: i32 = 23;
    const HEIGHT: i32 = 11;

    /// Create a new length slider control
    ///
    /// # Arguments
    /// * `point` - Top-left position
    /// * `value` - Current value from 0.0 to 1.0
    /// * `min_pos_active` - Whether to show minimum position indicator
    pub fn new(point: Point, value: f32, min_pos_active: bool) -> Self {
        Self {
            point,
            value: value.clamp(0.0, 1.0),
            min_pos_active,
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

    /// Set minimum position active state
    pub fn set_min_pos_active(&mut self, active: bool) {
        self.min_pos_active = active;
    }
}

impl Drawable for LengthSlider {
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

        // Calculate end position
        let end_x = x + (self.value * (Self::WIDTH - 1) as f32) as i32;

        // Fill from start to end position
        for fill_x in x..=end_x {
            Line::new(
                Point::new(fill_x, y),
                Point::new(fill_x, y + Self::HEIGHT - 1),
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;
        }

        // Draw minimum position indicator if active
        if self.min_pos_active {
            let min_x = x + 2;
            Line::new(
                Point::new(min_x, y),
                Point::new(min_x, y + Self::HEIGHT - 1),
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::Off, 1))
            .draw(display)?;
        }

        Ok(())
    }
}

impl Positionable for LengthSlider {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for LengthSlider {
    fn size(&self) -> Size {
        Size::new(Self::WIDTH as u32, Self::HEIGHT as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_length_slider_draw() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let slider = LengthSlider::new(Point::new(10, 20), 0.7, false);
        slider.draw(&mut display).unwrap();
    }

    #[test]
    fn test_with_min_pos() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let slider = LengthSlider::new(Point::new(10, 20), 0.7, true);
        slider.draw(&mut display).unwrap();
    }
}
