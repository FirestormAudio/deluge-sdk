use crate::prelude::*;
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
};

use crate::{FilledPolygon, Positionable, Waveform};

#[derive(Debug, Clone)]
pub struct LoopedWaveform {
    waveform: Waveform,
    position: Point,
    size: Size,
    loop_start: usize, // sample index
    loop_end: usize,   // sample index
}

impl LoopedWaveform {
    pub fn new(
        data: Vec<f32>,
        loop_start: usize,
        loop_end: usize,
        position: Point,
        size: Size,
    ) -> Self {
        Self {
            waveform: Waveform::new(
                data,
                Point::new(position.x, position.y + 4),
                Size::new(size.width, size.height - 8),
            ),
            position,
            size,
            loop_start,
            loop_end,
        }
    }
}

impl Drawable for LoopedWaveform {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        self.waveform.draw(display)?;

        // Draw loop markers
        let data_len = self.waveform.data().len();
        let loop_start_x = self.position.x
            + ((self.loop_start as f32 / data_len as f32) * self.size.width as f32) as i32;
        let loop_end_x = self.position.x
            + ((self.loop_end as f32 / data_len as f32) * self.size.width as f32) as i32;
        Line::new(
            Point::new(loop_start_x, self.position.y),
            Point::new(loop_start_x, self.position.y + self.size.height as i32),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
        Line::new(
            Point::new(loop_end_x, self.position.y),
            Point::new(loop_end_x, self.position.y + self.size.height as i32),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        // Draw filled right-facing triangle at the top for the loop start
        // Vertical edge aligned with loop start marker
        FilledPolygon::triangle(
            Point::new(loop_start_x, self.position.y),
            Point::new(loop_start_x, self.position.y + 4),
            Point::new(loop_start_x + 4, self.position.y + 2),
        )
        .draw(display)?;

        // Draw filled left-facing triangle at the top for the loop end
        // Vertical edge aligned with loop end marker
        FilledPolygon::triangle(
            Point::new(loop_end_x, self.position.y),
            Point::new(loop_end_x, self.position.y + 4),
            Point::new(loop_end_x - 4, self.position.y + 2),
        )
        .draw(display)?;

        Ok(())
    }
}

impl Positionable for LoopedWaveform {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
        self.waveform.set_position(Point::new(point.x, point.y + 4));
    }
}

impl OriginDimensions for LoopedWaveform {
    fn size(&self) -> Size {
        self.size
    }
}
