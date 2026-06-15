mod builder;
mod icon;
mod icon_with_label;
mod placeholder;
mod text_with_label;
mod value;

use crate::prelude::*;
use crate::{components::Title, positionable::Positionable};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;

pub use builder::HorizontalMenuBuilder;
pub use icon::IconOnly;
pub use icon_with_label::IconWithLabel;
pub use placeholder::Placeholder;
pub use text_with_label::TextWithLabel;
pub use value::{ParamType, Value};

pub const BASE_Y: i32 = 14;
pub const COLUMN_WIDTH: i32 = 32;
pub const BOTTOM_MARGIN: i32 = 11;

#[derive(Debug, Clone)]
pub enum HorizontalMenuItem {
    Icon(IconOnly),
    IconWithLabel(IconWithLabel),
    TextWithLabel(TextWithLabel),
    Value(Value),
}

impl HorizontalMenuItem {
    /// Set the position of the menu item
    pub fn set_position(&mut self, point: Point) {
        match self {
            HorizontalMenuItem::Icon(icon) => icon.set_position(point),
            HorizontalMenuItem::IconWithLabel(icon) => icon.set_position(point),
            HorizontalMenuItem::TextWithLabel(text) => text.set_position(point),
            HorizontalMenuItem::Value(value) => value.set_position(point),
        }
    }

    /// Set whether this menu item is selected (builder-style method that consumes self)
    pub fn selected(self, selected: bool) -> Self {
        match self {
            HorizontalMenuItem::Icon(icon) => HorizontalMenuItem::Icon(icon.selected(selected)),
            HorizontalMenuItem::IconWithLabel(icon) => {
                HorizontalMenuItem::IconWithLabel(icon.selected(selected))
            }
            HorizontalMenuItem::TextWithLabel(text) => {
                HorizontalMenuItem::TextWithLabel(text.selected(selected))
            }
            HorizontalMenuItem::Value(value) => HorizontalMenuItem::Value(value.selected(selected)),
        }
    }

    /// Draw the menu item
    pub fn draw<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        match self {
            HorizontalMenuItem::Icon(icon) => icon.draw(display),
            HorizontalMenuItem::IconWithLabel(icon) => icon.draw(display),
            HorizontalMenuItem::TextWithLabel(text) => text.draw(display),
            HorizontalMenuItem::Value(value) => value.draw(display),
        }
    }
}

/// Horizontal menu for selecting from options displayed horizontally
#[derive(Debug, Clone)]
pub struct HorizontalMenu {
    title: String,
    items: Vec<HorizontalMenuItem>,
    selected_index: usize,
}

impl HorizontalMenu {
    /// Create a new horizontal menu
    pub fn new(title: String, items: Vec<HorizontalMenuItem>, selected_index: usize) -> Self {
        Self {
            title,
            items,
            selected_index,
        }
    }

    /// Get the title
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Get the items
    pub fn items(&self) -> &[HorizontalMenuItem] {
        &self.items
    }

    /// Get the selected index
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Set the selected index
    pub fn set_selected_index(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected_index = index;
        }
    }

    /// Move selection left
    pub fn move_left(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection right
    pub fn move_right(&mut self) {
        if self.selected_index < self.items.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }
}

impl Drawable for HorizontalMenu {
    type Color = BinaryColor;
    type Output = ();

    /// Render the horizontal menu to a display
    fn draw<D>(&self, display: &mut D) -> Result<Self::Output, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        // Draw the title
        Title::new(&self.title).draw(display)?;

        // Draw all items - position them first, then draw
        let mut items = self.items.clone();
        for (column, item) in items.iter_mut().enumerate() {
            let x = (column as i32) * COLUMN_WIDTH;
            item.set_position(Point::new(x, BASE_Y));
            item.draw(display)?;
        }

        // Fill remaining slots with placeholders
        for column in self.items.len()..4 {
            let mut placeholder = Placeholder::new();
            let x = (column as i32) * COLUMN_WIDTH;
            placeholder.set_position(Point::new(x, BASE_Y));
            placeholder.draw(display)?;
        }

        Ok(())
    }
}
