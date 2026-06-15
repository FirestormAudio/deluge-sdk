//! Immediate-mode **horizontal** (param-column) menu for the Deluge OLED.
//!
//! The companion to [`Menu`](crate::menu::Menu): instead of a vertical list of
//! value rows, it lays out up to `max_visible` parameter **columns** (knob /
//! slider / bar / pan / filter visualizations from [`crate::params`]) with a
//! label under each. The horizontal encoder moves the selected column
//! ([`MenuInput::Turn`]); a second control edits the focused parameter directly
//! ([`MenuInput::Edit`]) — no edit mode, matching the Deluge.
//!
//! It shares [`MenuState`] / [`MenuStyle`] / [`MenuInput`] / [`Response`] with the
//! vertical menu, so an app can drill from a vertical [`Menu`](crate::menu::Menu)
//! submenu into a horizontal param page.
//!
//! ```ignore
//! let mut ui = HMenu::begin(&mut oled, &mut nav, input, &style);
//! ui.title("FILTER");
//! ui.lpf("CUT", &mut s.cutoff, 0.0..=1.0);
//! ui.knob("RES", &mut s.res, 0.0..=1.0);
//! ui.pan("PAN", &mut s.pan, -1.0..=1.0);
//! ui.end();
//! ```

use core::ops::RangeInclusive;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle, StyledDrawable},
    text::Alignment,
};

use crate::menu::{MenuInput, MenuState, MenuStyle, Response};
use crate::params::{
    Attack, BipolarBar, HighPassFilter, LengthSlider, LowPassFilter, Pan, Percent, Release, Slider,
    UnipolarBar, UnipolarKnob,
};
use crate::text::{TextStyle, draw_text};
use crate::{DISPLAY_WIDTH, Positionable};

/// Columns shown at once (the Deluge param view is 4-wide); independent of
/// [`MenuStyle::max_visible`], which sizes the vertical menu's rows.
const COLS: usize = 4;

// Layout (before `top_inset`).
const TITLE_H: i32 = 14;
const UNDERLINE_Y: i32 = 11;
const PARAM_TOP: i32 = 16;
const PARAM_H: i32 = 16;
const LABEL_Y: i32 = 34;
const LABEL_H: i32 = 9;
const BAR_H: u32 = 7;

/// Immediate-mode horizontal param menu for one frame.
pub struct HMenu<'a, D: DrawTarget<Color = BinaryColor>> {
    d: &'a mut D,
    state: &'a mut MenuState,
    style: &'a MenuStyle,
    /// Input still to apply to the focused column (`Edit`).
    pending: MenuInput,
    /// Column index within this frame.
    idx: usize,
    /// Number of columns this frame.
    cols: u16,
}

impl<'a, D: DrawTarget<Color = BinaryColor>> HMenu<'a, D> {
    /// Begin a frame. `Turn` moves the selected column; `Edit` flows to it.
    pub fn begin(
        d: &'a mut D,
        state: &'a mut MenuState,
        input: MenuInput,
        style: &'a MenuStyle,
    ) -> Self {
        let mut pending = input;
        match input {
            MenuInput::Turn(n) => {
                state.set_cursor((state.cursor() as i32 + n).max(0) as usize);
                pending = MenuInput::None;
            }
            // No drilldown in v1; Edit/Press flow to the focused column.
            MenuInput::Back => pending = MenuInput::None,
            MenuInput::Press | MenuInput::Edit(_) | MenuInput::None => {}
        }
        Self {
            d,
            state,
            style,
            pending,
            idx: 0,
            cols: 0,
        }
    }

    fn max_visible(&self) -> usize {
        COLS
    }

    fn col_w(&self) -> i32 {
        DISPLAY_WIDTH as i32 / COLS as i32
    }

    /// Draw the title + underline.
    pub fn title(&mut self, text: &str) {
        let inset = self.style.top_inset;
        let style = TextStyle::new(self.style.title_font).with_color(BinaryColor::On);
        let _ = draw_text(self.d, text, Point::new(3, inset + 1), style);
        let _ = Line::new(
            Point::new(0, inset + UNDERLINE_Y),
            Point::new(DISPLAY_WIDTH as i32 - 1, inset + UNDERLINE_Y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(self.d);
    }

    /// Column index + focus + visible slot x; applies an `Edit` delta to `value`.
    /// Returns `(visible_slot_x, focused, changed)`; `visible_slot_x` is `None`
    /// when the column is outside the horizontal scroll window.
    fn column_common(
        &mut self,
        value: &mut f32,
        range: &RangeInclusive<f32>,
    ) -> (Option<i32>, bool, bool) {
        let i = self.idx;
        self.idx += 1;
        self.cols = self.idx as u16;
        let focused = i == self.state.cursor();
        let mut changed = false;

        if focused {
            if let MenuInput::Edit(n) = self.pending {
                let (lo, hi) = (*range.start(), *range.end());
                let step = (hi - lo) / 64.0;
                let nv = (*value + n as f32 * step).clamp(lo, hi);
                if nv != *value {
                    *value = nv;
                    changed = true;
                }
                self.pending = MenuInput::None;
            }
        }

        let scroll = self.state.scroll();
        let max_visible = self.max_visible();
        let slot_x = if i < scroll || i >= scroll + max_visible {
            None
        } else {
            Some((i - scroll) as i32 * self.col_w())
        };
        (slot_x, focused, changed)
    }

    /// Center a param visualization in its column's param area and draw it.
    fn place<P>(&mut self, p: P, slot_x: i32)
    where
        P: Positionable + OriginDimensions,
    {
        let inset = self.style.top_inset;
        let col_w = self.col_w();
        let sz = p.size();
        let x = slot_x + (col_w - sz.width as i32) / 2;
        let y = inset + PARAM_TOP + (PARAM_H - sz.height as i32) / 2;
        let _ = p.with_position(Point::new(x, y)).draw(self.d);
    }

    /// Draw the column label, inverted on a fill when focused.
    fn label(&mut self, slot_x: i32, text: &str, focused: bool) {
        let inset = self.style.top_inset;
        let col_w = self.col_w();
        let cx = slot_x + col_w / 2;
        if focused {
            let _ = Rectangle::new(
                Point::new(slot_x, inset + LABEL_Y - 1),
                Size::new(col_w as u32, LABEL_H as u32),
            )
            .draw_styled(&PrimitiveStyle::with_fill(BinaryColor::On), self.d);
        }
        let color = if focused {
            BinaryColor::Off
        } else {
            BinaryColor::On
        };
        let style = TextStyle::new(self.style.item_font)
            .with_alignment(Alignment::Center)
            .with_color(color);
        let _ = draw_text(self.d, text, Point::new(cx, inset + LABEL_Y), style);
    }

    fn norm_unipolar(value: f32, range: &RangeInclusive<f32>) -> f32 {
        let (lo, hi) = (*range.start(), *range.end());
        if hi == lo {
            0.0
        } else {
            ((value - lo) / (hi - lo)).clamp(0.0, 1.0)
        }
    }

    fn norm_bipolar(value: f32, range: &RangeInclusive<f32>) -> f32 {
        let (lo, hi) = (*range.start(), *range.end());
        if hi == lo {
            0.0
        } else {
            (2.0 * (value - lo) / (hi - lo) - 1.0).clamp(-1.0, 1.0)
        }
    }

    fn bar_size(&self) -> Size {
        Size::new((self.col_w() - 10).max(2) as u32, BAR_H)
    }

    /// Finish the frame: clamp the selected column and slide the column window.
    pub fn end(mut self) {
        let n = self.cols;
        let max_visible = self.max_visible() as u16;
        let cursor = self.state.cursor().min(n.saturating_sub(1) as usize);
        self.state.set_cursor(cursor);
        let cursor = cursor as u16;
        let scroll = self.state.scroll() as u16;
        let new_scroll = if n <= max_visible {
            0
        } else if cursor < scroll {
            cursor
        } else if cursor >= scroll + max_visible {
            cursor + 1 - max_visible
        } else {
            scroll
        };
        self.state.set_scroll(new_scroll as usize);
        self.state.set_rows_last(n);
    }

    // ── Per-control column widgets ──────────────────────────────────────────

    /// A circular knob column (unipolar).
    pub fn knob(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(UnipolarKnob::new(Self::norm_unipolar(*value, &range)), x);
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A vertical slider column (unipolar).
    pub fn slider(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(
                Slider::new(Point::zero(), Self::norm_unipolar(*value, &range)),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A unipolar fill-bar column.
    pub fn unipolar_bar(
        &mut self,
        label: &str,
        value: &mut f32,
        range: RangeInclusive<f32>,
    ) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            let size = self.bar_size();
            self.place(
                UnipolarBar::new(Point::zero(), size, Self::norm_unipolar(*value, &range)),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A bipolar (center-out) fill-bar column.
    pub fn bipolar(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            let size = self.bar_size();
            self.place(
                BipolarBar::new(Point::zero(), size, Self::norm_bipolar(*value, &range)),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A pan column (bipolar L↔R).
    pub fn pan(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(Pan::new(Point::zero(), Self::norm_bipolar(*value, &range)), x);
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A low-pass-filter column (unipolar cutoff).
    pub fn lpf(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(
                LowPassFilter::new(Point::zero(), Self::norm_unipolar(*value, &range)),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A high-pass-filter column (unipolar cutoff).
    pub fn hpf(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(
                HighPassFilter::new(Point::zero(), Self::norm_unipolar(*value, &range)),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A percent-readout column (unipolar).
    pub fn percent(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(
                Percent::new(Point::zero(), Self::norm_unipolar(*value, &range)),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// An attack-envelope column (unipolar).
    pub fn attack(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(Attack::new(Point::zero(), Self::norm_unipolar(*value, &range)), x);
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A release-envelope column (unipolar).
    pub fn release(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(
                Release::new(Point::zero(), Self::norm_unipolar(*value, &range)),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A length-slider column (unipolar).
    pub fn length(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            self.place(
                LengthSlider::new(Point::zero(), Self::norm_unipolar(*value, &range), true),
                x,
            );
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embedded_graphics::primitives::Rectangle as EgRect;

    struct NullTarget;
    impl Dimensions for NullTarget {
        fn bounding_box(&self) -> EgRect {
            EgRect::new(Point::zero(), Size::new(DISPLAY_WIDTH, 48))
        }
    }
    impl DrawTarget for NullTarget {
        type Color = BinaryColor;
        type Error = Infallible;
        fn draw_iter<I>(&mut self, _: I) -> Result<(), Infallible>
        where
            I: IntoIterator<Item = Pixel<BinaryColor>>,
        {
            Ok(())
        }
    }

    #[derive(Default)]
    struct S {
        cut: f32,
        res: f32,
        pan: f32,
        amt: f32,
        a: f32,
        b: f32,
    }

    /// Six columns (more than the 4 visible) to exercise the scroll window.
    fn frame(state: &mut MenuState, s: &mut S, input: MenuInput) {
        let mut d = NullTarget;
        let style = MenuStyle {
            top_inset: 0,
            ..Default::default()
        };
        let mut ui = HMenu::begin(&mut d, state, input, &style);
        ui.title("FILTER");
        ui.lpf("CUT", &mut s.cut, 0.0..=1.0);
        ui.knob("RES", &mut s.res, 0.0..=1.0);
        ui.pan("PAN", &mut s.pan, -1.0..=1.0);
        ui.slider("AMT", &mut s.amt, 0.0..=1.0);
        ui.percent("A", &mut s.a, 0.0..=1.0);
        ui.bipolar("B", &mut s.b, -1.0..=1.0);
        ui.end();
    }

    #[test]
    fn column_moves_and_clamps() {
        let mut st = MenuState::new();
        let mut s = S::default();
        frame(&mut st, &mut s, MenuInput::None); // establishes 6 columns
        frame(&mut st, &mut s, MenuInput::Turn(1));
        assert_eq!(st.cursor(), 1);
        frame(&mut st, &mut s, MenuInput::Turn(10));
        assert_eq!(st.cursor(), 5); // clamped to last column
        frame(&mut st, &mut s, MenuInput::Turn(-100));
        assert_eq!(st.cursor(), 0);
    }

    #[test]
    fn edit_changes_focused_column_and_clamps() {
        let mut st = MenuState::new();
        let mut s = S {
            cut: 0.5,
            ..S::default()
        };
        frame(&mut st, &mut s, MenuInput::None);
        // Focused on CUT (column 0). Edit moves it in 1/64 steps of the range.
        frame(&mut st, &mut s, MenuInput::Edit(8));
        assert!((s.cut - (0.5 + 8.0 / 64.0)).abs() < 1e-6);
        // Move to RES (col 1) and slam it past the top — clamps to 1.0.
        frame(&mut st, &mut s, MenuInput::Turn(1));
        frame(&mut st, &mut s, MenuInput::Edit(1000));
        assert_eq!(s.res, 1.0);
        assert_eq!(st.cursor(), 1);
    }

    #[test]
    fn scroll_window_follows_column() {
        let mut st = MenuState::new();
        let mut s = S::default();
        frame(&mut st, &mut s, MenuInput::None);
        for _ in 0..5 {
            frame(&mut st, &mut s, MenuInput::Turn(1));
        }
        assert_eq!(st.cursor(), 5);
        assert_eq!(st.scroll(), 2); // 4-wide window [2..6] keeps column 5 visible
    }

    #[test]
    fn normalization_maps_ranges() {
        // Unipolar: midpoint of an arbitrary range → 0.5.
        assert!((HMenu::<NullTarget>::norm_unipolar(50.0, &(0.0..=100.0)) - 0.5).abs() < 1e-6);
        // Bipolar: a centered value → 0.0, ends → ∓1.
        assert!((HMenu::<NullTarget>::norm_bipolar(0.0, &(-1.0..=1.0))).abs() < 1e-6);
        assert!((HMenu::<NullTarget>::norm_bipolar(-1.0, &(-1.0..=1.0)) + 1.0).abs() < 1e-6);
        assert!((HMenu::<NullTarget>::norm_bipolar(1.0, &(-1.0..=1.0)) - 1.0).abs() < 1e-6);
    }
}
