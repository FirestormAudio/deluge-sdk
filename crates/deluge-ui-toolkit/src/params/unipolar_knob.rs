//! Knob drawing utilities
//!
//! Provides functions to draw knobs with arcs and value indicators,
//! replicating the Deluge firmware's knob visualization.

use crate::Positionable;
use crate::{icons::KNOB_ARC, primitives::Icon};
// Required for no_std ARM target where core_float_math is unavailable.
#[allow(unused_imports)]
use crate::prelude::F32Ext as _;
use embedded_graphics::{
    Drawable,
    geometry::Size,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Knob control for parameter adjustment
///
/// Shows a circular knob with an arc background and a value indicator line,
/// matching the Deluge firmware's knob visualization.
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::UnipolarKnob;
/// use embedded_graphics::prelude::*;
///
/// let knob = UnipolarKnob::new(0.5);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct UnipolarKnob {
    point: Point,
    value: f32, // 0.0 to 1.0
}

impl Default for UnipolarKnob {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl UnipolarKnob {
    /// Create a new knob control
    ///
    /// # Arguments
    /// * `point` - Top-left position (knob will be centered from this point)
    /// * `value` - Normalized value from 0.0 to 1.0
    pub fn new(value: f32) -> Self {
        Self {
            point: Point::new(0, 0),
            value: value.clamp(0.0, 1.0),
        }
    }

    /// Set the knob value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Get the knob value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for UnipolarKnob {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        const KNOB_RADIUS: i32 = 10;
        const ARC_RANGE_ANGLE: f32 = 210.0;
        const BEGINNING_ANGLE: f32 = 165.0;
        const OUTER_RADIUS: f32 = 9.0; // knob_radius - 1
        const INNER_RADIUS: f32 = 3.5;

        let center_x = self.point.x + KNOB_RADIUS;
        let center_y = self.point.y + KNOB_RADIUS;

        // Draw the background arc (centered)
        let arc_icon_width = KNOB_ARC.width as i32;
        let icon_x = center_x - arc_icon_width / 2;
        let icon_y = center_y - KNOB_RADIUS;
        Icon::new(&KNOB_ARC, Point::new(icon_x, icon_y)).draw(display)?;

        // Calculate current value angle
        let current_angle = BEGINNING_ANGLE + (ARC_RANGE_ANGLE * self.value);
        let radians = current_angle * core::f32::consts::PI / 180.0;

        // Calculate line endpoints
        let cos_a = radians.cos();
        let sin_a = radians.sin();

        let line_start_x = center_x as f32 + INNER_RADIUS * cos_a;
        let line_start_y = center_y as f32 + INNER_RADIUS * sin_a;
        let line_end_x = center_x as f32 + OUTER_RADIUS * cos_a;
        let line_end_y = center_y as f32 + OUTER_RADIUS * sin_a;

        // Draw the indicator line
        Line::new(
            Point::new(line_start_x.round() as i32, line_start_y.round() as i32),
            Point::new(line_end_x.round() as i32, line_end_y.round() as i32),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        .draw(display)?;

        // Fill gap pixels when knob is near edges (for stylistic effect)
        let gap_y = icon_y + KNOB_ARC.height as i32 - 1;
        if current_angle < 180.0 {
            Pixel(Point::new(center_x - KNOB_RADIUS, gap_y), BinaryColor::On).draw(display)?;
        }
        if current_angle > 360.0 {
            Pixel(Point::new(center_x + KNOB_RADIUS, gap_y), BinaryColor::On).draw(display)?;
        }

        Ok(())
    }
}

impl Positionable for UnipolarKnob {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for UnipolarKnob {
    fn size(&self) -> Size {
        const KNOB_RADIUS: i32 = 10;
        Size::new((KNOB_RADIUS * 2) as u32, (KNOB_RADIUS * 2) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_draw_knob_center() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let mut knob = UnipolarKnob::new(0.5);
        knob.set_position(Point::new(54, 22));
        knob.draw(&mut display).unwrap();
    }

    #[test]
    fn test_draw_knob_min_max() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));

        // Min value
        let mut knob_min = UnipolarKnob::new(0.0);
        knob_min.set_position(Point::new(64, 32));
        knob_min.draw(&mut display).unwrap();

        // Max value
        let mut knob_max = UnipolarKnob::new(1.0);
        knob_max.set_position(Point::new(74, 32));
        knob_max.draw(&mut display).unwrap();
    }
}
