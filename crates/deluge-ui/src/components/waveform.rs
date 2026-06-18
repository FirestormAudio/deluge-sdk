use crate::prelude::*;
use embedded_graphics::{
    Drawable,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
};

use crate::Positionable;

#[derive(Debug, Clone)]
pub struct Waveform {
    data: Vec<f32>,
    position: Point,
    size: Size,
}

impl Waveform {
    pub fn new(data: Vec<f32>, position: Point, size: Size) -> Self {
        Self {
            data,
            position,
            size,
        }
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }
}

impl Drawable for Waveform {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        // need to find min and max to scale properly
        let min = self.data.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = self.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let abs_max = max.max(min.abs());

        // scale should let us multiply the abs(max, -min) to fit in half the height
        let scale = if abs_max == 0.0 {
            0.0
        } else {
            (self.size.height as f32 / 2.0) / abs_max
        };

        let mut last_point: Option<Point> = None;
        let data_len = self.data.len() as f32;
        for (i, &sample) in self.data.iter().enumerate() {
            let x = self.position.x + ((i as f32 / data_len) * self.size.width as f32) as i32;
            let y = self.position.y + (self.size.height as f32 / 2.0) as i32
                - ((sample.clamp(-1.0, 1.0) * scale) as i32);
            let current_point = Point::new(x, y);
            if let Some(last) = last_point {
                Line::new(last, current_point)
                    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                    .draw(display)?;
            }
            last_point = Some(current_point);
        }
        Ok(())
    }
}

impl OriginDimensions for Waveform {
    fn size(&self) -> Size {
        self.size
    }
}

impl Positionable for Waveform {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
    }
}
