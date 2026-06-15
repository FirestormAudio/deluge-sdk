# deluge-ui-toolkit

Graphics toolkit for the Synthstrom Audible Deluge 128×48 horizontal monochrome OLED display.

## Features

- **Display Abstraction**: `DelugeDisplay` with frame buffer for 128×48 OLED
- **Menu System**: Fluent `MenuBuilder` API for parameter presentation
- **Text Rendering**: Variable-width fonts from Deluge firmware
- **Graphics Primitives**: Lines, rectangles, icons
- **Layout Helpers**: Point, Rect, alignment utilities

## Display Specifications

- **Resolution**: 128×43 pixels (visible area)
- **Orientation**: Horizontal
- **Color**: Monochrome (1-bit)
- **Buffer**: 688 bytes (column-major format)

## Usage

### Basic Display

```rust
use deluge_ui_toolkit::display::DelugeDisplay;

let mut display = DelugeDisplay::new();
display.clear();

// Draw using embedded-graphics primitives
// ...

// Get raw buffer for hardware
let buffer = display.to_buffer(); // 688 bytes
```

### Menu Building

```rust
use deluge_ui_toolkit::menu::MenuBuilder;

let mut menu = MenuBuilder::new("SOUND")
    .add_integer("Volume", 50, 0, 100, 1)
    .add_enum("Wave", vec!["Sine".into(), "Saw".into()], 0)
    .add_float("Cutoff", 1000.0, 20.0, 20000.0, 100.0, 1)
    .add_bool("Filter", true)
    .build();

// Render to display
menu.render(display.framebuf())?;

// Navigate
menu.move_down();
menu.select(); // Enter edit mode
menu.move_up(); // Increment value
menu.select(); // Exit edit mode
```

### Text Rendering

```rust
use deluge_ui_toolkit::{
    text::{draw_text, Font, TextStyle},
    layout::{Alignment, Point},
};
use embedded_graphics::pixelcolor::BinaryColor;

let style = TextStyle::new(Font::MetricBold9px)
    .with_alignment(Alignment::Center)
    .with_color(BinaryColor::On);

draw_text(
    display.framebuf(),
    "Hello Deluge",
    Point::new(64, 20),
    style,
)?;
```

### Graphics Primitives

```rust
use deluge_ui_toolkit::{
    graphics::{draw_line, draw_rect, draw_icon, Icon},
    layout::{Point, Rect},
};
use embedded_graphics::pixelcolor::BinaryColor;

// Draw shapes
draw_line(
    display.framebuf(),
    Point::new(0, 0),
    Point::new(127, 47),
    BinaryColor::On,
)?;

draw_rect(
    display.framebuf(),
    Rect::new(10, 10, 50, 20),
    BinaryColor::On,
)?;

// Draw icons
draw_icon(
    display.framebuf(),
    Icon::ArrowRight,
    Point::new(100, 20),
    BinaryColor::On,
)?;
```

## Menu Items

The menu system supports various parameter types:

- **Integer**: `add_integer(label, value, min, max, step)`
- **Float**: `add_float(label, value, min, max, step, decimals)`
- **Enum**: `add_enum(label, choices, selected_index)`
- **Bool**: `add_bool(label, value)`
- **Submenu**: `add_submenu(label, items)`
- **Action**: `add_action(label)`

## Available Fonts

From `embedded-fonts-deluge`:

- `Font::Font5px` - Original 5px font (very compact)
- `Font::FontApple` - Apple II 7px font (retro style)
- `Font::MetricBold9px` - Metric Bold 9px (recommended for menus)
- `Font::MetricBold13px` - Metric Bold 13px (larger text)
- `Font::MetricBold20px` - Metric Bold 20px (titles only)

## Examples

Run the included example:

```bash
cargo run --package deluge-ui-toolkit --example simple_menu
```

## Architecture

This toolkit is designed to work with the Spark/Firestorm CLAP plugin architecture:

1. **Core**: CLAP plugins define parameters
2. **UI Toolkit**: Presents parameters on Deluge OLED
3. **Extension**: `org.firestorm.hardware-control` bridges hardware and plugins

The menu system automatically presents plugin parameters without hardcoding ranges.

## License

GPL-3.0-or-later

**Note**: The Metric font family is proprietary and licensed to Synthstrom Audible Limited.
