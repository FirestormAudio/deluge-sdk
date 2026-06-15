//! Attack stage visualization control
//!
//! Displays envelope attack stage with upward diagonal line and endpoint marker.

use crate::Positionable;
use embedded_graphics::{
    Drawable, Pixel,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Attack stage indicator for envelope visualization
///
/// Shows an upward diagonal line from bottom to peak with a square endpoint marker.
/// The endpoint position indicates the attack time.
/// Dimensions: 19×11 pixels (starts at x+7 from point)
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::Attack;
/// use embedded_graphics::prelude::*;
///
/// let mut attack = Attack::new(Point::new(10, 20), 0.4);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Attack {
    point: Point,
    value: f32, // 0.0 to 1.0 (attack time)
}

impl Attack {
    const WIDTH: i32 = 19;
    const HEIGHT: i32 = 11;
    const X_OFFSET: i32 = 7; // Drawing starts at x+7
    const Y_OFFSET: i32 = 1;
    const INDICATOR_SIZE: i32 = 2; // Half-size of 5×5 square

    /// Create a new attack stage visualization
    ///
    /// # Arguments
    /// * `point` - Top-left position (actual drawing starts at x+7)
    /// * `value` - Attack time from 0.0 (fast) to 1.0 (slow)
    pub fn new(point: Point, value: f32) -> Self {
        Self {
            point,
            value: value.clamp(0.0, 1.0),
        }
    }

    /// Update the attack time value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Get the current attack time value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for Attack {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let atk_start_x = self.point.x + Self::X_OFFSET;
        let atk_end_x = atk_start_x + Self::WIDTH - 1;
        let atk_start_y = self.point.y + Self::Y_OFFSET;
        let atk_end_y = atk_start_y + Self::HEIGHT - 1;

        // Linear interpolation for attack endpoint
        let atk_effective_x =
            atk_start_x + (self.value * (atk_end_x - 2 - atk_start_x) as f32) as i32;

        // Draw attack line (bottom to top)
        Line::new(
            Point::new(atk_start_x, atk_end_y),
            Point::new(atk_effective_x, atk_start_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw square endpoint indicator (5×5 filled square)
        for fill_x in
            (atk_effective_x - Self::INDICATOR_SIZE)..=(atk_effective_x + Self::INDICATOR_SIZE)
        {
            for fill_y in
                (atk_start_y - Self::INDICATOR_SIZE + 1)..=(atk_start_y + Self::INDICATOR_SIZE)
            {
                Pixel(Point::new(fill_x, fill_y), BinaryColor::On).draw(display)?;
            }
        }

        // Draw dotted remainder line
        let mut dot_x = atk_end_x;
        while dot_x > atk_effective_x + 1 {
            Pixel(Point::new(dot_x, atk_start_y), BinaryColor::On).draw(display)?;
            dot_x -= 2;
        }

        Ok(())
    }
}

impl Positionable for Attack {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for Attack {
    fn size(&self) -> Size {
        Size::new(Self::WIDTH as u32, Self::HEIGHT as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_attack_draw() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let attack = Attack::new(Point::new(10, 20), 0.4);
        attack.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut attack = Attack::new(Point::new(10, 20), 0.5);
        attack.set_value(2.0);
        assert_eq!(attack.value(), 1.0);
        attack.set_value(-0.5);
        assert_eq!(attack.value(), 0.0);
    }
}
