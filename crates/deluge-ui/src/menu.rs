//! Immediate-mode menu system for the Deluge OLED.
//!
//! egui / Dear-ImGui style: there is no retained widget tree. Each frame the app
//! feeds one input event, builds the screen with calls like [`Menu::int`] /
//! [`Menu::toggle`] / [`Menu::submenu`], and the bindings are plain `&mut field`
//! references — so the view always reflects the model because it is redrawn from
//! the source of truth every frame.
//!
//! The only retained state is [`MenuState`] (cursor / scroll / drilldown path),
//! which the **app owns**: maker apps let [`MenuInput`] mutate it; a host like
//! spark can drive `cursor`/`path` externally instead.
//!
//! ```ignore
//! let mut ui = Menu::begin(&mut oled, &mut nav, input, &style);
//! ui.title("SOUND");
//! ui.int("FREQ", &mut s.freq, 20..=20000);
//! ui.toggle("MONO", &mut s.mono);
//! ui.enumv("WAVE", &mut s.wave);
//! ui.submenu("ADVANCED", |ui| {
//!     ui.float("DRIVE", &mut s.drive, 0.0..=1.0);
//! });
//! ui.end();
//! ```

use core::fmt::Write as _;
use core::ops::RangeInclusive;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle, StyledDrawable},
};
use heapless::String as HString;

use crate::{
    DISPLAY_WIDTH, icons,
    primitives::Icon,
    text::{Font, TextStyle, draw_text},
};

/// Maximum drilldown depth (submenu nesting).
const MAX_DEPTH: usize = 8;

// Layout constants (matching the Deluge firmware menu), before `top_inset`.
const TITLE_H: i32 = 14;
const ITEM_H: i32 = 9;
const PAD_X: i32 = 3;
const UNDERLINE_Y: i32 = 11;
const SCROLLBAR_W: i32 = 4;
const ICON_MARGIN_R: i32 = 10; // right edge → icon left

/// One input step for a frame, mapped by the caller from `deluge::Event`
/// (or spark's `UIEvent`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuInput {
    /// Nothing happened this frame.
    #[default]
    None,
    /// Select-encoder turned by `n` detents (signed).
    Turn(i32),
    /// Select-encoder / select-button pressed.
    Press,
    /// Back / cancel.
    Back,
}

/// Per-widget result (egui-style).
#[derive(Clone, Copy, Debug, Default)]
pub struct Response {
    /// The bound value was changed this frame.
    pub changed: bool,
    /// This widget is the focused row.
    pub focused: bool,
    /// A submenu was entered this frame.
    pub entered: bool,
}

/// Retained navigation state, owned by the app (≈ 24 bytes, no alloc).
#[derive(Clone, Debug, Default)]
pub struct MenuState {
    cursor: u16,
    scroll: u16,
    path: heapless::Vec<u16, MAX_DEPTH>,
    rows_last: u16,
    editing: bool,
}

impl MenuState {
    /// A fresh state at the root screen.
    pub fn new() -> Self {
        Self::default()
    }

    /// Focused row index on the active screen.
    pub fn cursor(&self) -> usize {
        self.cursor as usize
    }

    /// Set the focused row directly (e.g. host-driven external selection).
    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor as u16;
    }

    /// Current drilldown depth (0 = root).
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// Whether the focused value is in edit mode.
    pub fn is_editing(&self) -> bool {
        self.editing
    }
}

/// Visual style + layout for a menu.
#[derive(Clone, Copy, Debug)]
pub struct MenuStyle {
    /// Font for the screen title.
    pub title_font: Font,
    /// Font for item rows.
    pub item_font: Font,
    /// Pixels to push all content down, to clear the faceplate-hidden top rows.
    /// Default 5 (Deluge 128×48 panel shows the bottom 128×43); set 0 if the
    /// target is already the visible area (e.g. spark's 128×43 `FrameBuf`).
    pub top_inset: i32,
    /// Rows visible at once.
    pub max_visible: usize,
}

impl Default for MenuStyle {
    fn default() -> Self {
        Self {
            title_font: Font::MetricBold9px,
            item_font: Font::FontApple,
            top_inset: 5,
            max_visible: 3,
        }
    }
}

/// An enum that can be cycled through in a menu (`ui.enumv`).
pub trait MenuEnum: Copy + PartialEq {
    /// All selectable variants, in cycle order.
    fn variants() -> &'static [Self]
    where
        Self: Sized;
    /// Display name of this variant.
    fn name(&self) -> &'static str;
}

/// What a row draws on its right side.
enum Right<'a> {
    None,
    Text(&'a str),
    Icon(&'static icons::IconData),
}

/// Immediate-mode menu builder for one frame.
///
/// Construct with [`Menu::begin`], emit widgets, then call [`Menu::end`].
pub struct Menu<'a, D: DrawTarget<Color = BinaryColor>> {
    d: &'a mut D,
    state: &'a mut MenuState,
    style: &'a MenuStyle,
    /// Input still to be applied to the focused widget during the pass
    /// (`Press` while navigating, or `Turn` while editing). Navigation that
    /// needs no widget context (cursor move, back) is applied in `begin`.
    pending: MenuInput,
    /// Snapshot of `state.path.len()` taken at `begin`; path edits this frame
    /// take effect next frame so a drilldown never renders a mixed screen.
    active_depth: usize,
    /// Current walk depth.
    depth: usize,
    /// Sibling index within the current depth (advances for every widget).
    idx: usize,
    /// Number of rows on the active screen this frame.
    active_rows: u16,
    /// Whether a scrollbar is shown (decided from last frame's row count).
    scrollbar: bool,
}

impl<'a, D: DrawTarget<Color = BinaryColor>> Menu<'a, D> {
    /// Begin a frame. Applies navigation input and prepares the scroll window.
    pub fn begin(
        d: &'a mut D,
        state: &'a mut MenuState,
        input: MenuInput,
        style: &'a MenuStyle,
    ) -> Self {
        let max_visible = style.max_visible.max(1);
        let mut pending = input;

        if state.editing {
            // In edit mode, Press/Back leave it; Turn is applied to the value.
            match input {
                MenuInput::Press | MenuInput::Back => {
                    state.editing = false;
                    pending = MenuInput::None;
                }
                _ => {}
            }
        } else {
            match input {
                MenuInput::Back => {
                    if let Some(idx) = state.path.pop() {
                        state.cursor = idx;
                    }
                    pending = MenuInput::None;
                }
                MenuInput::Turn(n) => {
                    // Apply the delta now; the cursor is clamped to the real row
                    // count in `end`, so a turn right after a screen change can't
                    // over-move against a stale count.
                    state.cursor = (state.cursor as i32 + n).max(0) as u16;
                    pending = MenuInput::None;
                }
                // Press is resolved against the focused widget during the pass.
                MenuInput::Press | MenuInput::None => {}
            }
        }

        let scrollbar = state.rows_last as usize > max_visible;
        let active_depth = state.path.len();

        Self {
            d,
            state,
            style,
            pending,
            active_depth,
            depth: 0,
            idx: 0,
            active_rows: 0,
            scrollbar,
        }
    }

    fn content_width(&self) -> i32 {
        if self.scrollbar {
            DISPLAY_WIDTH as i32 - SCROLLBAR_W
        } else {
            DISPLAY_WIDTH as i32
        }
    }

    /// Draw the screen title + underline (only on the active screen).
    pub fn title(&mut self, text: &str) {
        if self.depth == self.active_depth {
            self.draw_header(text);
        }
    }

    fn draw_header(&mut self, text: &str) {
        let inset = self.style.top_inset;
        let style = TextStyle::new(self.style.title_font).with_color(BinaryColor::On);
        let _ = draw_text(self.d, text, Point::new(PAD_X, inset + 1), style);
        let _ = Line::new(
            Point::new(0, inset + UNDERLINE_Y),
            Point::new(DISPLAY_WIDTH as i32 - 1, inset + UNDERLINE_Y),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(self.d);
    }

    /// Advance the sibling counter and, when on the active screen, return this
    /// row's index and whether it is focused. Returns `None` off the active
    /// screen (the widget should then do nothing but the counter still moves so
    /// drilldown path indices stay consistent across depths).
    fn begin_row(&mut self) -> Option<(usize, bool)> {
        let my = self.idx;
        self.idx += 1;
        if self.depth != self.active_depth {
            return None;
        }
        self.active_rows = self.idx as u16;
        Some((my, my == self.state.cursor as usize))
    }

    fn row_visible(&self, i: usize) -> Option<i32> {
        let max_visible = self.style.max_visible.max(1);
        let scroll = self.state.scroll as usize;
        if i < scroll || i >= scroll + max_visible {
            return None;
        }
        Some(self.style.top_inset + TITLE_H + (i - scroll) as i32 * ITEM_H)
    }

    /// Draw one row (label left, optional right element), highlighting if focused.
    fn draw_row(&mut self, i: usize, label: &str, right: Right<'_>, focused: bool) {
        let Some(y) = self.row_visible(i) else {
            return;
        };
        let content_w = self.content_width();

        if focused {
            let _ = Rectangle::new(Point::new(0, y - 1), Size::new(content_w as u32, ITEM_H as u32))
                .draw_styled(&PrimitiveStyle::with_fill(BinaryColor::On), self.d);
        }
        let color = if focused {
            BinaryColor::Off
        } else {
            BinaryColor::On
        };

        let label_style = TextStyle::new(self.style.item_font).with_color(color);
        let _ = draw_text(self.d, label, Point::new(PAD_X, y), label_style);

        match right {
            Right::None => {}
            Right::Text(s) => {
                let style = TextStyle::new(self.style.item_font)
                    .with_alignment(embedded_graphics::text::Alignment::Right)
                    .with_color(color);
                let _ = draw_text(self.d, s, Point::new(content_w - PAD_X, y), style);
            }
            Right::Icon(icon) => {
                let icon_x = content_w - ICON_MARGIN_R;
                let _ = Icon::new(icon, Point::new(icon_x, y))
                    .with_color(color)
                    .draw(self.d);
            }
        }
    }

    /// A plain selectable row. `Response::changed` is set when it is activated.
    pub fn entry(&mut self, label: &str) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused && self.pending == MenuInput::Press {
            resp.changed = true;
            self.pending = MenuInput::None;
        }
        self.draw_row(i, label, Right::None, focused);
        resp
    }

    /// A boolean toggle. Pressing it flips the value.
    pub fn toggle(&mut self, label: &str, value: &mut bool) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused && self.pending == MenuInput::Press {
            *value = !*value;
            resp.changed = true;
            self.pending = MenuInput::None;
        }
        let icon: &'static icons::IconData = if *value {
            &icons::CHECKED_BOX
        } else {
            &icons::UNCHECKED_BOX
        };
        self.draw_row(i, label, Right::Icon(icon), focused);
        resp
    }

    /// An integer value. Press to edit, then turn to change (clamped to `range`).
    pub fn int(&mut self, label: &str, value: &mut i32, range: RangeInclusive<i32>) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused {
            self.edit_press();
            if self.state.editing {
                if let MenuInput::Turn(n) = self.pending {
                    let nv = (*value + n).clamp(*range.start(), *range.end());
                    if nv != *value {
                        *value = nv;
                        resp.changed = true;
                    }
                    self.pending = MenuInput::None;
                }
            }
        }
        let mut buf: HString<12> = HString::new();
        let _ = write!(buf, "{}", *value);
        self.draw_row(i, label, Right::Text(&buf), focused);
        resp
    }

    /// A float value, edited in 1/64 steps of the range.
    pub fn float(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused {
            self.edit_press();
            if self.state.editing {
                if let MenuInput::Turn(n) = self.pending {
                    let step = (*range.end() - *range.start()) / 64.0;
                    let nv = (*value + n as f32 * step).clamp(*range.start(), *range.end());
                    if nv != *value {
                        *value = nv;
                        resp.changed = true;
                    }
                    self.pending = MenuInput::None;
                }
            }
        }
        let mut buf: HString<16> = HString::new();
        let _ = write!(buf, "{:.2}", *value);
        self.draw_row(i, label, Right::Text(&buf), focused);
        resp
    }

    /// An enum value, cycled through [`MenuEnum::variants`] when editing.
    pub fn enumv<E: MenuEnum + 'static>(&mut self, label: &str, value: &mut E) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused {
            self.edit_press();
            if self.state.editing {
                if let MenuInput::Turn(n) = self.pending {
                    let variants = E::variants();
                    if !variants.is_empty() {
                        let cur = variants.iter().position(|v| v == value).unwrap_or(0) as i32;
                        let len = variants.len() as i32;
                        let next = (cur + n).rem_euclid(len) as usize;
                        if variants[next] != *value {
                            *value = variants[next];
                            resp.changed = true;
                        }
                    }
                    self.pending = MenuInput::None;
                }
            }
        }
        self.draw_row(i, label, Right::Text(value.name()), focused);
        resp
    }

    /// A drilldown submenu. `body` is the child screen, run only once the
    /// submenu has been entered (its rows replace the parent's).
    pub fn submenu(&mut self, label: &str, body: impl FnOnce(&mut Self)) -> Response {
        let my = self.idx;
        self.idx += 1;
        let mut resp = Response::default();

        if self.depth == self.active_depth {
            // Drawn as a row on the current screen.
            self.active_rows = self.idx as u16;
            let focused = my == self.state.cursor as usize;
            resp.focused = focused;
            if focused && self.pending == MenuInput::Press {
                // Take effect next frame (active_depth is snapshotted).
                let _ = self.state.path.push(my as u16);
                self.state.cursor = 0;
                self.state.scroll = 0;
                self.state.editing = false;
                self.pending = MenuInput::None;
                resp.entered = true;
            }
            self.draw_row(my, label, Right::Icon(&icons::SUBMENU_ARROW), focused);
        } else if self.depth < self.active_depth
            && self.state.path.get(self.depth).copied() == Some(my as u16)
        {
            // On the path to the active screen: descend and render the child.
            let saved = self.idx;
            self.depth += 1;
            self.idx = 0;
            self.draw_header(label); // child screen is auto-titled with the submenu label
            body(self);
            self.depth -= 1;
            self.idx = saved;
        }
        resp
    }

    /// If the focused value row is pressed, enter edit mode for it.
    fn edit_press(&mut self) {
        if !self.state.editing && self.pending == MenuInput::Press {
            self.state.editing = true;
            self.pending = MenuInput::None;
        }
    }

    /// Finish the frame: draw the scrollbar (matching the rows just drawn), then
    /// clamp the cursor to the real row count and recompute the scroll window for
    /// next frame.
    pub fn end(mut self) {
        let max_visible = self.style.max_visible.max(1);
        let n = self.active_rows;

        // Scrollbar uses the scroll value the rows were drawn with.
        if n as usize > max_visible {
            self.draw_scrollbar();
        }

        // Clamp focus to this frame's actual rows, then slide the window.
        let cursor = self.state.cursor.min(n.saturating_sub(1));
        self.state.cursor = cursor;
        if n as usize <= max_visible {
            self.state.scroll = 0;
        } else if cursor < self.state.scroll {
            self.state.scroll = cursor;
        } else if cursor >= self.state.scroll + max_visible as u16 {
            self.state.scroll = cursor + 1 - max_visible as u16;
        }
        self.state.rows_last = n;
    }

    fn draw_scrollbar(&mut self) {
        let inset = self.style.top_inset;
        let max_visible = self.style.max_visible.max(1);
        let total = self.active_rows as f32;
        let visible = max_visible as f32;
        let track_top = inset + TITLE_H;
        let track_h = max_visible as i32 * ITEM_H;

        let ratio = self.state.scroll as f32 / (total - visible).max(1.0);
        let ind_h = (((visible / total) * track_h as f32) as i32).max(3);
        let ind_y = track_top + ((track_h - ind_h) as f32 * ratio) as i32;

        let x = DISPLAY_WIDTH as i32 - 2;
        let _ = Rectangle::new(Point::new(x - 1, ind_y), Size::new(3, ind_h as u32))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(self.d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embedded_graphics::primitives::Rectangle;

    /// A `DrawTarget` that discards everything — these tests assert state
    /// transitions, not pixels.
    struct NullTarget;

    impl Dimensions for NullTarget {
        fn bounding_box(&self) -> Rectangle {
            Rectangle::new(Point::zero(), Size::new(DISPLAY_WIDTH, 48))
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

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Wave {
        Sine,
        Saw,
        Square,
    }
    impl MenuEnum for Wave {
        fn variants() -> &'static [Self] {
            &[Wave::Sine, Wave::Saw, Wave::Square]
        }
        fn name(&self) -> &'static str {
            match self {
                Wave::Sine => "SINE",
                Wave::Saw => "SAW",
                Wave::Square => "SQUARE",
            }
        }
    }

    #[derive(Default)]
    struct S {
        freq: i32,
        mono: bool,
        wave: Wave,
        drive: f32,
    }
    impl Default for Wave {
        fn default() -> Self {
            Wave::Sine
        }
    }

    /// Run one frame with the standard 4-item menu (incl. a submenu).
    fn frame(state: &mut MenuState, s: &mut S, input: MenuInput) {
        let mut d = NullTarget;
        let style = MenuStyle {
            top_inset: 0,
            ..Default::default()
        };
        let mut ui = Menu::begin(&mut d, state, input, &style);
        ui.title("SOUND");
        ui.int("FREQ", &mut s.freq, 20..=20000);
        ui.toggle("MONO", &mut s.mono);
        ui.enumv("WAVE", &mut s.wave);
        ui.submenu("ADVANCED", |ui| {
            ui.title("ADVANCED");
            ui.float("DRIVE", &mut s.drive, 0.0..=1.0);
        });
        ui.end();
    }

    #[test]
    fn cursor_moves_and_clamps() {
        let mut st = MenuState::new();
        let mut s = S::default();
        // First frame establishes rows_last = 4.
        frame(&mut st, &mut s, MenuInput::None);
        assert_eq!(st.cursor(), 0);
        frame(&mut st, &mut s, MenuInput::Turn(1));
        assert_eq!(st.cursor(), 1);
        frame(&mut st, &mut s, MenuInput::Turn(2));
        assert_eq!(st.cursor(), 3);
        // Clamp at the last row (4 rows → max index 3).
        frame(&mut st, &mut s, MenuInput::Turn(5));
        assert_eq!(st.cursor(), 3);
        frame(&mut st, &mut s, MenuInput::Turn(-10));
        assert_eq!(st.cursor(), 0);
    }

    #[test]
    fn toggle_flips_on_press() {
        let mut st = MenuState::new();
        let mut s = S::default();
        frame(&mut st, &mut s, MenuInput::None); // rows_last
        frame(&mut st, &mut s, MenuInput::Turn(1)); // focus MONO (row 1)
        assert!(!s.mono);
        frame(&mut st, &mut s, MenuInput::Press);
        assert!(s.mono);
        frame(&mut st, &mut s, MenuInput::Press);
        assert!(!s.mono);
    }

    #[test]
    fn int_edits_only_after_press_and_clamps() {
        let mut st = MenuState::new();
        let mut s = S {
            freq: 100,
            ..S::default()
        };
        frame(&mut st, &mut s, MenuInput::None);
        // Focused on FREQ (row 0). Turn before pressing just moves the cursor.
        frame(&mut st, &mut s, MenuInput::Turn(1));
        assert_eq!(s.freq, 100);
        assert_eq!(st.cursor(), 1);
        // Back to row 0 and enter edit mode.
        frame(&mut st, &mut s, MenuInput::Turn(-1));
        frame(&mut st, &mut s, MenuInput::Press);
        assert!(st.is_editing());
        frame(&mut st, &mut s, MenuInput::Turn(50));
        assert_eq!(s.freq, 150);
        // Press again leaves edit mode; turns move the cursor again.
        frame(&mut st, &mut s, MenuInput::Press);
        assert!(!st.is_editing());
        frame(&mut st, &mut s, MenuInput::Turn(1));
        assert_eq!(s.freq, 150);
        assert_eq!(st.cursor(), 1);
    }

    #[test]
    fn enum_cycles_when_editing() {
        let mut st = MenuState::new();
        let mut s = S::default();
        frame(&mut st, &mut s, MenuInput::None);
        frame(&mut st, &mut s, MenuInput::Turn(2)); // focus WAVE (row 2)
        frame(&mut st, &mut s, MenuInput::Press); // edit
        frame(&mut st, &mut s, MenuInput::Turn(1));
        assert_eq!(s.wave, Wave::Saw);
        frame(&mut st, &mut s, MenuInput::Turn(1));
        assert_eq!(s.wave, Wave::Square);
        frame(&mut st, &mut s, MenuInput::Turn(1)); // wraps
        assert_eq!(s.wave, Wave::Sine);
    }

    #[test]
    fn submenu_enter_and_back() {
        let mut st = MenuState::new();
        let mut s = S {
            drive: 0.5,
            ..S::default()
        };
        frame(&mut st, &mut s, MenuInput::None);
        frame(&mut st, &mut s, MenuInput::Turn(3)); // focus ADVANCED (row 3)
        assert_eq!(st.depth(), 0);
        frame(&mut st, &mut s, MenuInput::Press); // enter (takes effect: path pushed)
        assert_eq!(st.depth(), 1);
        assert_eq!(st.cursor(), 0); // child cursor reset
        // Child screen has one row (DRIVE). Edit it.
        frame(&mut st, &mut s, MenuInput::Press);
        frame(&mut st, &mut s, MenuInput::Turn(1));
        assert!(s.drive > 0.5);
        // Exit edit, then Back leaves the submenu.
        frame(&mut st, &mut s, MenuInput::Press);
        frame(&mut st, &mut s, MenuInput::Back);
        assert_eq!(st.depth(), 0);
        assert_eq!(st.cursor(), 3); // focus returns to the submenu row
    }

    #[test]
    fn child_screen_rows_only() {
        // While in the submenu, only the child's rows are counted (1), so the
        // cursor cannot land on a parent row.
        let mut st = MenuState::new();
        let mut s = S::default();
        frame(&mut st, &mut s, MenuInput::None);
        frame(&mut st, &mut s, MenuInput::Turn(3));
        frame(&mut st, &mut s, MenuInput::Press); // enter ADVANCED
        frame(&mut st, &mut s, MenuInput::Turn(5)); // try to move down
        assert_eq!(st.cursor(), 0); // only 1 child row → clamped to 0
    }

    #[test]
    fn scroll_follows_cursor() {
        // A 6-item menu (no submenu) to exercise the 3-row window.
        fn frame6(state: &mut MenuState, input: MenuInput) {
            let mut d = NullTarget;
            let style = MenuStyle {
                top_inset: 0,
                ..Default::default()
            };
            let mut ui = Menu::begin(&mut d, state, input, &style);
            ui.title("LIST");
            for i in 0..6 {
                let _ = ui.entry(["A", "B", "C", "D", "E", "F"][i]);
            }
            ui.end();
        }
        let mut st = MenuState::new();
        frame6(&mut st, MenuInput::None);
        for _ in 0..4 {
            frame6(&mut st, MenuInput::Turn(1));
        }
        assert_eq!(st.cursor(), 4);
        assert_eq!(st.scroll, 2); // window [2..5] keeps row 4 visible
    }
}
