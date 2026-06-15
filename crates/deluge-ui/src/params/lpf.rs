//! Low-pass filter visualization control
//!
//! Displays frequency response curve for LPF with horizontal passband to downward slope.

use crate::Positionable;
use embedded_graphics::{
    Drawable, Pixel,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// Low-pass filter frequency response visualization
///
/// Shows a horizontal passband transitioning to a downward slope.
/// The slope position indicates the cutoff frequency.
/// Dimensions: 21×11 pixels (starts at x+5 from point)
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::LowPassFilter;
/// use embedded_graphics::prelude::*;
///
/// let mut lpf = LowPassFilter::new(Point::new(10, 20), 0.7);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LowPassFilter {
    point: Point,
    value: f32, // 0.0 to 1.0 (cutoff frequency)
}

impl LowPassFilter {
    const SLOPE_WIDTH: i32 = 5;
    const WIDTH: i32 = 21;
    const HEIGHT: i32 = 11;
    const X_OFFSET: i32 = 5; // Drawing starts at x+5

    /// Create a new low-pass filter visualization
    ///
    /// # Arguments
    /// * `point` - Top-left position (actual drawing starts at x+5)
    /// * `value` - Cutoff frequency from 0.0 (low) to 1.0 (high)
    pub fn new(point: Point, value: f32) -> Self {
        Self {
            point,
            value: value.clamp(0.0, 1.0),
        }
    }

    /// Update the cutoff frequency value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Get the current cutoff frequency value
    pub fn value(&self) -> f32 {
        self.value
    }
}

impl Drawable for LowPassFilter {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let lpf_start_x = self.point.x + Self::X_OFFSET;
        let lpf_end_x = lpf_start_x + Self::WIDTH - 1;
        let lpf_start_y = self.point.y + 1;
        let lpf_end_y = lpf_start_y + Self::HEIGHT - 1;

        // Linear interpolation for slope position
        let slope_start_x = lpf_start_x
            + 3
            + ((self.value * (lpf_end_x - Self::SLOPE_WIDTH - lpf_start_x - 3) as f32) as i32);
        let slope_end_x = slope_start_x + Self::SLOPE_WIDTH;

        // Draw the downward slope (thick line - 2 pixels)
        Line::new(
            Point::new(slope_start_x, lpf_start_y),
            Point::new(slope_end_x, lpf_end_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        Line::new(
            Point::new(slope_start_x + 1, lpf_start_y),
            Point::new(slope_end_x + 1, lpf_end_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw horizontal passband (2 pixels thick)
        Line::new(
            Point::new(lpf_start_x, lpf_start_y),
            Point::new(slope_start_x, lpf_start_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        Line::new(
            Point::new(lpf_start_x, lpf_start_y + 1),
            Point::new(slope_start_x, lpf_start_y + 1),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw dotted stopband (horizontal)
        let mut dot_x = lpf_end_x;
        while dot_x > slope_end_x {
            Pixel(Point::new(dot_x, lpf_start_y), BinaryColor::On).draw(display)?;
            dot_x -= 3;
        }

        // Draw dotted stopband (vertical) if there's room
        if slope_end_x != lpf_end_x {
            let mut dot_y = lpf_start_y;
            while dot_y < lpf_end_y {
                Pixel(Point::new(lpf_end_x, dot_y), BinaryColor::On).draw(display)?;
                dot_y += 3;
            }
        }

        Ok(())
    }
}

impl Positionable for LowPassFilter {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for LowPassFilter {
    fn size(&self) -> Size {
        Size::new(Self::WIDTH as u32, Self::HEIGHT as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_lpf_draw() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let lpf = LowPassFilter::new(Point::new(10, 20), 0.7);
        lpf.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut lpf = LowPassFilter::new(Point::new(10, 20), 0.5);
        lpf.set_value(2.0);
        assert_eq!(lpf.value(), 1.0);
        lpf.set_value(-0.5);
        assert_eq!(lpf.value(), 0.0);
    }
}
