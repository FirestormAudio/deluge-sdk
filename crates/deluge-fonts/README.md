# deluge-fonts

Fonts extracted from the Synthstrom Audible Deluge firmware for use with embedded-graphics.

## Available Fonts

| Font Variant | Height | Character Range | Notes |
|--------------|--------|-----------------|-------|
| `Font::Font5px` | 5px | U+0020 to U+005A | Tiny font, uppercase only |
| `Font::FontApple` | 7px | U+0020 to U+005A | Apple II style, uppercase only |
| `Font::MetricBold9px` | 9px | U+0020 to U+007E | Default UI font |
| `Font::MetricBold13px` | 13px | U+0020 to U+007E | Medium headers |
| `Font::MetricBold20px` | 20px | U+0020 to U+007F | Large titles |

## Seven-Segment Display Emulation

The `seven_segment` module provides rendering of the Deluge's 7-segment LED display as OLED graphics.

```rust
use embedded_fonts_deluge::seven_segment;

// Render a 4-digit number (pass as string)
seven_segment::render_display(&mut display, "1234", 1, 0)?;

// Render text (up to 4 characters)
seven_segment::render_display(&mut display, "LOAD", 1, 0)?;

// Text with decimal points (periods don't consume digit positions)
seven_segment::render_display(&mut display, "V1.3.0", 1, 0)?;

// All segments lit
seven_segment::render_display(&mut display, "8888", 1, 0)?;
```

The `render_display` function accepts strings with up to 4 display positions. It automatically converts:

- Digits (0-9)
- Uppercase and lowercase letters (A-Z, a-z)
- Decimal points (`.`) - adds a dot to the previous digit without consuming a position

This allows strings longer than 4 characters when decimal points are used, such as "V1.3.0" which displays as 4 characters with decimal points.

## Usage

### Basic Text Rendering

The easiest way to use the fonts is with the `Font` enum API:

```rust
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
};
use embedded_fonts_deluge::Font;

fn render(display: &mut impl DrawTarget<Color = BinaryColor>) {
    // Draw text with default 2px spacing (matching Deluge firmware)
    Font::MetricBold9px.draw_text(display, "HELLO WORLD", Point::new(10, 20))?;
    
    // Draw with custom spacing
    Font::MetricBold13px.draw_text_with_spacing(display, "SPACED OUT", Point::new(10, 40), 4)?;
    
    // Draw a single glyph
    let descriptor = &Font::MetricBold9px.descriptors()[0];
    Font::MetricBold9px.draw_glyph(display, descriptor, Point::new(10, 60))?;
}
```

### Font Properties

Access font metadata easily:

```rust
let font = Font::MetricBold9px;

let height = font.height();        // 9
let baseline = font.baseline();    // Baseline offset
let bitmap = font.bitmap();        // Raw bitmap data
let descriptors = font.descriptors(); // Glyph descriptors
```

### Low-Level Access

You can also access the raw font data directly if needed:

```rust
use embedded_fonts_deluge::*;

// Access font data constants
let descriptors = &METRIC_BOLD_9PX_DESCRIPTORS;
let bitmap = &METRIC_BOLD_9PX_BITMAP;
let height = METRIC_BOLD_9PX_HEIGHT;

// Get glyph for 'A' character
let char_index = ('A' as usize) - (' ' as usize);
let glyph = &descriptors[char_index];
println!("Width: {}px, Index: {}", glyph.w_px, glyph.glyph_index);
```

## Examples

Run the demo showing all fonts:

```bash
cargo run --example all_fonts
```

Run the simple demo with just the 9px font:

```bash
cargo run --example demo
```

Run the seven-segment display demo:

```bash
cargo run --example seven_segment
```

## Features

- **`embedded-graphics`** (default): Enables the `Font` enum API and rendering methods. Disable if you only need raw font data.

```toml
# Use without embedded-graphics
deluge-fonts = { version = "0.1", default-features = false }
```

## Important Notes

### Character Support

- **5px and Apple fonts**: Only uppercase letters (A-Z) and symbols
- **Metric fonts**: Only uppercase letters, numbers, and symbols

## Licensing

 The "Metric" font is a **propritary font** licensed to Synthstrom Audible Limited from [Klim Type Foundry](https://klim.co.nz/).

**This font is NOT free to use in other projects.** It is included here for use on the Deluge hardware _only_.
