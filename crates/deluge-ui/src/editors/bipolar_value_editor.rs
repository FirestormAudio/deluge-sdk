//! Bipolar value editor — large centred number + bipolar bar with zero-crossing tick.
//!
//! Renders a centred `MetricBold13px` value string and a fill bar that grows
//! outward from a configurable zero-crossing point, matching the layout used
//! throughout the Deluge menu system for pan, detune, and other bipolar params.
//!
//! # Display geometry (128×43 visible buffer)
//!
//! ```text
//!  y=0  ┌─────────────────────────────────┐
//!  y=1  │  (title drawn by caller)        │
//! y=11  ├─────────────────────────────────┤
//! y=15  │       +VALUE (MetricBold13px)   │
//! y=31  │    ╔═══════╦═══════════════╗    │  bar: fill + zero-crossing tick
//! y=37  │    ╚═══════╩═══════════════╝    │
//! y=42  └─────────────────────────────────┘
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

/// Composite editor widget: centred value text + bipolar fill bar.
///
/// Both `value_frac` and `zero_frac` are normalised positions within the bar
/// in [0.0, 1.0]:
/// - `value_frac` — where the current value falls in the range
/// - `zero_frac`  — where zero (the centre/origin) falls in the range
///   (0.5 for symmetric ±N ranges; use [`BipolarValueEditor::symmetric`] as a
///   shorthand)
///
/// The fill region spans from `zero_frac` to `value_frac`; a 1 px tick marks
/// the zero crossing.
#[derive(Debug, Clone)]
pub struct BipolarValueEditor<'a> {
    display_text: &'a str,
    /// Current value normalised to [0.0, 1.0]
    value_frac: f32,
    /// Zero-crossing position normalised to [0.0, 1.0]
    zero_frac: f32,
}

impl<'a> BipolarValueEditor<'a> {
    /// Create a new bipolar value editor.
    ///
    /// * `display_text` — pre-formatted value string (e.g. `"+25"`, `"-12"`)
    /// * `value_frac`   — current value normalised to [0.0, 1.0]
    /// * `zero_frac`    — zero-crossing position normalised to [0.0, 1.0]
    pub fn new(display_text: &'a str, value_frac: f32, zero_frac: f32) -> Self {
        Self {
            display_text,
            value_frac: value_frac.clamp(0.0, 1.0),
            zero_frac: zero_frac.clamp(0.0, 1.0),
        }
    }

    /// Convenience constructor for symmetric bipolar ranges (zero at centre).
    ///
    /// * `value_frac` — current value normalised to [0.0, 1.0] (0.5 = zero)
    pub fn symmetric(display_text: &'a str, value_frac: f32) -> Self {
        Self::new(display_text, value_frac, 0.5)
    }
}

impl Drawable for BipolarValueEditor<'_> {
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

        let bar_width = (BAR_RIGHT - BAR_LEFT) as f32;
        let zero_x = BAR_LEFT + (self.zero_frac * bar_width) as i32;
        let value_x = BAR_LEFT + (self.value_frac * bar_width) as i32;

        // Fill region between zero crossing and current value
        let fill_x = zero_x.min(value_x) + 1;
        let fill_w = (zero_x - value_x).unsigned_abs().saturating_sub(1);
        if fill_w > 0 {
            Rectangle::new(
                Point::new(fill_x, BAR_Y + 1),
                Size::new(fill_w, (BAR_H - 2) as u32),
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

        // Zero-crossing tick (1 px wide, full inner bar height)
        Rectangle::new(
            Point::new(zero_x, BAR_Y + 1),
            Size::new(1, (BAR_H - 2) as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;

        Ok(())
    }
}
