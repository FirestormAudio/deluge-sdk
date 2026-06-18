//! Unipolar value editor — large centred number + unipolar bar indicator.
//!
//! Renders a centred `MetricBold13px` value string and a proportional fill bar
//! matching the layout used throughout the Deluge menu system.
//!
//! # Display geometry (128×43 visible buffer)
//!
//! ```text
//!  y=0  ┌──────────────────────────────────┐
//!  y=1  │  (title drawn by caller)         │
//! y=11  ├──────────────────────────────────┤  separator
//! y=15  │       VALUE  (MetricBold13px)    │
//! y=28  │                                  │
//! y=31  │  ╔════════════════════════════╗  │  bar outline
//! y=32  │  ║███████░░░░░░░░░░░░░░░░░░░░░║  │  bar fill (proportional)
//! y=37  │  ╚════════════════════════════╝  │
//! y=42  └──────────────────────────────────┘
//! ```
//!
//! The title row is drawn by the caller (`render_title`); this widget covers
//! only the value area below it.

use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::text::{Font, TextStyle, draw_text};

const BAR_LEFT: i32 = 12;
const BAR_RIGHT: i32 = 115;
const BAR_Y: i32 = 31;
const BAR_H: i32 = 7;

/// Composite editor widget: centred value text + unipolar fill bar.
///
/// Pass a pre-formatted display string and the current normalised value
/// (0.0 = minimum, 1.0 = maximum).  The title row must be drawn separately
/// by the caller.
#[derive(Debug, Clone)]
pub struct UnipolarValueEditor<'a> {
    display_text: &'a str,
    /// Normalised value in [0.0, 1.0]
    value: f32,
}

impl<'a> UnipolarValueEditor<'a> {
    /// Create a new unipolar value editor.
    ///
    /// * `display_text` — pre-formatted value string (e.g. `"75"`, `"0.50"`)
    /// * `value` — current value normalised to [0.0, 1.0]
    pub fn new(display_text: &'a str, value: f32) -> Self {
        Self {
            display_text,
            value: value.clamp(0.0, 1.0),
        }
    }
}

impl Drawable for UnipolarValueEditor<'_> {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Self::Color>,
    {
        // Centred large value text
        let style = TextStyle::new(Font::MetricBold13px)
            .with_alignment(embedded_graphics::text::Alignment::Center);
        draw_text(display, self.display_text, Point::new(64, 15), style)?;

        // Bar fill (left-to-right proportional), clamped to the inner width
        // between the 1 px outline walls so it never overflows the right edge.
        let inner = (BAR_RIGHT - BAR_LEFT - 2).max(0);
        let fill_width = ((self.value * inner as f32) as i32).clamp(0, inner);
        if fill_width > 0 {
            Rectangle::new(
                Point::new(BAR_LEFT + 1, BAR_Y + 1),
                Size::new(fill_width as u32, (BAR_H - 2) as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        }

        // Bar outline (plain rectangle)
        Rectangle::new(
            Point::new(BAR_LEFT, BAR_Y),
            Size::new((BAR_RIGHT - BAR_LEFT) as u32, BAR_H as u32),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;

        Ok(())
    }
}
