use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};

use crate::{IconData, Positionable};

#[derive(Debug, Clone, Copy)]
pub struct Icon {
    pub icon_data: &'static IconData,
    pub position: Point,
    pub color: BinaryColor,
}

impl Icon {
    pub fn new(icon_data: &'static IconData, position: Point) -> Self {
        Self {
            icon_data,
            position,
            color: BinaryColor::On,
        }
    }

    pub fn with_color(mut self, color: BinaryColor) -> Self {
        self.color = color;
        self
    }
}

impl Drawable for Icon {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        use crate::icons::{IconFormat, get_bmp_pixel};

        // Handle BMP format icons
        match self.icon_data.format {
            IconFormat::Bmp => {
                for y in 0..self.icon_data.height {
                    for x in 0..self.icon_data.width {
                        if get_bmp_pixel(self.icon_data, x, y) {
                            let pixel_pos =
                                Point::new(self.position.x + x as i32, self.position.y + y as i32);
                            Pixel(pixel_pos, self.color).draw(display)?;
                        }
                    }
                }
            }
            IconFormat::RawBitmap => {
                // Row-major raw bitmap format (1 bit per pixel)
                let row_size_bytes = (self.icon_data.width as usize).div_ceil(8);
                for y in 0..self.icon_data.height {
                    for x in 0..self.icon_data.width {
                        let byte_index = (y as usize * row_size_bytes) + (x as usize / 8);
                        let bit_index = 7 - (x % 8);
                        if byte_index < self.icon_data.data.len() {
                            let byte = self.icon_data.data[byte_index];
                            if (byte & (1 << bit_index)) != 0 {
                                let pixel_pos = Point::new(
                                    self.position.x + x as i32,
                                    self.position.y + y as i32,
                                );
                                Pixel(pixel_pos, self.color).draw(display)?;
                            }
                        }
                    }
                }
                return Ok(());
            }
        }

        Ok(())
    }
}

impl Positionable for Icon {
    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, point: Point) {
        self.position = point;
    }
}

impl OriginDimensions for Icon {
    fn size(&self) -> Size {
        Size::new(self.icon_data.width as u32, self.icon_data.height as u32)
    }
}
