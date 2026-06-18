use crate::prelude::*;
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle},
    text::Text,
};

use crate::{FONT_METRIC_BOLD_9PX, VariTextStyle};

pub struct Title {
    text: String,
    separator: bool,
}

impl Title {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            separator: false,
        }
    }

    pub fn with_separator(mut self, separator: bool) -> Self {
        self.separator = separator;
        self
    }
}

impl Drawable for Title {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        Text::new(
            &self.text,
            Point::new(2, 0),
            VariTextStyle::new(FONT_METRIC_BOLD_9PX),
        )
        .draw(display)?;

        if self.separator {
            Line::new(Point::new(0, 10), Point::new(128, 10))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                .draw(display)?;
        }
        Ok(())
    }
}
