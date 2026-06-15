use super::{HorizontalMenu, HorizontalMenuItem, IconOnly, IconWithLabel, TextWithLabel, Value};
use crate::prelude::*;

/// Fluent builder for horizontal menus
pub struct HorizontalMenuBuilder {
    title: String,
    items: Vec<HorizontalMenuItem>,
    selected_index: usize,
}

impl HorizontalMenuBuilder {
    /// Create a new horizontal menu builder
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
            selected_index: 0,
        }
    }

    /// Add an icon item
    pub fn add_icon(mut self, icon: IconOnly) -> Self {
        self.items.push(HorizontalMenuItem::Icon(icon));
        self
    }

    /// Add an icon with label item
    pub fn add_icon_with_label(mut self, icon: IconWithLabel) -> Self {
        self.items.push(HorizontalMenuItem::IconWithLabel(icon));
        self
    }

    /// Add a text with label item
    pub fn add_text_with_label(mut self, text: TextWithLabel) -> Self {
        self.items.push(HorizontalMenuItem::TextWithLabel(text));
        self
    }

    /// Add a value item
    pub fn add_value(mut self, value: Value) -> Self {
        self.items.push(HorizontalMenuItem::Value(value));
        self
    }

    /// Add any horizontal menu item
    pub fn add_item(mut self, item: HorizontalMenuItem) -> Self {
        self.items.push(item);
        self
    }

    /// Set the initially selected index
    pub fn selected_index(mut self, index: usize) -> Self {
        self.selected_index = index;
        self
    }

    /// Build the horizontal menu
    pub fn build(self) -> HorizontalMenu {
        // Mark the item at selected_index as selected
        let items = self
            .items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                if i == self.selected_index {
                    item.selected(true)
                } else {
                    item
                }
            })
            .collect();

        HorizontalMenu::new(self.title, items, self.selected_index)
    }
}
