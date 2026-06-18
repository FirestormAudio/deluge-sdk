//! Sidechain ducking visualization control
//!
//! Displays compression curve as a trapezoid shape for sidechain ducking visualization.

use crate::Positionable;
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Sidechain ducking compression curve visualization
///
/// Shows a trapezoid/triangle envelope shape representing the compression curve.
/// Positive values show downward compression, negative values show upward expansion.
/// Dimensions: 23×11 pixels (centered in provided width)
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::SidechainDucking;
/// use embedded_graphics::prelude::*;
///
/// let mut ducking = SidechainDucking::new(Point::new(10, 20), Size::new(30, 15), 0.5);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct SidechainDucking {
    point: Point,
    size: Size,
    value: f32, // -1.0 to 1.0 (ducking amount)
}

impl SidechainDucking {
    const SHAPE_WIDTH: i32 = 23;
    const SHAPE_HEIGHT: i32 = 11;
    const Y_OFFSET: i32 = 0; // Adjusted for -1 in original
    const OFFSET_RIGHT: i32 = 10;

    /// Create a new sidechain ducking visualization
    ///
    /// # Arguments
    /// * `point` - Top-left position
    /// * `size` - Width and height (shape is centered horizontally)
    /// * `value` - Ducking amount from -1.0 (expansion) to 1.0 (compression)
    pub fn new(point: Point, size: Size, value: f32) -> Self {
        Self {
            point,
            size,
            value: value.clamp(-1.0, 1.0),
        }
    }

    /// Update the ducking amount value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(-1.0, 1.0);
    }

    /// Get the current ducking amount value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for SidechainDucking {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let width = self.size.width as i32;
        let left_padding = (width - Self::SHAPE_WIDTH) / 2;
        let min_x = self.point.x + left_padding;
        let max_x = min_x + Self::SHAPE_WIDTH;
        let min_y = self.point.y + Self::Y_OFFSET;
        let max_y = min_y + Self::SHAPE_HEIGHT - 1;

        // Calculate shape height based on value
        let norm = self.value.abs();
        let fill_height = (norm * Self::SHAPE_HEIGHT as f32) as i32;
        let y_offset = (Self::SHAPE_HEIGHT - fill_height) / 2;

        let (y0, y1) = if self.value >= 0.0 {
            // For positive values, draw from top down (compression)
            let ducking_start_y = min_y + y_offset;
            let ducking_end_y = ducking_start_y + fill_height;
            (ducking_end_y, ducking_start_y)
        } else {
            // For negative values, draw from bottom up (expansion)
            let ducking_end_y = max_y - y_offset;
            let ducking_start_y = ducking_end_y - fill_height;
            (ducking_start_y, ducking_end_y)
        };

        // Draw sidechain level trapezoid shape

        // Diagonal line
        Line::new(
            Point::new(min_x, y0),
            Point::new(max_x - Self::OFFSET_RIGHT, y1),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Vertical line on left
        Line::new(Point::new(min_x, y0), Point::new(min_x, y1))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;

        // Horizontal line on top/bottom
        Line::new(
            Point::new(max_x - Self::OFFSET_RIGHT, y1),
            Point::new(max_x, y1),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        Ok(())
    }
}

impl Positionable for SidechainDucking {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for SidechainDucking {
    fn size(&self) -> Size {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_positive_ducking() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let ducking = SidechainDucking::new(Point::new(10, 20), Size::new(30, 15), 0.5);
        ducking.draw(&mut display).unwrap();
    }

    #[test]
    fn test_negative_ducking() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let ducking = SidechainDucking::new(Point::new(10, 20), Size::new(30, 15), -0.5);
        ducking.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut ducking = SidechainDucking::new(Point::new(10, 20), Size::new(30, 15), 0.5);
        ducking.set_value(2.0);
        assert_eq!(ducking.value(), 1.0);
        ducking.set_value(-2.0);
        assert_eq!(ducking.value(), -1.0);
    }
}
