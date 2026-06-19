use crate::prelude::*;
use crate::{DottedLine, Positionable, Waveform};
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};

#[derive(Debug, Clone)]
pub struct SlicedWaveform {
    waveform: Waveform,
    position: Point,
    size: Size,
    /// Sample indices in the waveform data where slices occur
    slice_points: Vec<usize>,
}

impl SlicedWaveform {
    pub fn new(data: Vec<f32>, slice_points: Vec<usize>, position: Point, size: Size) -> Self {
        Self {
            waveform: Waveform::new(
                data,
                Point::new(position.x, position.y + 2),
                Size::new(size.width, size.height - 4),
            ),
            position,
            size,
            slice_points,
        }
    }
}

impl Drawable for SlicedWaveform {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        self.waveform.draw(display)?;

        // Draw slice markers as full-height dotted vertical lines
        for &slice_point in &self.slice_points {
            let x = slice_point * self.size.width as usize / self.waveform.data().len();
            DottedLine::new(
                Point::new(self.position.x + x as i32, self.position.y),
                Point::new(
                    self.position.x + x as i32,
                    self.position.y + self.size.height as i32 - 1,
                ),
                2,
            )
            .draw(display)?;
        }

        Ok(())
    }
}

impl Positionable for SlicedWaveform {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
        self.waveform.set_position(Point::new(point.x, point.y + 2));
    }
}

impl OriginDimensions for SlicedWaveform {
    fn size(&self) -> Size {
        self.size
    }
}
