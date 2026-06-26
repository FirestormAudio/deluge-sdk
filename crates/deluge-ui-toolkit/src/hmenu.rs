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

use crate::menu::{MenuInput, MenuState, MenuStyle, Response, apply_turn};
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
const UNDERLINE_Y: i32 = 11;
const LABEL_Y: i32 = 34;
const LABEL_H: i32 = 9;
const BAR_H: u32 = 7;

/// A pixel-discarding [`DrawTarget`] that records the vertical extent (top and
/// bottom lit rows) of what a widget draws.
///
/// Lets `place` center a widget by its *actual ink*, not its
/// [`OriginDimensions::size`] — whose bounding box often includes blank padding
/// around the drawn content (a knob arc, the pan cylinder), which would
/// otherwise leave the visible pixels off-centre.
struct InkBounds {
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    any: bool,
}

impl InkBounds {
    /// Ink extent `(min_x, max_x, min_y, max_y)` of `p` (drawn at its current
    /// origin), or `None` if it draws nothing.
    fn of<P: Drawable<Color = BinaryColor>>(p: &P) -> Option<(i32, i32, i32, i32)> {
        let mut b = InkBounds {
            min_x: i32::MAX,
            max_x: i32::MIN,
            min_y: i32::MAX,
            max_y: i32::MIN,
            any: false,
        };
        let _ = p.draw(&mut b);
        b.any.then_some((b.min_x, b.max_x, b.min_y, b.max_y))
    }
}

impl Dimensions for InkBounds {
    fn bounding_box(&self) -> Rectangle {
        // Large enough that no column widget clips while being measured.
        Rectangle::new(Point::new(0, 0), Size::new(1024, 1024))
    }
}

impl DrawTarget for InkBounds {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            if c == BinaryColor::On {
                self.any = true;
                self.min_x = self.min_x.min(p.x);
                self.max_x = self.max_x.max(p.x);
                self.min_y = self.min_y.min(p.y);
                self.max_y = self.max_y.max(p.y);
            }
        }
        Ok(())
    }
}

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
                // Clamp to the current column count now (not only in `end`) so a
                // turn past the last column doesn't move the selection off-screen
                // for a frame and then skip to the second-to-last on reverse.
                let cursor = apply_turn(state.cursor() as u16, n, state.rows_last());
                state.set_cursor(cursor as usize);
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

    /// Advance to the next column: bump the column index/count, report whether it
    /// is focused, and give its visible slot x (`None` when outside the horizontal
    /// scroll window). The layout backbone shared by every column kind — value
    /// widgets, [`text_value`](Self::text_value), and [`placeholder`](Self::placeholder).
    fn next_slot(&mut self) -> (Option<i32>, bool) {
        let i = self.idx;
        self.idx += 1;
        self.cols = self.idx as u16;
        let focused = i == self.state.cursor();
        let scroll = self.state.scroll();
        let max_visible = self.max_visible();
        let slot_x = if i < scroll || i >= scroll + max_visible {
            None
        } else {
            Some((i - scroll) as i32 * self.col_w())
        };
        (slot_x, focused)
    }

    /// Column index + focus + visible slot x; applies an `Edit` delta to `value`.
    /// Returns `(visible_slot_x, focused, changed)`; `visible_slot_x` is `None`
    /// when the column is outside the horizontal scroll window.
    fn column_common(
        &mut self,
        value: &mut f32,
        range: &RangeInclusive<f32>,
    ) -> (Option<i32>, bool, bool) {
        let (slot_x, focused) = self.next_slot();
        let mut changed = false;

        if focused && let MenuInput::Edit(n) = self.pending {
            let (lo, hi) = (*range.start(), *range.end());
            let step = (hi - lo) / 64.0;
            let nv = (*value + n as f32 * step).clamp(lo, hi);
            if nv != *value {
                *value = nv;
                changed = true;
            }
            self.pending = MenuInput::None;
        }

        (slot_x, focused, changed)
    }

    /// Place a value-driven param visualization in its column, centred on
    /// **both** axes by its *actual drawn pixels*: horizontally within the column
    /// slot, vertically within the band between the underline and the label.
    ///
    /// `make(t)` builds the widget at a normalised value `t`; `live` is the value
    /// to draw, and `[lo, hi]` are its extremes. Centring by
    /// [`OriginDimensions::size`] doesn't work — the bounding boxes include blank
    /// padding (the knob arc, the pan cylinder). But centring by the *live* ink
    /// doesn't either: the value-dependent part (knob pointer, fill, …) changes
    /// the ink each frame, so the centre would drift as the value changes.
    /// Instead we centre by the **union of the ink at both extremes**, which
    /// bounds the full range of motion and is identical every frame — so the
    /// widget stays put while only its indicator moves.
    fn place<P>(&mut self, slot_x: i32, live: f32, lo: f32, hi: f32, make: impl Fn(f32) -> P)
    where
        P: Positionable + OriginDimensions + Drawable<Color = BinaryColor>,
    {
        let inset = self.style.top_inset;
        let col_w = self.col_w();

        // Centres to align the ink to: the column's horizontal mid-point, and the
        // mid-point of the vertical band between the underline and the label.
        let col_center = slot_x + col_w / 2;
        let band_center = (inset + UNDERLINE_Y + 1 + inset + LABEL_Y - 2) / 2;

        // Value-independent layout: union the ink at both value extremes.
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        for t in [lo, hi] {
            let probe = make(t).with_position(Point::new(0, 0));
            if let Some((a, b, c, d)) = InkBounds::of(&probe) {
                min_x = min_x.min(a);
                max_x = max_x.max(b);
                min_y = min_y.min(c);
                max_y = max_y.max(d);
            }
        }
        let widget = make(live);
        if min_x > max_x {
            // Both extremes drew nothing — fall back to the bounding box.
            let sz = widget.size();
            min_x = 0;
            max_x = sz.width as i32 - 1;
            min_y = 0;
            max_y = sz.height as i32 - 1;
        }
        let x = col_center - (min_x + max_x) / 2;
        let y = band_center - (min_y + max_y) / 2;
        let _ = widget.with_position(Point::new(x, y)).draw(self.d);
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
    pub fn end(self) {
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
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, UnipolarKnob::new);
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
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| Slider::new(Point::zero(), t));
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
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| {
                UnipolarBar::new(Point::zero(), size, t)
            });
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A bipolar (center-out) fill-bar column.
    pub fn bipolar(
        &mut self,
        label: &str,
        value: &mut f32,
        range: RangeInclusive<f32>,
    ) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            let size = self.bar_size();
            let live = Self::norm_bipolar(*value, &range);
            self.place(x, live, -1.0, 1.0, |t| {
                BipolarBar::new(Point::zero(), size, t)
            });
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
            let live = Self::norm_bipolar(*value, &range);
            self.place(x, live, -1.0, 1.0, |t| Pan::new(Point::zero(), t));
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
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| LowPassFilter::new(Point::zero(), t));
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
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| HighPassFilter::new(Point::zero(), t));
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A percent-readout column (unipolar).
    pub fn percent(
        &mut self,
        label: &str,
        value: &mut f32,
        range: RangeInclusive<f32>,
    ) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| Percent::new(Point::zero(), t));
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
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| Attack::new(Point::zero(), t));
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A release-envelope column (unipolar).
    pub fn release(
        &mut self,
        label: &str,
        value: &mut f32,
        range: RangeInclusive<f32>,
    ) -> Response {
        let (slot, focused, changed) = self.column_common(value, &range);
        if let Some(x) = slot {
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| Release::new(Point::zero(), t));
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
            let live = Self::norm_unipolar(*value, &range);
            self.place(x, live, 0.0, 1.0, |t| {
                LengthSlider::new(Point::zero(), t, true)
            });
            self.label(x, label, focused);
        }
        Response {
            changed,
            focused,
            entered: false,
        }
    }

    /// A text / enum read-out column: `text` centred in the widget band with the
    /// `label` below it.
    ///
    /// Unlike the value widgets this has **no numeric binding** — it does not
    /// consume `Edit`. It is for params whose value is a word (an enum variant, an
    /// on/off toggle) rendered by a caller that owns the value and its cycling
    /// (e.g. spark's menu engine driving a CLAP enum). It still takes part in
    /// column layout and selection, so it highlights when focused like any column.
    pub fn text_value(&mut self, label: &str, text: &str) -> Response {
        let (slot, focused) = self.next_slot();
        if let Some(x) = slot {
            let inset = self.style.top_inset;
            let cx = x + self.col_w() / 2;
            let band_center = (inset + UNDERLINE_Y + 1 + inset + LABEL_Y - 2) / 2;
            let h = self.style.item_font.height() as i32;
            let style = TextStyle::new(self.style.item_font)
                .with_alignment(Alignment::Center)
                .with_color(BinaryColor::On);
            let _ = draw_text(self.d, text, Point::new(cx, band_center - h / 2), style);
            self.label(x, label, focused);
        }
        Response {
            changed: false,
            focused,
            entered: false,
        }
    }

    /// An empty-slot column: a dotted box marking an unused param slot (matching
    /// the Deluge firmware's placeholder), occupying a column so neighbouring
    /// columns lay out correctly. It carries no label and is not meant to be the
    /// focused column.
    pub fn placeholder(&mut self) {
        let (slot, _focused) = self.next_slot();
        if let Some(x) = slot {
            self.draw_placeholder_box(x);
        }
    }

    /// Dotted rectangle outline centred in the column's widget band.
    fn draw_placeholder_box(&mut self, slot_x: i32) {
        const DOT: i32 = 5;
        let inset = self.style.top_inset;
        let start_x = slot_x + 7;
        let end_x = start_x + 17;
        let start_y = inset + UNDERLINE_Y + 2; // just below the underline
        let end_y = inset + LABEL_Y + 2; // a little past the label row

        let mut x = start_x + 1;
        while x < end_x {
            let _ = Pixel(Point::new(x, start_y), BinaryColor::On).draw(self.d);
            let _ = Pixel(Point::new(x, end_y), BinaryColor::On).draw(self.d);
            x += DOT;
        }
        let mut y = start_y + 3;
        while y < end_y {
            let _ = Pixel(Point::new(start_x - 2, y), BinaryColor::On).draw(self.d);
            let _ = Pixel(Point::new(end_x + 2, y), BinaryColor::On).draw(self.d);
            y += DOT;
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
    fn text_and_placeholder_columns_take_slots() {
        // A page mixing a value column, a text/enum column, and trailing empties:
        // every column kind must advance the layout so selection lands correctly.
        let mut st = MenuState::new();
        let mut cut = 0.5;
        let mut render = |st: &mut MenuState, input: MenuInput| {
            let mut d = NullTarget;
            let style = MenuStyle {
                top_inset: 0,
                ..Default::default()
            };
            let mut ui = HMenu::begin(&mut d, st, input, &style);
            ui.title("OSC");
            let r0 = ui.knob("LEV", &mut cut, 0.0..=1.0);
            let r1 = ui.text_value("WAVE", "SAW");
            ui.placeholder();
            ui.placeholder();
            ui.end();
            (r0, r1)
        };

        // 4 columns established (2 real + 2 placeholders).
        let (_r0, _r1) = render(&mut st, MenuInput::None);
        assert_eq!(st.rows_last(), 4);

        // Focus starts on column 0 (the knob); the text column is not focused.
        let (r0, r1) = render(&mut st, MenuInput::None);
        assert!(r0.focused);
        assert!(!r1.focused);

        // Turn moves focus onto the text/enum column; it does not consume Edit.
        let (r0, r1) = render(&mut st, MenuInput::Turn(1));
        assert!(!r0.focused);
        assert!(r1.focused);
        assert!(!r1.changed);
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
