//! Vertical scrollbar component.
//!
//! A 1 px track rail with a proportional 3 px-wide hollow indicator, matching
//! the Deluge menu look. Drawn only when the content actually overflows
//! (`total > visible`); otherwise it is a no-op, so callers can construct it
//! unconditionally.

use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
};

/// A vertical scrollbar drawn at column `x`, spanning `top..top+height`.
///
/// The indicator height is proportional to `visible / total` (min 3 px) and its
/// position to `scroll / (total - visible)`; the rest of the track is a 1 px
/// rail above and below it. The indicator straddles the rail (`x-1 ..= x+1`).
#[derive(Debug, Clone, Copy)]
pub struct Scrollbar {
    /// X of the track rail.
    pub x: i32,
    /// Top Y of the track.
    pub top: i32,
    /// Track height in pixels.
    pub height: i32,
    /// Total number of items.
    pub total: u16,
    /// Items visible at once.
    pub visible: u16,
    /// Index of the first visible item.
    pub scroll: u16,
}

impl Scrollbar {
    /// Construct a scrollbar from its track geometry and scroll state.
    pub fn new(x: i32, top: i32, height: i32, total: u16, visible: u16, scroll: u16) -> Self {
        Self {
            x,
            top,
            height,
            total,
            visible,
            scroll,
        }
    }
}

impl Drawable for Scrollbar {
    type Color = BinaryColor;
    type Output = ();

    fn draw<D>(&self, d: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        // No bar needed when everything fits (or a degenerate track).
        if self.total <= self.visible || self.height <= 0 || self.visible == 0 {
            return Ok(());
        }

        let total = self.total as f32;
        let visible = self.visible as f32;
        let ind_h = (((visible / total) * self.height as f32) as i32).max(3);
        let ratio = self.scroll as f32 / (total - visible).max(1.0);
        let ind_y = self.top + ((self.height - ind_h) as f32 * ratio) as i32;

        let stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

        // Track rail above and below the indicator, so the bar reads as a
        // position within a fixed groove.
        if ind_y > self.top {
            Line::new(Point::new(self.x, self.top), Point::new(self.x, ind_y - 1))
                .into_styled(stroke)
                .draw(d)?;
        }
        let ind_bottom = ind_y + ind_h;
        let track_bottom = self.top + self.height - 1;
        if ind_bottom < track_bottom {
            Line::new(
                Point::new(self.x, ind_bottom),
                Point::new(self.x, track_bottom),
            )
            .into_styled(stroke)
            .draw(d)?;
        }

        // Indicator: a 3 px-wide hollow box straddling the rail.
        Rectangle::new(Point::new(self.x - 1, ind_y), Size::new(3, ind_h as u32))
            .into_styled(stroke)
            .draw(d)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics_simulator::SimulatorDisplay;

    fn lit(d: &SimulatorDisplay<BinaryColor>) -> usize {
        d.bounding_box()
            .points()
            .filter(|&p| d.get_pixel(p) == BinaryColor::On)
            .count()
    }

    #[test]
    fn no_bar_when_everything_fits() {
        let mut d: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 48));
        Scrollbar::new(126, 14, 27, 3, 3, 0).draw(&mut d).unwrap();
        assert_eq!(lit(&d), 0, "no scrollbar when total <= visible");
    }

    #[test]
    fn draws_track_and_indicator_when_overflowing() {
        let mut d: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 48));
        Scrollbar::new(126, 14, 27, 8, 3, 0).draw(&mut d).unwrap();
        assert!(lit(&d) > 10, "expected a track + indicator to be drawn");
    }

    #[test]
    fn indicator_moves_down_with_scroll() {
        // Indicator centre is lower when scrolled further down.
        let centre = |scroll| {
            let mut d: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(128, 48));
            Scrollbar::new(126, 14, 27, 8, 3, scroll)
                .draw(&mut d)
                .unwrap();
            // Mean y of lit indicator pixels (x = 125, the indicator's left wall).
            let ys: heapless::Vec<i32, 64> = (0..48)
                .filter(|&y| d.get_pixel(Point::new(125, y)) == BinaryColor::On)
                .collect();
            ys.iter().sum::<i32>() / ys.len().max(1) as i32
        };
        assert!(
            centre(0) < centre(5),
            "indicator should descend as scroll grows"
        );
    }
}
