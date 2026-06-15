//! Scrollable list view for the Deluge 128×43 OLED menu system.
//!
//! Encapsulates the render logic and label-scroll animation state shared by
//! all three scrollable-list surfaces: `Submenu`, `DynamicSubmenu`, and
//! `ListPickerItem`.
//!
//! # Display geometry
//!
//! ```text
//!  y=0   ┌─────────────────────────────────────────┐
//!  y=1   │  Title (drawn by caller)                │
//! y=11   ├─────────────────────────────────────────┤
//! y=14   │  Item 0                             [▶] │
//! y=23   │  ██ Item 1 (selected, inverted)     [▶] │
//! y=32   │  Item 2                             [▶] │
//! y=42   └─────────────────────────────────────────┘
//! ```
//!
//! The title bar is **not** drawn here; call `render_title` first.

use embedded_graphics::{
    Drawable,
    draw_target::{DrawTarget, DrawTargetExt},
    geometry::{Point, Size},
    pixelcolor::BinaryColor,
    prelude::Primitive,
    primitives::{Line, PrimitiveStyle, Rectangle},
};

use crate::icons::{CHECKED_BOX, IconData, SUBMENU_ARROW, UNCHECKED_BOX};
// Required for no_std ARM target where core_float_math is unavailable.
#[allow(unused_imports)]
use crate::prelude::F32Ext as _;
use crate::primitives::Icon;
use crate::text::{Font, TextStyle, draw_text};

/// The right-hand icon shown on a list row.
///
/// `Submenu` and `Custom` both occupy the same 7 px wide icon column;
/// `None` means no icon and the text may use that space instead.
#[derive(Clone, Copy, Debug)]
pub enum RowIcon {
    /// No icon — text extends to the right margin.
    None,
    /// Submenu-arrow `▶` (drawn from the toolkit's built-in icon).
    Submenu,
    /// Checked checkbox `☑` — use for a true/on toggle.
    Checked,
    /// Unchecked checkbox `☐` — use for a false/off toggle.
    Unchecked,
    /// Arbitrary toolkit icon.
    Custom(&'static IconData),
}

impl RowIcon {
    /// Returns `true` when the row needs the 7px icon column reserved.
    #[inline]
    fn has_icon(self) -> bool {
        !matches!(self, RowIcon::None)
    }
}

/// Label-scroll animation state and list rendering for the Deluge 3-row OLED list.
///
/// Create one with [`ListMenuView::new`], store it in your item struct, then:
/// - Call [`tick`] every frame to advance the animation.
/// - Call [`render`] from `render_oled` to draw the list (after the title bar).
///
/// [`tick`]: ListMenuView::tick
/// [`render`]: ListMenuView::render
#[derive(Clone, Debug, Default)]
pub struct ListMenuView {
    label_scroll_px: i32,
    label_scroll_ms: u32,
    label_scroll_dir: i32,
    label_scroll_pausing: bool,
    label_scroll_for: Option<usize>,
}

impl ListMenuView {
    pub fn new() -> Self {
        Self {
            label_scroll_px: 0,
            label_scroll_ms: 0,
            label_scroll_dir: 1,
            label_scroll_pausing: false,
            label_scroll_for: None,
        }
    }

    /// Advance the label-scroll animation. Returns `true` when a redraw is needed.
    ///
    /// # Parameters
    /// - `selected_index` — the currently highlighted row index.
    /// - `selected_name` — label text of the selected row (used to measure overflow).
    /// - `selected_icon` — the right-hand icon for the selected row.
    /// - `total_len` — total number of rows (determines scrollbar visibility).
    /// - `delta_ms` — elapsed milliseconds since the last tick.
    pub fn tick(
        &mut self,
        selected_index: usize,
        selected_name: &str,
        selected_icon: RowIcon,
        total_len: usize,
        delta_ms: u32,
    ) -> bool {
        // DelugeFirmware cadence:
        //   forward 15 ms/px  →  pause 400 ms  →  back 5 ms/px  →  pause  →  repeat
        const PAUSE_MS: u32 = 400;
        const FORWARD_MS_PER_PX: u32 = 15;
        const BACKWARD_MS_PER_PX: u32 = 5;
        const MAX_VISIBLE: usize = 3;
        const TEXT_PADDING_X: i32 = 3;

        // Reset when selection changes — enter initial pause.
        if self.label_scroll_for != Some(selected_index) {
            self.label_scroll_px = 0;
            self.label_scroll_ms = 0;
            self.label_scroll_dir = 1;
            self.label_scroll_pausing = true;
            self.label_scroll_for = Some(selected_index);
            return false;
        }

        let has_scrollbar = total_len > MAX_VISIBLE;
        let ix = icon_x_for(selected_icon, has_scrollbar);
        let max_text_right = ix - 2;
        let text_w = Font::FontApple.deluge_font().text_width(selected_name);
        let overflow = text_w - (max_text_right - TEXT_PADDING_X);

        if overflow <= 0 {
            let was_nonzero = self.label_scroll_px != 0;
            self.label_scroll_px = 0;
            self.label_scroll_ms = 0;
            self.label_scroll_dir = 1;
            self.label_scroll_pausing = false;
            return was_nonzero;
        }

        self.label_scroll_ms = self.label_scroll_ms.saturating_add(delta_ms);

        if self.label_scroll_pausing {
            if self.label_scroll_ms >= PAUSE_MS {
                self.label_scroll_ms = 0;
                self.label_scroll_pausing = false;
            }
            return false;
        }

        let ms_per_px = if self.label_scroll_dir > 0 {
            FORWARD_MS_PER_PX
        } else {
            BACKWARD_MS_PER_PX
        };
        let advance = (self.label_scroll_ms / ms_per_px) as i32;
        if advance == 0 {
            return false;
        }

        self.label_scroll_ms %= ms_per_px;
        let new_px = (self.label_scroll_px + advance * self.label_scroll_dir).clamp(0, overflow);
        let changed = new_px != self.label_scroll_px;
        self.label_scroll_px = new_px;
        if self.label_scroll_px <= 0 || self.label_scroll_px >= overflow {
            self.label_scroll_dir = -self.label_scroll_dir;
            self.label_scroll_pausing = true;
            self.label_scroll_ms = 0;
        }
        changed
    }

    /// Render the list rows to `display`.
    ///
    /// Draws item highlights, text (with clipped label-scroll for the selected
    /// row), optional `▶` submenu-arrow icons, and the proportional scrollbar.
    ///
    /// The title bar must be drawn by the caller before this method is called.
    ///
    /// `rows` is a slice of `(name, RowIcon)` pairs.
    /// `empty_message` is shown in `FontApple` when `rows` is empty.
    pub fn render<D>(
        &self,
        display: &mut D,
        rows: &[(&str, RowIcon)],
        selected_index: usize,
        empty_message: Option<&str>,
    ) where
        D: DrawTarget<Color = BinaryColor>,
    {
        const TITLE_HEIGHT: i32 = 14;
        const ITEM_HEIGHT: i32 = 9;
        const TEXT_PADDING_X: i32 = 3;
        const TEXT_PADDING_Y: i32 = 1;
        const FONT_HEIGHT: u32 = 7; // FontApple
        const MAX_VISIBLE: usize = 3;

        if rows.is_empty() {
            if let Some(msg) = empty_message {
                let style = TextStyle::new(Font::FontApple).with_color(BinaryColor::On);
                let _ = draw_text(
                    display,
                    msg,
                    Point::new(TEXT_PADDING_X, TITLE_HEIGHT),
                    style,
                );
            }
            return;
        }

        let has_scrollbar = rows.len() > MAX_VISIBLE;
        let scroll_offset = if selected_index >= MAX_VISIBLE {
            selected_index - MAX_VISIBLE + 1
        } else {
            0
        };
        let visible_end = (scroll_offset + MAX_VISIBLE).min(rows.len());

        for (slot, global_idx) in (scroll_offset..visible_end).enumerate() {
            let (name, row_icon) = rows[global_idx];
            let y = TITLE_HEIGHT + (slot as i32 * ITEM_HEIGHT);
            let is_selected = global_idx == selected_index;

            if is_selected {
                let _ = Rectangle::new(
                    Point::new(0, y - TEXT_PADDING_Y),
                    Size::new(128, ITEM_HEIGHT as u32),
                )
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(display);
            }

            let text_color = if is_selected {
                BinaryColor::Off
            } else {
                BinaryColor::On
            };
            let style = TextStyle::new(Font::FontApple).with_color(text_color);
            let ix = icon_x_for(row_icon, has_scrollbar);
            let max_text_right = ix - 2;

            if is_selected && self.label_scroll_px > 0 {
                let clip_w = max_text_right.max(0) as u32;
                let clip = Rectangle::new(Point::new(0, y), Size::new(clip_w, FONT_HEIGHT));
                let mut clipped = display.clipped(&clip);
                let _ = draw_text(
                    &mut clipped,
                    name,
                    Point::new(TEXT_PADDING_X - self.label_scroll_px, y),
                    style,
                );
            } else {
                let _ = draw_text(display, name, Point::new(TEXT_PADDING_X, y), style);
            }

            let icon_data: Option<&'static IconData> = match row_icon {
                RowIcon::None => None,
                RowIcon::Submenu => Some(&SUBMENU_ARROW),
                RowIcon::Checked => Some(&CHECKED_BOX),
                RowIcon::Unchecked => Some(&UNCHECKED_BOX),
                RowIcon::Custom(d) => Some(d),
            };
            if let Some(data) = icon_data {
                let _ = Icon::new(data, Point::new(ix, y))
                    .with_color(text_color)
                    .draw(display);
            }
        }

        if has_scrollbar {
            draw_scrollbar(display, rows.len(), scroll_offset);
        }
    }
}

/// Returns the x-coordinate at which the icon (or right margin) begins for a row.
fn icon_x_for(row_icon: RowIcon, has_scrollbar: bool) -> i32 {
    match (row_icon.has_icon(), has_scrollbar) {
        (true, true) => 115,
        (true, false) => 119,
        (false, true) => 122,
        (false, false) => 126,
    }
}

/// Draw a proportional scrollbar on the right edge of the 128×43 display.
///
/// The indicator position and height are proportional to the current scroll
/// position and the ratio of visible to total items.
fn draw_scrollbar<D>(display: &mut D, total_items: usize, scroll_offset: usize)
where
    D: DrawTarget<Color = BinaryColor>,
{
    const MAX_VISIBLE: usize = 3;
    /// Y-coordinate where the track begins (just below the title separator).
    const TRACK_TOP: i32 = 14;
    /// Last visible pixel row on the 43-row display (0-indexed).
    const TRACK_BOTTOM: i32 = 42;
    /// Total pixel span of the track (both endpoints inclusive).
    const TRACK_HEIGHT_PX: i32 = TRACK_BOTTOM - TRACK_TOP + 1; // 29
    /// Centre x of the track line.
    const SCROLLBAR_X: i32 = 126; // 128 − 2
    /// Width of the indicator rectangle.
    const INDICATOR_W: u32 = 3;

    if total_items <= MAX_VISIBLE {
        return;
    }

    let total = total_items as f32;
    let visible = MAX_VISIBLE as f32;
    let scroll_ratio = (scroll_offset as f32 / (total - visible).max(1.0)).clamp(0.0, 1.0);

    let indicator_height = ((visible / total) * TRACK_HEIGHT_PX as f32).round() as i32;
    let indicator_height = indicator_height.clamp(3, TRACK_HEIGHT_PX);

    let travel = TRACK_HEIGHT_PX - indicator_height;
    let indicator_y =
        (TRACK_TOP + (travel as f32 * scroll_ratio).round() as i32).min(TRACK_TOP + travel);
    let indicator_bottom = indicator_y + indicator_height;

    // Clear the scrollbar strip so row-inversion fills don't bleed into it.
    let _ = Rectangle::new(
        Point::new(SCROLLBAR_X - 2, TRACK_TOP),
        Size::new(INDICATOR_W + 1, TRACK_HEIGHT_PX as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
    .draw(display);

    // Track above indicator.
    if indicator_y > TRACK_TOP {
        let _ = Line::new(
            Point::new(SCROLLBAR_X, TRACK_TOP),
            Point::new(SCROLLBAR_X, indicator_y - 1),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display);
    }

    // Hollow indicator rectangle.
    let _ = Rectangle::new(
        Point::new(SCROLLBAR_X - 1, indicator_y),
        Size::new(INDICATOR_W, indicator_height as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display);

    // Track below indicator.
    if indicator_bottom <= TRACK_BOTTOM {
        let _ = Line::new(
            Point::new(SCROLLBAR_X, indicator_bottom),
            Point::new(SCROLLBAR_X, TRACK_BOTTOM),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display);
    }
}
