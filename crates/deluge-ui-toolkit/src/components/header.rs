use crate::prelude::*;
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle, StyledDrawable},
    text::Text,
};

use crate::{FONT_APPLE, VariTextStyle};

#[derive(Clone, Debug)]
pub struct Header {
    text: String,
}

impl Header {
    /// Create a new header component with the given text
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Drawable for Header {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        RoundedRectangle::new(
            Rectangle::new(Point::new(1, -2), Size::new(126, 12)),
            CornerRadii::new(Size::new(2, 2)),
        )
        .draw_styled(&PrimitiveStyle::with_fill(BinaryColor::On), display)?;

        Text::new(
            &self.text,
            Point::new(4, 2),
            VariTextStyle::new(FONT_APPLE).with_text_color(BinaryColor::Off),
        )
        .draw(display)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_creation() {
        let header = Header::new("Test Header");
        assert_eq!(header.text, "Test Header");
    }

    #[test]
    fn test_header_draw_height() {
        use embedded_graphics::mock_display::MockDisplay;

        let header = Header::new("TEST");
        let mut display = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        display.set_allow_overdraw(true);
        header.draw(&mut display).unwrap();

        // The header should only draw in the first 10 pixels of height (y=0 to y=9)
        // Rectangle is at y=-2 with height 12, so visible area is y=0 to y=9
        let bounding_box = display.affected_area();

        // Check that no pixels are drawn beyond y=9
        assert!(
            bounding_box.bottom_right().unwrap().y <= 9,
            "Header draws beyond y=9: {:?}",
            bounding_box
        );
    }
}
