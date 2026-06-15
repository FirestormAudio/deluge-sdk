use crate::prelude::*;
use embedded_graphics::{
    Drawable,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
};

/// A filled polygon primitive
#[derive(Debug, Clone)]
pub struct FilledPolygon {
    points: Vec<Point>,
}

impl FilledPolygon {
    pub fn new(points: Vec<Point>) -> Self {
        Self { points }
    }

    /// Create a filled triangle from three points
    pub fn triangle(p1: Point, p2: Point, p3: Point) -> Self {
        Self::new(vec![p1, p2, p3])
    }
}

impl Drawable for FilledPolygon {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        if self.points.len() < 3 {
            return Ok(());
        }

        // Find bounding box
        let min_y = self.points.iter().map(|p| p.y).min().unwrap();
        let max_y = self.points.iter().map(|p| p.y).max().unwrap();

        // Scanline fill algorithm
        for y in min_y..=max_y {
            let mut intersections = Vec::new();

            // Find intersections with polygon edges at this scanline
            for i in 0..self.points.len() {
                let p1 = self.points[i];
                let p2 = self.points[(i + 1) % self.points.len()];

                // Check if edge crosses this scanline
                if (p1.y <= y && p2.y > y) || (p2.y <= y && p1.y > y) {
                    // Calculate intersection x coordinate
                    let x = if p2.y == p1.y {
                        p1.x
                    } else {
                        p1.x + ((y - p1.y) * (p2.x - p1.x)) / (p2.y - p1.y)
                    };
                    intersections.push(x);
                }
            }

            // Sort intersections
            intersections.sort_unstable();

            // Fill between pairs of intersections
            for chunk in intersections.chunks(2) {
                if chunk.len() == 2 {
                    Line::new(Point::new(chunk[0], y), Point::new(chunk[1], y))
                        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                        .draw(display)?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    #[test]
    fn test_filled_triangle() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(64, 64));

        FilledPolygon::triangle(Point::new(10, 10), Point::new(30, 10), Point::new(20, 30))
            .draw(&mut display)
            .unwrap();
    }

    #[test]
    fn test_filled_quad() {
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(64, 64));

        FilledPolygon::new(vec![
            Point::new(10, 10),
            Point::new(30, 10),
            Point::new(30, 30),
            Point::new(10, 30),
        ])
        .draw(&mut display)
        .unwrap();
    }
}
