//! Release stage visualization control
//!
//! Displays envelope release stage with horizontal sustain to downward diagonal.

use crate::Positionable;
use embedded_graphics::{
    Drawable, Pixel,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Release stage indicator for envelope visualization
///
/// Shows horizontal sustain transitioning to a downward diagonal release with endpoint marker.
/// The endpoint position indicates the release time.
/// Dimensions: 19×11 pixels (starts at x+5 from point)
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::Release;
/// use embedded_graphics::prelude::*;
///
/// let mut release = Release::new(Point::new(10, 20), 0.6);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Release {
    point: Point,
    value: f32, // 0.0 to 1.0 (release time)
}

impl Release {
    const WIDTH: i32 = 19;
    const HEIGHT: i32 = 11;
    const X_OFFSET: i32 = 5; // Drawing starts at x+5
    const Y_OFFSET: i32 = 0; // Adjusted for -1 in original
    const SUSTAIN_WIDTH: i32 = 4;
    const INDICATOR_SIZE: i32 = 2; // Half-size of 5×4 square

    /// Create a new release stage visualization
    ///
    /// # Arguments
    /// * `point` - Top-left position (actual drawing starts at x+5)
    /// * `value` - Release time from 0.0 (fast) to 1.0 (slow)
    pub fn new(point: Point, value: f32) -> Self {
        Self {
            point,
            value: value.clamp(0.0, 1.0),
        }
    }

    /// Update the release time value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Get the current release time value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for Release {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let rel_start_x = self.point.x + Self::X_OFFSET;
        let rel_end_x = rel_start_x + Self::WIDTH - 1;
        let rel_start_y = self.point.y + Self::Y_OFFSET;
        let rel_end_y = rel_start_y + Self::HEIGHT - 1;

        let rel_stage_start_x = rel_start_x + Self::SUSTAIN_WIDTH;

        // Linear interpolation for release endpoint
        let rel_effective_x =
            rel_stage_start_x + (self.value * (rel_end_x - rel_stage_start_x) as f32) as i32;

        // Draw horizontal sustain portion
        Line::new(
            Point::new(rel_start_x, rel_start_y),
            Point::new(rel_stage_start_x, rel_start_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw release line (sustain level to bottom)
        Line::new(
            Point::new(rel_stage_start_x, rel_start_y),
            Point::new(rel_effective_x, rel_end_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw square endpoint indicator (5×4 filled square)
        for fill_x in
            (rel_effective_x - Self::INDICATOR_SIZE)..=(rel_effective_x + Self::INDICATOR_SIZE)
        {
            for fill_y in
                (rel_end_y - Self::INDICATOR_SIZE)..=(rel_end_y + Self::INDICATOR_SIZE - 1)
            {
                Pixel(Point::new(fill_x, fill_y), BinaryColor::On).draw(display)?;
            }
        }

        // Draw dotted remainder line
        let mut dot_x = rel_end_x;
        while dot_x > rel_effective_x + 1 {
            Pixel(Point::new(dot_x, rel_end_y), BinaryColor::On).draw(display)?;
            dot_x -= 2;
        }

        Ok(())
    }
}

impl Positionable for Release {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for Release {
    fn size(&self) -> Size {
        Size::new(Self::WIDTH as u32, Self::HEIGHT as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_release_draw() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let release = Release::new(Point::new(10, 20), 0.6);
        release.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut release = Release::new(Point::new(10, 20), 0.5);
        release.set_value(2.0);
        assert_eq!(release.value(), 1.0);
        release.set_value(-0.5);
        assert_eq!(release.value(), 0.0);
    }
}
