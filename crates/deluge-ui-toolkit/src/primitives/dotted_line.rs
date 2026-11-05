use embedded_graphics::{
    Drawable,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
};
// Required for no_std ARM target where core_float_math is unavailable.
#[allow(unused_imports)]
use crate::prelude::F32Ext as _;

#[derive(Debug, Clone)]
pub struct DottedLine {
    line: Line,
    dash_length: i32,
    gap_length: i32,
}

impl DottedLine {
    pub fn new(start: Point, end: Point, length: i32) -> Self {
        // Ensure length is at least 1 to prevent infinite loops
        let length = length.max(1);
        Self {
            line: Line::new(start, end),
            dash_length: length,
            gap_length: length,
        }
    }

    pub fn with_gap_length(mut self, gap_length: i32) -> Self {
        // Ensure gap_length is at least 1 to prevent infinite loops
        self.gap_length = gap_length.max(1);
        self
    }
}

impl Drawable for DottedLine {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let Line { start, end } = self.line;

        if (start.x == end.x) && (start.y != end.y) {
            // Vertical line
            let min_y = start.y.min(end.y);
            let max_y = start.y.max(end.y);
            let mut y = min_y;

            while y <= max_y {
                let dash_end_y = (y + self.dash_length).min(max_y);
                Line::new(Point::new(start.x, y), Point::new(start.x, dash_end_y))
                    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                    .draw(display)?;
                y += self.dash_length + self.gap_length;
            }

            return Ok(());
        } else if (start.y == end.y) && (start.x != end.x) {
            // Horizontal line
            let min_x = start.x.min(end.x);
            let max_x = start.x.max(end.x);
            let mut x = min_x;

            while x <= max_x {
                let dash_end_x = (x + self.dash_length).min(max_x);
                Line::new(Point::new(x, start.y), Point::new(dash_end_x, start.y))
                    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                    .draw(display)?;
                x += self.dash_length + self.gap_length;
            }

            return Ok(());
        } else {
            let total_length = ((end.x - start.x).pow(2) + (end.y - start.y).pow(2)) as f32;
            let dash_gap_length = (self.dash_length + self.gap_length) as f32;
            let num_dashes = (total_length / dash_gap_length).ceil() as i32;

            for i in 0..num_dashes {
                let dash_start_ratio: f32 = (i as f32 * dash_gap_length) / total_length;
                let dash_end_ratio: f32 =
                    ((i as f32 * dash_gap_length) + self.dash_length as f32) / total_length;

                let dash_start_x = start.x + ((end.x - start.x) as f32 * dash_start_ratio) as i32;
                let dash_start_y = start.y + ((end.y - start.y) as f32 * dash_start_ratio) as i32;
                let dash_end_x = start.x + ((end.x - start.x) as f32 * dash_end_ratio) as i32;
                let dash_end_y = start.y + ((end.y - start.y) as f32 * dash_end_ratio) as i32;

                Line::new(
                    Point::new(dash_start_x, dash_start_y),
                    Point::new(dash_end_x, dash_end_y),
                )
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                .draw(display)?;
            }
        }

        Ok(())
    }
}
