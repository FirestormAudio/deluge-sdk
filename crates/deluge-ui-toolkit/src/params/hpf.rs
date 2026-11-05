//! High-pass filter visualization control
//!
//! Displays frequency response curve for HPF with upward slope to horizontal passband.

use crate::Positionable;
use embedded_graphics::{
    Drawable, Pixel,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::OriginDimensions,
    primitives::{Line, Primitive, PrimitiveStyle},
};

/// High-pass filter frequency response visualization
///
/// Shows an upward slope transitioning to a horizontal passband.
/// The slope position indicates the cutoff frequency.
/// Dimensions: 21×11 pixels (starts at x+5 from point)
///
/// # Example
/// ```
/// use deluge_ui_toolkit::params::HighPassFilter;
/// use embedded_graphics::prelude::*;
///
/// let mut hpf = HighPassFilter::new(Point::new(10, 20), 0.3);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct HighPassFilter {
    point: Point,
    value: f32, // 0.0 to 1.0 (cutoff frequency)
}

impl HighPassFilter {
    const SLOPE_WIDTH: i32 = 5;
    const WIDTH: i32 = 21;
    const HEIGHT: i32 = 11;
    const X_OFFSET: i32 = 5; // Drawing starts at x+5

    /// Create a new high-pass filter visualization
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

impl Drawable for HighPassFilter {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let hpf_start_x = self.point.x + Self::X_OFFSET;
        let hpf_end_x = hpf_start_x + Self::WIDTH - 1;
        let hpf_start_y = self.point.y + 1;
        let hpf_end_y = hpf_start_y + Self::HEIGHT - 1;

        // Linear interpolation for slope position
        let slope_start_x = hpf_start_x
            + ((self.value * (hpf_end_x - Self::SLOPE_WIDTH - 4 - hpf_start_x) as f32) as i32);
        let slope_end_x = slope_start_x + Self::SLOPE_WIDTH;

        // Draw the upward slope (thick line - 2 pixels)
        Line::new(
            Point::new(slope_start_x, hpf_end_y),
            Point::new(slope_end_x, hpf_start_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        Line::new(
            Point::new(slope_start_x + 1, hpf_end_y),
            Point::new(slope_end_x + 1, hpf_start_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw horizontal passband (2 pixels thick)
        Line::new(
            Point::new(slope_end_x, hpf_start_y),
            Point::new(hpf_end_x, hpf_start_y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        Line::new(
            Point::new(slope_end_x, hpf_start_y + 1),
            Point::new(hpf_end_x, hpf_start_y + 1),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw dotted stopband (horizontal)
        let mut dot_x = hpf_start_x;
        while dot_x < slope_start_x {
            Pixel(Point::new(dot_x, hpf_start_y), BinaryColor::On).draw(display)?;
            dot_x += 3;
        }

        // Draw dotted stopband (vertical) if there's room
        if slope_start_x != hpf_start_x {
            let mut dot_y = hpf_start_y;
            while dot_y < hpf_end_y {
                Pixel(Point::new(hpf_start_x, dot_y), BinaryColor::On).draw(display)?;
                dot_y += 3;
            }
        }

        Ok(())
    }
}

impl Positionable for HighPassFilter {
    fn position(&self) -> Point {
        self.point
    }

    fn set_position(&mut self, point: Point) {
        self.point = point;
    }
}

impl OriginDimensions for HighPassFilter {
    fn size(&self) -> Size {
        Size::new(Self::WIDTH as u32, Self::HEIGHT as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_hpf_draw() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 64));
        let hpf = HighPassFilter::new(Point::new(10, 20), 0.3);
        hpf.draw(&mut display).unwrap();
    }

    #[test]
    fn test_value_clamping() {
        let mut hpf = HighPassFilter::new(Point::new(10, 20), 0.5);
        hpf.set_value(2.0);
        assert_eq!(hpf.value(), 1.0);
        hpf.set_value(-0.5);
        assert_eq!(hpf.value(), 0.0);
    }
}
