//! Pan control parameter visualization
//!
//! Displays stereo pan position with half-cylinder icon and radial arc fill.

use crate::Positionable;
use crate::{icons::PAN_HALF_CYLINDER, primitives::Icon};
// Required for no_std ARM target where core_float_math is unavailable.
#[allow(unused_imports)]
use crate::prelude::F32Ext as _;
use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Pan control for stereo positioning
///
/// Shows pan position from left (-1.0) to right (1.0) using a half-cylinder icon
/// with a radial arc fill indicating the direction and amount.
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::Pan;
/// use embedded_graphics::prelude::*;
///
/// let mut pan = Pan::new(Point::new(10, 20), 0.5);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Pan {
    point: Point,
    value: f32, // -1.0 (full left) to 1.0 (full right)
}

impl Pan {
    const RADIUS: f32 = 11.0;

    /// Create a new pan control
    ///
    /// # Arguments
    /// * `point` - Top-left position
    /// * `value` - Pan value from -1.0 (left) to 1.0 (right)
    pub fn new(point: Point, value: f32) -> Self {
        Self {
            point,
            value: value.clamp(-1.0, 1.0),
        }
    }

    /// Update the value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(-1.0, 1.0);
    }

    /// Get the current value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for Pan {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let x = self.point.x;
        let y = self.point.y;

        // Draw the half-cylinder base icon
        let icon_width = PAN_HALF_CYLINDER.width as i32;
        Icon::new(&PAN_HALF_CYLINDER, Point::new(x, y - 1)).draw(display)?;

        // Determine direction
        let direction = if self.value > 0.0 {
            1.0
        } else if self.value < 0.0 {
            -1.0
        } else {
            return Ok(()); // Nothing to fill when value is zero
        };

        // Arc parameters
        const ARC_RANGE_ANGLE: f32 = 90.0;
        const BEGINNING_ANGLE: f32 = 270.0;
        const INNER_RADIUS: f32 = 5.0;
        const ANGLE_STEP: f32 = 1.0;
        let outer_radius = Self::RADIUS - 1.0;

        // Calculate target angle based on value
        let norm = self.value.abs();
        let target_angle = BEGINNING_ANGLE + ARC_RANGE_ANGLE * norm * direction;

        // Center point (center of icon width, RADIUS down from top)
        let center_x = x + icon_width / 2;
        let center_y = y + Self::RADIUS as i32;

        // Initial angle (270° = straight down)
        let step_rad = ANGLE_STEP * core::f32::consts::PI / 180.0;
        let cos_step = step_rad.cos();
        let sin_step = step_rad.sin();

        // Calculate initial cos/sin
        let mut cos_a = (BEGINNING_ANGLE * core::f32::consts::PI / 180.0).cos();
        let mut sin_a = (BEGINNING_ANGLE * core::f32::consts::PI / 180.0).sin();

        // Number of steps to reach target
        let steps = ((target_angle - BEGINNING_ANGLE).abs() / ANGLE_STEP).round() as i32;

        for _ in 0..steps {
            // Draw radial line from inner to outer radius
            let line_start_x = (center_x as f32 + INNER_RADIUS * cos_a).round() as i32;
            let line_start_y = (center_y as f32 + INNER_RADIUS * sin_a).round() as i32;
            let line_end_x = (center_x as f32 + outer_radius * cos_a).round() as i32;
            let line_end_y = (center_y as f32 + outer_radius * sin_a).round() as i32;

            Line::new(
                Point::new(line_start_x, line_start_y),
                Point::new(line_end_x, line_end_y),
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;

            // Advance angle by one step using rotation matrix
            let new_cos = cos_a * cos_step - sin_a * sin_step * direction;
            let new_sin = sin_a * cos_step + cos_a * sin_step * direction;
            cos_a = new_cos;
            sin_a = new_sin;
        }

        Ok(())
    }
}

impl Positionable for Pan {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for Pan {
    fn size(&self) -> Size {
        // Size is based on icon width and arc radius
        Size::new(PAN_HALF_CYLINDER.width as u32, (Self::RADIUS * 2.0) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_pan_center() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let pan = Pan::new(Point::new(10, 20), 0.0);
        pan.draw(&mut display).unwrap();
    }

    #[test]
    fn test_pan_right() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let pan = Pan::new(Point::new(10, 20), 1.0);
        pan.draw(&mut display).unwrap();
    }

    #[test]
    fn test_pan_left() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let pan = Pan::new(Point::new(10, 20), -1.0);
        pan.draw(&mut display).unwrap();
    }
}
