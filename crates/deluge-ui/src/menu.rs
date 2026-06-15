//! Menu system for parameter presentation
//!
//! Provides a menu builder and rendering system for displaying parameters
//! on the Deluge OLED display.

use crate::prelude::*;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle, StyledDrawable},
    text::Alignment,
};

use crate::{
    DISPLAY_WIDTH,
    horizontal_menu::HorizontalMenu,
    icons,
    primitives::Icon,
    text::{Font, TextStyle, draw_text},
};

#[derive(Debug, Clone)]
pub enum Menu {
    List(ListMenu),
    Value(ValueMenu),
    Horizontal(HorizontalMenu),
}

/// Value menu for editing a single parameter
#[derive(Debug, Clone)]
pub struct ValueMenu {
    title: String,
    // Add value editing fields as needed
}

impl ValueMenu {
    /// Create a new value menu
    pub fn new(title: String) -> Self {
        Self {
            title,
            // Initialize other fields as needed
        }
    }

    /// Get the title
    pub fn title(&self) -> &str {
        &self.title
    }
}

/// Menu item types
#[derive(Debug, Clone)]
pub enum ListMenuItem {
    TextOnly {
        label: &'static str,
    },
    TextWithIconLeft {
        icon: &'static icons::IconData,
        label: &'static str,
    },
    TextWithIconRight {
        icon: &'static icons::IconData,
        label: &'static str,
    },
}

impl ListMenuItem {
    /// Get the label text
    pub fn label(&self) -> &str {
        match self {
            ListMenuItem::TextOnly { label } => label,
            ListMenuItem::TextWithIconLeft { label, .. } => label,
            ListMenuItem::TextWithIconRight { label, .. } => label,
        }
    }
}

const MAX_VISIBLE_ITEMS: usize = 3;

/// Menu display state - just for rendering a single menu screen
#[derive(Debug, Clone)]
pub struct ListMenu {
    title: String,
    items: Vec<ListMenuItem>,
    selected_index: usize,
    scroll_offset: usize,
}

impl ListMenu {
    /// Create a new menu display
    pub fn new(title: String, items: Vec<ListMenuItem>, selected_index: usize) -> Self {
        let mut menu = Self {
            title,
            items,
            selected_index,
            scroll_offset: 0,
        };
        menu.update_scroll();
        menu
    }

    /// Get the title
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Get current items
    pub fn items(&self) -> &[ListMenuItem] {
        &self.items
    }

    /// Get selected index
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Update scroll offset to keep selection visible
    fn update_scroll(&mut self) {
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + MAX_VISIBLE_ITEMS {
            self.scroll_offset = self.selected_index - MAX_VISIBLE_ITEMS + 1;
        }
    }

    /// Render the menu to a display
    pub fn render<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        // Deluge menu layout constants (matching actual Deluge firmware)
        const TITLE_HEIGHT: u32 = 14; // First row for title
        const ITEM_HEIGHT: u32 = 9; // 11 pixels per menu item (kTextSpacingY in firmware)
        const TEXT_PADDING_X: i32 = 3; // kTextSpacingX in firmware
        const TEXT_PADDING_Y: i32 = 1; // Vertical padding within item
        const SCROLLBAR_WIDTH: i32 = 4; // Scrollbar width in pixels

        // Draw title (Deluge style: normal text with underline, not inverted)
        // Matches Canvas::drawScreenTitle() from deluge firmware
        let title_style = TextStyle::new(Font::MetricBold9px)
            .with_alignment(Alignment::Left)
            .with_color(BinaryColor::On);
        draw_text(
            display,
            &self.title,
            Point::new(TEXT_PADDING_X, 1),
            title_style,
        )?;

        // Draw horizontal line under title (at y=11 in firmware)
        Line::new(Point::new(0, 11), Point::new(DISPLAY_WIDTH as i32 - 1, 11))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;

        // Calculate content width (reserve space for scrollbar if needed)
        let has_scrollbar = self.items.len() > MAX_VISIBLE_ITEMS;
        let content_width = if has_scrollbar {
            DISPLAY_WIDTH - SCROLLBAR_WIDTH as u32
        } else {
            DISPLAY_WIDTH
        };

        // Draw menu items (Deluge style: 3 items visible at a time)
        let visible_end = (self.scroll_offset + MAX_VISIBLE_ITEMS).min(self.items.len());
        for (idx, item) in self.items[self.scroll_offset..visible_end]
            .iter()
            .enumerate()
        {
            let global_idx = self.scroll_offset + idx;
            let y = TITLE_HEIGHT as i32 + (idx as i32 * ITEM_HEIGHT as i32); // + TEXT_PADDING_Y;
            let is_selected = global_idx == self.selected_index;

            // Deluge style: Only invert background when item is selected
            // (both in navigation mode and editing mode)
            if is_selected {
                Rectangle::new(
                    Point::new(0, y - TEXT_PADDING_Y),
                    Size::new(content_width, ITEM_HEIGHT),
                )
                .draw_styled(&PrimitiveStyle::with_fill(BinaryColor::On), display)?;
            }

            // Inverted colors for selected item (black text on white background)
            let text_color = if is_selected {
                BinaryColor::Off
            } else {
                BinaryColor::On
            };

            // Draw label (left-aligned)
            let label_style = TextStyle::new(Font::FontApple).with_color(text_color);
            draw_text(
                display,
                item.label(),
                Point::new(TEXT_PADDING_X, y),
                label_style,
            )?;

            // Draw value/type indicator (right side) - matches renderSubmenuItemTypeForOled()
            // Icon position calculation from firmware: OLED_MAIN_WIDTH_PIXELS - kSubmenuIconSpacingX - 3
            const ICON_SPACING_X: i32 = 7; // kSubmenuIconSpacingX from firmware
            let content_right_edge = content_width as i32;
            let icon_x = content_right_edge - ICON_SPACING_X - 3;

            match item {
                ListMenuItem::TextWithIconRight { icon, .. } => {
                    // Icon on the right (e.g., submenu arrow, checkbox)
                    Icon::new(icon, Point::new(icon_x, y))
                        .with_color(text_color)
                        .draw(display)?;
                }
                ListMenuItem::TextWithIconLeft { icon, .. } => {
                    // Icon on the left side
                    let icon_x = TEXT_PADDING_X;
                    Icon::new(icon, Point::new(icon_x, y))
                        .with_color(text_color)
                        .draw(display)?;
                }
                ListMenuItem::TextOnly { .. } => {
                    // Plain text item, no icons
                }
            }
        }

        // Draw scrollbar if there are more items than can be displayed
        if self.items.len() > MAX_VISIBLE_ITEMS {
            self.draw_scrollbar(display, TITLE_HEIGHT, ITEM_HEIGHT, SCROLLBAR_WIDTH)?;
        }

        Ok(())
    }

    /// Draw the scrollbar indicator
    fn draw_scrollbar<D>(
        &self,
        display: &mut D,
        title_height: u32,
        item_height: u32,
        _scrollbar_width: i32,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        // Calculate scrollbar dimensions
        let content_start_y = title_height as i32;
        let content_height = (MAX_VISIBLE_ITEMS as u32 * item_height) as i32;

        // Calculate scrollbar position as a ratio
        let total_items = self.items.len() as f32;
        let visible_items = MAX_VISIBLE_ITEMS as f32;
        let scroll_ratio = self.scroll_offset as f32 / (total_items - visible_items);

        // Calculate indicator height (proportional to visible items / total items)
        let indicator_height = ((visible_items / total_items) * content_height as f32) as i32;
        let indicator_height = indicator_height.max(3); // Minimum height of 3 pixels

        // Calculate indicator position
        let scrollable_height = content_height - indicator_height;
        let indicator_y = content_start_y + (scrollable_height as f32 * scroll_ratio) as i32;

        // Scrollbar x position - moved one pixel to the right
        // Column 0 (empty): DISPLAY_WIDTH - 4
        // Column 1 (track): DISPLAY_WIDTH - 3
        // Columns 2-3 (indicator): DISPLAY_WIDTH - 2, DISPLAY_WIDTH - 1
        let scrollbar_x = DISPLAY_WIDTH as i32 - 2; // Start at column 1 (track position)

        // Draw the position indicator as a 3-pixel wide hollow rectangle
        // The indicator includes the track line plus 2 pixels to the right
        let indicator_rect = Rectangle::new(
            Point::new(scrollbar_x - 1, indicator_y),
            Size::new(3, indicator_height as u32),
        );
        indicator_rect
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;

        // Draw the scrollbar track lines (above and below the indicator)
        // Top track line (from content start to top of indicator)
        if indicator_y > content_start_y {
            Line::new(
                Point::new(scrollbar_x, content_start_y),
                Point::new(scrollbar_x, indicator_y - 1),
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;
        }

        // Bottom track line (from bottom of indicator to content end)
        let indicator_bottom = indicator_y + indicator_height;
        let content_bottom = content_start_y + content_height - 1;
        if indicator_bottom < content_bottom {
            Line::new(
                Point::new(scrollbar_x, indicator_bottom),
                Point::new(scrollbar_x, content_bottom),
            )
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;
        }

        Ok(())
    }
}

/// Menu navigator - manages menu state and navigation including submenu stack
pub struct MenuNavigator {
    current_menu: Menu,
    menu_stack: Vec<Menu>,
}

impl MenuNavigator {
    /// Create a new menu navigator with a list menu
    pub fn new(title: impl Into<String>, items: Vec<ListMenuItem>) -> Self {
        Self {
            current_menu: Menu::List(ListMenu::new(title.into(), items, 0)),
            menu_stack: Vec::new(),
        }
    }

    /// Get the current menu
    pub fn current_menu(&self) -> &Menu {
        &self.current_menu
    }

    /// Get the title of the current menu
    pub fn title(&self) -> &str {
        match &self.current_menu {
            Menu::List(list) => list.title(),
            Menu::Value(value) => value.title(),
            Menu::Horizontal(hmenu) => hmenu.title(),
        }
    }

    /// Get current items (only valid for List menus)
    pub fn items(&self) -> &[ListMenuItem] {
        match &self.current_menu {
            Menu::List(list) => list.items(),
            _ => &[],
        }
    }

    /// Get selected index (only valid for List menus)
    pub fn selected_index(&self) -> usize {
        match &self.current_menu {
            Menu::List(list) => list.selected_index(),
            _ => 0,
        }
    }

    /// Navigate up in menu
    pub fn move_up(&mut self) {
        if let Menu::List(list) = &mut self.current_menu {
            if list.selected_index > 0 {
                list.selected_index -= 1;
                list.update_scroll();
            }
        }
    }

    /// Navigate down in menu
    pub fn move_down(&mut self) {
        if let Menu::List(list) = &mut self.current_menu {
            if list.selected_index < list.items.len().saturating_sub(1) {
                list.selected_index += 1;
                list.update_scroll();
            }
        }
    }

    /// Get the currently selected item (only valid for List menus)
    pub fn selected_item(&self) -> Option<&ListMenuItem> {
        match &self.current_menu {
            Menu::List(list) => list.items.get(list.selected_index),
            _ => None,
        }
    }

    /// Go back (exit submenu if in one)
    pub fn back(&mut self) {
        if let Some(previous_menu) = self.menu_stack.pop() {
            self.current_menu = previous_menu;
        }
    }

    /// Push current menu and navigate to a new menu (list, value, or horizontal)
    pub fn push_menu(&mut self, menu: Menu) {
        self.menu_stack.push(self.current_menu.clone());
        self.current_menu = menu;
    }

    /// Convenience method: Push a list submenu
    pub fn push_submenu(&mut self, title: String, items: Vec<ListMenuItem>) {
        let submenu = Menu::List(ListMenu::new(title, items, 0));
        self.push_menu(submenu);
    }

    /// Convenience method: Push a value editing menu (leaf node)
    pub fn push_value_menu(&mut self, title: String) {
        let value_menu = Menu::Value(ValueMenu::new(title));
        self.push_menu(value_menu);
    }

    /// Convenience method: Push a horizontal menu (leaf node)
    pub fn push_horizontal_menu(&mut self, horizontal_menu: HorizontalMenu) {
        let menu = Menu::Horizontal(horizontal_menu);
        self.push_menu(menu);
    }

    /// Get mutable access to the items (only valid for List menus)
    pub fn items_mut(&mut self) -> Option<&mut [ListMenuItem]> {
        match &mut self.current_menu {
            Menu::List(list) => Some(&mut list.items),
            _ => None,
        }
    }

    /// Check if we're at the root menu (no parent menus in stack)
    pub fn is_root(&self) -> bool {
        self.menu_stack.is_empty()
    }

    /// Get a MenuList for rendering (only valid for List menus)
    pub fn as_menu(&self) -> Option<ListMenu> {
        match &self.current_menu {
            Menu::List(list) => Some(list.clone()),
            _ => None,
        }
    }

    /// Get the current menu for rendering (handles all menu types)
    pub fn as_current_menu(&self) -> &Menu {
        &self.current_menu
    }
}

/// Fluent builder for menu navigators
pub struct MenuListBuilder {
    title: String,
    items: Vec<ListMenuItem>,
}

impl MenuListBuilder {
    /// Create a new menu builder
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
        }
    }

    /// Add a boolean parameter
    pub fn add_bool(mut self, label: &'static str, value: bool) -> Self {
        self.items.push(ListMenuItem::TextWithIconRight {
            icon: if value {
                &icons::CHECKED_BOX
            } else {
                &icons::UNCHECKED_BOX
            },
            label,
        });
        self
    }

    /// Add a submenu entry (just displays name, navigation handled separately)
    pub fn add_submenu(mut self, label: &'static str) -> Self {
        self.items.push(ListMenuItem::TextWithIconRight {
            icon: &icons::SUBMENU_ARROW,
            label,
        });
        self
    }

    pub fn add_entry(mut self, label: &'static str) -> Self {
        self.items.push(ListMenuItem::TextOnly { label });
        self
    }

    pub fn add_entry_with_icon_left(
        mut self,
        icon: &'static icons::IconData,
        label: &'static str,
    ) -> Self {
        self.items
            .push(ListMenuItem::TextWithIconLeft { icon, label });
        self
    }

    pub fn add_entry_with_icon_right(
        mut self,
        icon: &'static icons::IconData,
        label: &'static str,
    ) -> Self {
        self.items
            .push(ListMenuItem::TextWithIconRight { icon, label });
        self
    }

    /// Build the menu navigator
    pub fn build(self) -> MenuNavigator {
        MenuNavigator::new(self.title, self.items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_menu_builder() {
        let nav = MenuListBuilder::new("TEST")
            .add_submenu("Volume")
            .add_bool("Mute", false)
            .build();

        assert_eq!(nav.title(), "TEST");
        assert_eq!(nav.items().len(), 2);
        assert_eq!(nav.selected_index(), 0);
    }

    #[test]
    fn test_menu_navigation() {
        let mut nav = MenuListBuilder::new("TEST")
            .add_submenu("A")
            .add_submenu("B")
            .build();

        assert_eq!(nav.selected_index(), 0);
        nav.move_down();
        assert_eq!(nav.selected_index(), 1);
        nav.move_up();
        assert_eq!(nav.selected_index(), 0);
    }

    #[test]
    fn test_menu_select() {
        let nav = MenuListBuilder::new("TEST")
            .add_submenu("Volume")
            .add_submenu("Save")
            .build();

        // Application would check selected_item() to determine what action to take
        let item = nav.selected_item();
        assert!(item.is_some());
        assert_eq!(item.unwrap().label(), "Volume");
    }

    #[test]
    fn test_menu_item_label() {
        let item = ListMenuItem::TextOnly { label: "Test" };
        assert_eq!(item.label(), "Test");

        let item = ListMenuItem::TextWithIconRight {
            icon: &icons::SUBMENU_ARROW,
            label: "Test",
        };
        assert_eq!(item.label(), "Test");
    }
}
