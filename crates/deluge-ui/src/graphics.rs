//! Graphics helper functions for the Deluge OLED display.

use crate::prelude::*;
use embedded_graphics::{
    Pixel,
    draw_target::DrawTarget,
    geometry::Point,
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
};

use crate::icons::{IconData, get_bmp_pixel};

/// Draw an [`IconData`] bitmap at `position` using the given `color`.
///
/// Each lit pixel in the icon is drawn as a single pixel at the corresponding
/// offset from `position`.
pub fn draw_icon_data<D>(
    display: &mut D,
    icon: &IconData,
    position: Point,
    color: BinaryColor,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let pixels: Vec<Pixel<BinaryColor>> = (0..icon.height)
        .flat_map(|y| {
            (0..icon.width).filter_map(move |x| {
                if get_bmp_pixel(icon, x, y) {
                    Some(Pixel(
                        Point::new(position.x + x as i32, position.y + y as i32),
                        color,
                    ))
                } else {
                    None
                }
            })
        })
        .collect();

    display.draw_iter(pixels)
}

/// Draw a straight line from `start` to `end` in the given `color`.
pub fn draw_line<D>(
    display: &mut D,
    start: Point,
    end: Point,
    color: BinaryColor,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    Line::new(start, end)
        .into_styled(PrimitiveStyle::with_stroke(color, 1))
        .draw(display)
}
