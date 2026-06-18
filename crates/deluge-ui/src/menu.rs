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
//! Menu::show(&mut oled, &mut nav, input, &style, |ui| {
//!     ui.title("SOUND");
//!     ui.int("FREQ", &mut s.freq, 20..=20000);
//!     ui.toggle("MONO", &mut s.mono);
//!     ui.enumv("WAVE", &mut s.wave);
//!     ui.submenu("ADVANCED", |ui| {
//!         ui.float("DRIVE", &mut s.drive, 0.0..=1.0);
//!     });
//! });
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
    DISPLAY_WIDTH,
    components::Scrollbar,
    editors::{BipolarValueEditor, UnipolarValueEditor},
    icons,
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
    /// Move the selection by `n` detents (vertical: row; horizontal: column).
    Turn(i32),
    /// Change the focused value directly by `n` detents — no edit mode. Lets a
    /// second control (a gold knob, or the select encoder in an [`HMenu`]) edit
    /// the focused parameter while [`Turn`](MenuInput::Turn) moves the selection.
    Edit(i32),
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
    /// Set when a frame changed which screen renders next (entered a submenu or
    /// an editor); the app should redraw once more before waiting for input.
    redraw: bool,
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

    /// Index of the first visible row/column.
    pub fn scroll(&self) -> usize {
        self.scroll as usize
    }

    pub(crate) fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll as u16;
    }

    pub(crate) fn set_rows_last(&mut self, n: u16) {
        self.rows_last = n;
    }

    /// Row/column count drawn last frame (0 before the first draw).
    pub(crate) fn rows_last(&self) -> u16 {
        self.rows_last
    }

    /// Current drilldown depth (0 = root).
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// Whether the focused value is in edit mode.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// Whether the last frame changed which screen renders next (entered a
    /// submenu or opened a value editor). When `true`, the app should rebuild
    /// and flush one more frame *before* blocking for input — otherwise the new
    /// screen doesn't appear until the next event. Cleared at the start of the
    /// next [`Menu::show`]. See the `oled_menu` example loop.
    pub fn needs_redraw(&self) -> bool {
        self.redraw
    }
}

/// Apply a navigation delta to a cursor and clamp it to the current screen.
///
/// `rows_last` is the row/column count drawn on the previous frame; it is never
/// stale when a `Turn` arrives because the only screen changes (`Press`/`Back`)
/// consume their input and don't also apply a turn. Clamping the upper bound
/// here (not only in `end`) keeps a turn past the last row from moving the
/// cursor off-screen for a frame and then skipping to the second-to-last row on
/// reverse. Before the first draw (`rows_last == 0`) only the lower bound applies;
/// `end` re-clamps against the real count as the safety net.
pub(crate) fn apply_turn(cursor: u16, n: i32, rows_last: u16) -> u16 {
    let moved = (cursor as i32 + n).max(0) as u16;
    if rows_last > 0 {
        moved.min(rows_last - 1)
    } else {
        moved
    }
}

/// The first-visible-row index that keeps `cursor` inside a `max_visible`-row
/// window of `n` rows, nudging the previous `scroll` the minimum needed.
fn scroll_window(cursor: u16, scroll: u16, n: u16, max_visible: u16) -> u16 {
    let mv = max_visible.max(1);
    if n <= mv {
        0
    } else if cursor < scroll {
        cursor
    } else if cursor >= scroll + mv {
        cursor + 1 - mv
    } else {
        scroll.min(n - mv)
    }
}

/// Scratch buffer for a formatted value + suffix (e.g. `"20000 Hz"`).
type ValBuf = HString<24>;

/// Append `" {suffix}"` to a value string (no-op for an empty suffix). Truncated
/// silently if the buffer is full — units are short, the buffer is generous.
fn append_suffix(buf: &mut ValBuf, suffix: &str) {
    if !suffix.is_empty() {
        let _ = buf.push(' ');
        let _ = buf.push_str(suffix);
    }
}

/// Draw `value` centred horizontally **by the value text alone**, with the unit
/// `suffix` (if any) in a smaller font flush against the value's right edge (no
/// gap), bottom-aligned to the value — so the suffix never shifts the value off
/// centre. Shared by every value editor (big readout, bars, enums).
fn draw_centered_value<D>(d: &mut D, value: &str, suffix: &str, value_font: Font, y: i32)
where
    D: DrawTarget<Color = BinaryColor>,
{
    let value_w = value_font.deluge_font().text_width(value);
    let value_left = DISPLAY_WIDTH as i32 / 2 - value_w / 2;
    let _ = draw_text(
        d,
        value,
        Point::new(value_left, y),
        TextStyle::new(value_font),
    );
    if !suffix.is_empty() {
        let sf = Font::MetricBold9px;
        let sy = y + value_font.height() as i32 - sf.height() as i32;
        let _ = draw_text(
            d,
            suffix,
            Point::new(value_left + value_w, sy),
            TextStyle::new(sf),
        );
    }
}

/// Pick the editor bar for a numeric value over `lo..=hi`: bipolar (centre-zero)
/// when the range straddles zero, otherwise a unipolar fill.
fn value_bar(value: f32, lo: f32, hi: f32) -> EditorBar {
    let span = (hi - lo).abs().max(f32::EPSILON);
    let frac = ((value - lo) / span).clamp(0.0, 1.0);
    if lo < 0.0 && hi > 0.0 {
        EditorBar::Bipolar {
            value: frac,
            zero: ((0.0 - lo) / span).clamp(0.0, 1.0),
        }
    } else {
        EditorBar::Unipolar(frac)
    }
}

/// Visual style + layout for a menu.
#[derive(Clone, Copy, Debug)]
pub struct MenuStyle {
    /// Font for the screen title.
    pub title_font: Font,
    /// Font for item rows.
    pub item_font: Font,
    /// Pixels to push all content down, to clear any faceplate-hidden top rows.
    /// Defaults to **0** (this crate is display-agnostic). On the Deluge SDK set
    /// it to `deluge::Oled::VISIBLE_TOP` (= 5) so content lands in the visible
    /// `128×43`; leave it 0 when the target is already the visible area (e.g.
    /// spark's `128×43` `FrameBuf`).
    pub top_inset: i32,
    /// Rows visible at once.
    pub max_visible: usize,
}

impl Default for MenuStyle {
    fn default() -> Self {
        Self {
            title_font: Font::MetricBold9px,
            item_font: Font::FontApple,
            top_inset: 0,
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

/// The bar a full-screen value editor draws under the value, by value kind.
enum EditorBar {
    /// No bar, value drawn at the normal editor size (e.g. an enum label).
    Text,
    /// Fill bar from the left; `0.0..=1.0` of the range.
    Unipolar(f32),
    /// Centre-anchored bar; `value`/`zero` are fractions of the range `0.0..=1.0`.
    Bipolar { value: f32, zero: f32 },
}

/// Immediate-mode menu builder for one frame.
///
/// You don't construct this directly: [`Menu::show`] hands a `&mut Menu` to your
/// build closure (twice — a count pass then a draw pass) and you emit widgets on
/// it (`title`, `int`, `submenu`, …).
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
    /// Count pass (no input applied, nothing drawn) vs. draw pass.
    counting: bool,
    /// Whether a scrollbar gutter is reserved this frame (set from the count
    /// pass's row count, so it is correct on the screen's first frame).
    scrollbar: bool,
    /// Whether this frame renders the focused value's full-screen editor
    /// (snapshot of `state.editing` at `begin`, so a mid-pass `edit_press`
    /// doesn't retroactively blank the rows on the frame that opened it).
    editing_active: bool,
}

impl<'a, D: DrawTarget<Color = BinaryColor>> Menu<'a, D> {
    /// Build and render one frame of the menu.
    ///
    /// `build` is invoked **twice**: first a *count pass* that walks the widgets
    /// drawing nothing, so the row count is known, then a *draw pass*. Knowing
    /// the count up front lets the scrollbar gutter, scroll window, and focus
    /// clamp all be correct on the very first frame of any screen — no lag, no
    /// follow-up redraw. Navigation input and `Response` changes are applied only
    /// on the draw pass, so `build` must be free of side effects except through
    /// the menu (the usual `&mut field` bindings are fine).
    ///
    /// ```ignore
    /// Menu::show(&mut oled, &mut nav, input, &style, |ui| {
    ///     ui.title("SOUND");
    ///     ui.int("FREQ", &mut app.freq, 20..=20000);
    ///     ui.submenu("ADVANCED", |ui| ui.float("DRIVE", &mut app.drive, 0.0..=1.0));
    /// });
    /// ```
    pub fn show<F>(
        d: &'a mut D,
        state: &'a mut MenuState,
        input: MenuInput,
        style: &'a MenuStyle,
        mut build: F,
    ) where
        F: FnMut(&mut Menu<'a, D>),
    {
        let max_visible = style.max_visible.max(1) as u16;

        // A new frame: clear last frame's transition request. `submenu` /
        // `edit_press` re-set it if this frame opens a new screen.
        state.redraw = false;

        // Resolve navigation that doesn't need the row count.
        let mut pending = input;
        if state.editing {
            if matches!(input, MenuInput::Press | MenuInput::Back) {
                state.editing = false;
                pending = MenuInput::None;
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
                    // Raw move; the upper clamp + scroll happen after the count.
                    state.cursor = (state.cursor as i32 + n).max(0) as u16;
                    pending = MenuInput::None;
                }
                MenuInput::Press | MenuInput::Edit(_) | MenuInput::None => {}
            }
        }

        let active_depth = state.path.len();
        let editing_active = state.editing;

        let mut ui = Menu {
            d,
            state,
            style,
            pending: MenuInput::None, // the count pass applies no input
            active_depth,
            depth: 0,
            idx: 0,
            active_rows: 0,
            counting: true,
            scrollbar: false,
            editing_active,
        };

        // ── Count pass: walk the widgets, drawing nothing, to learn the count.
        build(&mut ui);
        let n = ui.active_rows;

        // The count is known: clamp focus, slide the window, decide the gutter.
        ui.state.cursor = ui.state.cursor.min(n.saturating_sub(1));
        ui.state.scroll = scroll_window(ui.state.cursor, ui.state.scroll, n, max_visible);
        ui.scrollbar = n > max_visible;

        // ── Draw pass: re-walk, applying input and drawing at the right width.
        ui.counting = false;
        ui.pending = pending;
        ui.depth = 0;
        ui.idx = 0;
        ui.active_rows = 0;
        build(&mut ui);

        if !ui.editing_active && ui.active_rows > max_visible {
            ui.draw_scrollbar();
        }
        ui.state.rows_last = ui.active_rows;
    }

    fn content_width(&self) -> i32 {
        // Reserve the scrollbar gutter only when this screen actually scrolls.
        // The count pass establishes `scrollbar` before the draw pass, so the
        // width is correct on the screen's first frame.
        if self.scrollbar {
            DISPLAY_WIDTH as i32 - SCROLLBAR_W
        } else {
            DISPLAY_WIDTH as i32
        }
    }

    /// Draw the screen title + underline (only on the active screen).
    pub fn title(&mut self, text: &str) {
        // While a value editor is open it draws its own header (the parameter
        // label), so suppress the list title.
        if self.editing_active {
            return;
        }
        if self.depth == self.active_depth {
            self.draw_header(text);
        }
    }

    fn draw_header(&mut self, text: &str) {
        if self.counting {
            return;
        }
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
        // Nothing is drawn on the count pass; while a value editor is open it owns
        // the whole screen, so the list rows are hidden too.
        if self.counting || self.editing_active {
            return;
        }
        let Some(y) = self.row_visible(i) else {
            return;
        };
        let content_w = self.content_width();

        if focused {
            let _ = Rectangle::new(
                Point::new(0, y - 1),
                Size::new(content_w as u32, ITEM_H as u32),
            )
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

    /// Draw the focused value's full-screen editor: the parameter `label` as the
    /// header, the `value` centred (by the value alone), the unit `suffix` tucked
    /// small against the value's right edge, and the optional `bar` below.
    ///
    /// The value uses the big font when there's no bar (no-range values and
    /// enums), and the standard editor font when a bar is shown.
    fn draw_value_editor(&mut self, label: &str, value: &str, suffix: &str, bar: EditorBar) {
        if self.counting {
            return;
        }
        use embedded_graphics::draw_target::DrawTargetExt;
        // Header (parameter name + separator), aligned with the list's header.
        self.draw_header(label);
        // The editor widgets draw at fixed coordinates for a top-anchored visible
        // area; shift everything down by `top_inset` to clear hidden rows.
        let mut t = self.d.translated(Point::new(0, self.style.top_inset));

        // The bar (drawn with empty text so the widget renders only its bar; the
        // value text is drawn uniformly below for every editor kind).
        let (value_font, value_y) = match bar {
            EditorBar::Text => (Font::MetricBold20px, 16),
            EditorBar::Unipolar(v) => {
                let _ = UnipolarValueEditor::new("", v).draw(&mut t);
                (Font::MetricBold13px, 15)
            }
            EditorBar::Bipolar { value: vf, zero } => {
                let _ = BipolarValueEditor::new("", vf, zero).draw(&mut t);
                (Font::MetricBold13px, 15)
            }
        };
        draw_centered_value(&mut t, value, suffix, value_font, value_y);
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
        if focused {
            let flip = match self.pending {
                MenuInput::Press => true,
                MenuInput::Edit(n) => n != 0,
                _ => false,
            };
            if flip {
                *value = !*value;
                resp.changed = true;
                self.pending = MenuInput::None;
            }
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
        self.int_impl(label, value, range, "")
    }

    /// An integer value with a unit suffix, e.g. `int_unit("FREQ", &mut f,
    /// 20..=20000, "Hz")` shows `440 Hz`. The suffix appears on both the list row
    /// and the editor; a space is inserted before it.
    pub fn int_unit(
        &mut self,
        label: &str,
        value: &mut i32,
        range: RangeInclusive<i32>,
        suffix: &str,
    ) -> Response {
        self.int_impl(label, value, range, suffix)
    }

    fn int_impl(
        &mut self,
        label: &str,
        value: &mut i32,
        range: RangeInclusive<i32>,
        suffix: &str,
    ) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused {
            self.edit_press();
            if let Some(n) = self.take_value_delta() {
                let nv = (*value + n).clamp(*range.start(), *range.end());
                if nv != *value {
                    *value = nv;
                    resp.changed = true;
                }
            }
        }
        let mut val: ValBuf = ValBuf::new();
        let _ = write!(val, "{}", *value);
        if self.editing_active && focused {
            let bar = value_bar(*value as f32, *range.start() as f32, *range.end() as f32);
            self.draw_value_editor(label, &val, suffix, bar);
        } else {
            append_suffix(&mut val, suffix);
            self.draw_row(i, label, Right::Text(&val), focused);
        }
        resp
    }

    /// A float value, edited in 1/64 steps of the range, shown to 2 decimals.
    pub fn float(&mut self, label: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        self.float_impl(label, value, range, 2, "")
    }

    /// A float value with a unit suffix and explicit decimal precision, e.g.
    /// `float_unit("DRIVE", &mut d, 0.0..=1.0, "dB", 1)` shows `3.0 dB`. The
    /// suffix appears on both the list row and the editor.
    pub fn float_unit(
        &mut self,
        label: &str,
        value: &mut f32,
        range: RangeInclusive<f32>,
        suffix: &str,
        precision: usize,
    ) -> Response {
        self.float_impl(label, value, range, precision, suffix)
    }

    fn float_impl(
        &mut self,
        label: &str,
        value: &mut f32,
        range: RangeInclusive<f32>,
        precision: usize,
        suffix: &str,
    ) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused {
            self.edit_press();
            if let Some(n) = self.take_value_delta() {
                let step = (*range.end() - *range.start()) / 64.0;
                let nv = (*value + n as f32 * step).clamp(*range.start(), *range.end());
                if nv != *value {
                    *value = nv;
                    resp.changed = true;
                }
            }
        }
        let mut val: ValBuf = ValBuf::new();
        let _ = write!(val, "{:.*}", precision, *value);
        if self.editing_active && focused {
            let bar = value_bar(*value, *range.start(), *range.end());
            self.draw_value_editor(label, &val, suffix, bar);
        } else {
            append_suffix(&mut val, suffix);
            self.draw_row(i, label, Right::Text(&val), focused);
        }
        resp
    }

    /// An integer value with **no range**, e.g. `int_value("TEMPO", &mut bpm,
    /// "BPM")` shows `120 BPM`. With no range to scale against, the editor shows
    /// no bar and draws the value extra-large. Press to edit, then turn: each
    /// detent steps by 1 (unclamped).
    pub fn int_value(&mut self, label: &str, value: &mut i32, suffix: &str) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused {
            self.edit_press();
            if let Some(n) = self.take_value_delta()
                && n != 0
            {
                *value += n;
                resp.changed = true;
            }
        }
        let mut val: ValBuf = ValBuf::new();
        let _ = write!(val, "{}", *value);
        if self.editing_active && focused {
            self.draw_value_editor(label, &val, suffix, EditorBar::Text);
        } else {
            append_suffix(&mut val, suffix);
            self.draw_row(i, label, Right::Text(&val), focused);
        }
        resp
    }

    /// A float value with **no range**, shown to `precision` decimals with a
    /// unit suffix, e.g. `float_value("GAIN", &mut g, "dB", 1)`. No bar; the
    /// value is drawn extra-large. Each edit detent steps by `10^-precision`
    /// (unclamped), so the step matches the displayed resolution.
    pub fn float_value(
        &mut self,
        label: &str,
        value: &mut f32,
        suffix: &str,
        precision: usize,
    ) -> Response {
        let mut resp = Response::default();
        let Some((i, focused)) = self.begin_row() else {
            return resp;
        };
        resp.focused = focused;
        if focused {
            self.edit_press();
            if let Some(n) = self.take_value_delta()
                && n != 0
            {
                // Step = 10^-precision, without needing std float ops.
                let mut step = 1.0f32;
                for _ in 0..precision {
                    step *= 0.1;
                }
                *value += n as f32 * step;
                resp.changed = true;
            }
        }
        let mut val: ValBuf = ValBuf::new();
        let _ = write!(val, "{:.*}", precision, *value);
        if self.editing_active && focused {
            self.draw_value_editor(label, &val, suffix, EditorBar::Text);
        } else {
            append_suffix(&mut val, suffix);
            self.draw_row(i, label, Right::Text(&val), focused);
        }
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
            if let Some(n) = self.take_value_delta() {
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
            }
        }
        if self.editing_active && focused {
            self.draw_value_editor(label, value.name(), "", EditorBar::Text);
        } else {
            self.draw_row(i, label, Right::Text(value.name()), focused);
        }
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
                // Take effect next frame (active_depth is snapshotted), so ask
                // the app to redraw the child screen without waiting for input.
                let _ = self.state.path.push(my as u16);
                self.state.cursor = 0;
                self.state.scroll = 0;
                self.state.editing = false;
                self.state.redraw = true;
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
            // Child screen is auto-titled with the submenu label — unless a value
            // editor is open, which draws its own header (the parameter label).
            if !self.editing_active {
                self.draw_header(label);
            }
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
            // The editor renders next frame (this frame still drew the list);
            // ask the app to redraw without waiting for input.
            self.state.redraw = true;
        }
    }

    /// Detents to apply to the focused value this frame (consuming the input):
    /// `Edit(n)` always, or `Turn(n)` while in edit mode.
    fn take_value_delta(&mut self) -> Option<i32> {
        let delta = match self.pending {
            MenuInput::Edit(n) => Some(n),
            MenuInput::Turn(n) if self.state.editing => Some(n),
            _ => None,
        };
        if delta.is_some() {
            self.pending = MenuInput::None;
        }
        delta
    }

    fn draw_scrollbar(&mut self) {
        let inset = self.style.top_inset;
        let max_visible = self.style.max_visible.max(1);
        let _ = Scrollbar::new(
            DISPLAY_WIDTH as i32 - 2,
            inset + TITLE_H,
            max_visible as i32 * ITEM_H,
            self.active_rows,
            max_visible as u16,
            self.state.scroll,
        )
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
        Menu::show(&mut d, state, input, &style, |ui| {
            ui.title("SOUND");
            ui.int("FREQ", &mut s.freq, 20..=20000);
            ui.toggle("MONO", &mut s.mono);
            ui.enumv("WAVE", &mut s.wave);
            ui.submenu("ADVANCED", |ui| {
                ui.title("ADVANCED");
                ui.float("DRIVE", &mut s.drive, 0.0..=1.0);
            });
        });
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
    fn turn_clamps_to_last_row_in_begin() {
        // Regression: a turn past the last row used to move the cursor
        // off-screen for a frame (clamped only in `end`), so the selector
        // vanished and then reappeared on the second-to-last entry on reverse.
        // With the row count known from last frame, `begin` pins it to the last
        // row so it stays visible.
        assert_eq!(apply_turn(2, 5, 4), 3); // overshoot pinned to last row, not 7
        assert_eq!(apply_turn(3, 1, 4), 3); // holding past the end stays put
        assert_eq!(apply_turn(3, -1, 4), 2); // reverse from a shown last row → 2
        assert_eq!(apply_turn(0, -3, 4), 0); // clamp at the top
        assert_eq!(apply_turn(0, 9, 0), 9); // before the first draw: no upper clamp
    }

    #[test]
    fn suffix_appended_with_space() {
        let mut b = ValBuf::new();
        let _ = write!(b, "{}", 440);
        append_suffix(&mut b, "Hz");
        assert_eq!(b.as_str(), "440 Hz");

        let mut plain = ValBuf::new();
        let _ = write!(plain, "{}", 50);
        append_suffix(&mut plain, ""); // empty suffix is a no-op
        assert_eq!(plain.as_str(), "50");
    }

    #[test]
    fn needs_redraw_on_transitions() {
        // A press that opens an editor or enters a submenu changes what renders
        // next frame, so the menu asks the app to redraw once more before
        // blocking for input (otherwise the new screen appears only after the
        // next encoder turn).
        let mut st = MenuState::new();
        let mut s = S::default();
        frame(&mut st, &mut s, MenuInput::None);
        assert!(!st.needs_redraw());

        // Open the value editor.
        frame(&mut st, &mut s, MenuInput::Press);
        assert!(st.is_editing());
        assert!(
            st.needs_redraw(),
            "opening the editor must request a redraw"
        );
        // The editor frame itself clears the request.
        frame(&mut st, &mut s, MenuInput::None);
        assert!(!st.needs_redraw());
        // Leaving the editor renders the list in the same frame — no extra redraw.
        frame(&mut st, &mut s, MenuInput::Press);
        assert!(!st.is_editing());
        assert!(!st.needs_redraw());

        // Enter the submenu (row 3).
        frame(&mut st, &mut s, MenuInput::Turn(3));
        assert_eq!(st.cursor(), 3);
        frame(&mut st, &mut s, MenuInput::Press);
        assert_eq!(st.depth(), 1);
        assert!(
            st.needs_redraw(),
            "entering a submenu must request a redraw"
        );
        // The child renders next frame; with a constant-width gutter there is no
        // follow-up redraw.
        frame(&mut st, &mut s, MenuInput::None);
        assert!(!st.needs_redraw());
    }

    #[test]
    fn int_value_edits_unclamped() {
        // A rangeless value has no min/max: editing steps freely in both
        // directions (and the editor draws the big-font, no-bar form).
        fn f(st: &mut MenuState, v: &mut i32, input: MenuInput) {
            let mut d = NullTarget;
            let style = MenuStyle {
                top_inset: 0,
                ..Default::default()
            };
            Menu::show(&mut d, st, input, &style, |ui| {
                ui.title("CLOCK");
                ui.int_value("TEMPO", v, "BPM");
            });
        }
        let mut st = MenuState::new();
        let mut v = 120;
        f(&mut st, &mut v, MenuInput::None); // focus the row
        f(&mut st, &mut v, MenuInput::Press); // enter edit
        assert!(st.is_editing());
        f(&mut st, &mut v, MenuInput::Turn(5));
        assert_eq!(v, 125);
        f(&mut st, &mut v, MenuInput::Turn(-200)); // no lower bound
        assert_eq!(v, -75);
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
    fn edit_changes_focused_value_without_press() {
        let mut st = MenuState::new();
        let mut s = S {
            freq: 100,
            ..S::default()
        };
        frame(&mut st, &mut s, MenuInput::None);
        // Focused on FREQ (row 0): Edit applies directly, no Press/edit-mode.
        frame(&mut st, &mut s, MenuInput::Edit(25));
        assert_eq!(s.freq, 125);
        assert!(!st.is_editing());
        // Move to WAVE (row 2: FREQ, MONO, WAVE, ADVANCED) and Edit-cycle directly.
        frame(&mut st, &mut s, MenuInput::Turn(2));
        frame(&mut st, &mut s, MenuInput::Edit(1));
        assert_eq!(s.wave, Wave::Saw);
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
            Menu::show(&mut d, state, input, &style, |ui| {
                ui.title("LIST");
                for i in 0..6 {
                    let _ = ui.entry(["A", "B", "C", "D", "E", "F"][i]);
                }
            });
        }
        let mut st = MenuState::new();
        frame6(&mut st, MenuInput::None);
        for _ in 0..4 {
            frame6(&mut st, MenuInput::Turn(1));
        }
        assert_eq!(st.cursor(), 4);
        assert_eq!(st.scroll, 2); // window [2..5] keeps row 4 visible
    }

    #[test]
    fn scroll_window_keeps_cursor_visible() {
        // 3-row window over 6 rows.
        assert_eq!(scroll_window(0, 0, 6, 3), 0);
        assert_eq!(scroll_window(2, 0, 6, 3), 0); // still fits the window
        assert_eq!(scroll_window(3, 0, 6, 3), 1); // crossed the bottom → advance now
        assert_eq!(scroll_window(5, 1, 6, 3), 3); // last row → window [3..6)
        assert_eq!(scroll_window(1, 3, 6, 3), 1); // crossed the top → retreat now
        assert_eq!(scroll_window(4, 3, 6, 3), 3); // still inside [3..6) → unchanged
        assert_eq!(scroll_window(0, 0, 3, 3), 0); // n <= max_visible → no scroll
    }

    #[test]
    fn focused_row_stays_visible_crossing_bottom_edge() {
        // Regression: moving the cursor past the bottom of the window must scroll
        // in the *same* frame, so the focused row is drawn highlighted (not off
        // the visible list until the next turn). A state-only check can't catch
        // this — the scroll converges either way — so render and inspect the
        // focused row's highlight bar.
        use embedded_graphics_simulator::SimulatorDisplay;
        let style = MenuStyle {
            top_inset: 0,
            max_visible: 3,
            ..Default::default()
        };
        let labels = ["A", "B", "C", "D", "E", "F"];
        let render = |d: &mut SimulatorDisplay<BinaryColor>, st: &mut MenuState, input| {
            let _ = d.clear(BinaryColor::Off);
            Menu::show(d, st, input, &style, |ui| {
                ui.title("LIST");
                for l in labels {
                    let _ = ui.entry(l);
                }
            });
        };

        let mut st = MenuState::new();
        let mut d: SimulatorDisplay<BinaryColor> =
            SimulatorDisplay::new(Size::new(DISPLAY_WIDTH, 48));
        render(&mut d, &mut st, MenuInput::None); // rows_last = 6
        render(&mut d, &mut st, MenuInput::Turn(2)); // cursor 2, still window [0..3)
        render(&mut d, &mut st, MenuInput::Turn(1)); // cursor 3 → must scroll to [1..4) now
        assert_eq!(st.cursor(), 3);

        // Focused row is the 3rd visible slot (cursor - scroll = 3 - 1 = 2); its
        // highlight is a filled bar, so that band is mostly lit.
        let slot = (st.cursor() - st.scroll()) as i32;
        let y0 = style.top_inset + TITLE_H + slot * ITEM_H - 1;
        let mut lit = 0;
        for y in y0..(y0 + ITEM_H) {
            for x in 0..40 {
                if d.get_pixel(Point::new(x, y)) == BinaryColor::On {
                    lit += 1;
                }
            }
        }
        assert!(
            lit > 40 * ITEM_H / 2,
            "focused row should be a filled highlight in view, got {lit} lit px"
        );
    }

    /// Render a full frame to a real framebuffer (not the null target) and check
    /// the immediate-mode pipeline actually lights pixels: the title text, its
    /// underline, and the focused-row highlight bar.
    #[test]
    fn render_smoke_draws_pixels() {
        use embedded_graphics_simulator::SimulatorDisplay;

        let mut st = MenuState::new();
        let mut s = S::default();
        let style = MenuStyle::default();
        let mut d: SimulatorDisplay<BinaryColor> =
            SimulatorDisplay::new(Size::new(DISPLAY_WIDTH, 48));

        Menu::show(&mut d, &mut st, MenuInput::None, &style, |ui| {
            ui.title("SOUND");
            ui.int("FREQ", &mut s.freq, 20..=20000);
            ui.toggle("MONO", &mut s.mono);
        });

        // Count lit pixels — a blank screen would mean the pipeline drew nothing.
        let lit = d
            .bounding_box()
            .points()
            .filter(|&p| d.get_pixel(p) == BinaryColor::On)
            .count();
        assert!(
            lit > 20,
            "expected the menu to light real pixels, got {lit}"
        );
    }
}
