//! Text value editor — large centred text string.
//!
//! Used for enum/selection parameters where the value is expressed as a label
//! (e.g. "Sine", "Triangle", "Saw") rather than a number.  Renders the text
//! centred horizontally with `MetricBold13px`, vertically positioned in the
//! middle of the space below the title bar.
//!
//! # Display geometry (128×43 visible buffer)
//!
//! ```text
//!  y=0  ┌──────────────────────────────────┐
//!  y=1  │  (title drawn by caller)         │
//! y=11  ├──────────────────────────────────┤
//!       │                                  │
//! y=20  │         VALUE (MetricBold13px)   │  centred horizontally
//!       │                                  │
//! y=42  └──────────────────────────────────┘
//! ```

use embedded_graphics::{
    Drawable, draw_target::DrawTarget, geometry::Point, pixelcolor::BinaryColor, text::Alignment,
};

use crate::text::{Font, TextStyle, draw_text};

/// Editor widget for enum / text-selection parameters.
///
/// Renders the provided label string centred horizontally at a fixed vertical
/// position below the title bar.  The title must be drawn separately by the
/// caller.
#[derive(Debug, Clone)]
pub struct TextValueEditor<'a> {
    text: &'a str,
}

impl<'a> TextValueEditor<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text }
    }
}

impl Drawable for TextValueEditor<'_> {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        let style = TextStyle::new(Font::MetricBold13px).with_alignment(Alignment::Center);
        draw_text(display, self.text, Point::new(64, 20), style)
    }
}
