# deluge-ui-toolkit

Graphics and menu toolkit for the Synthstrom Audible Deluge's 128×48 monochrome
OLED.

## Features

- **Menus**: immediate-mode vertical (`Menu`) and parameter-column (`HMenu`)
  menus sharing one `MenuState` / `MenuStyle` / `MenuInput`
- **Parameter visualisations**: knobs, sliders, bars, pan, filter and envelope
  displays, waveforms
- **Text rendering** with the Deluge firmware fonts (`deluge-fonts`)
- **Graphics primitives**: lines, polygons, icons, and layout helpers

Everything draws onto any `embedded-graphics`
`DrawTarget<Color = BinaryColor>`. In the Deluge SDK that target is
`deluge::Oled`, so no framebuffer wrapper is needed.

## Display

- **Resolution**: 128×48, of which 128×43 is visible (the faceplate hides the
  top 5 rows)
- **Colour**: monochrome (1 bit per pixel)

Set `MenuStyle::top_inset` to `deluge::Oled::VISIBLE_TOP` so content lands in
the visible area.

## Usage

The toolkit needs a global allocator: enable the `deluge` crate's `alloc`
feature and build with `-Zbuild-std=core,alloc`.

### Vertical menu

```rust,ignore
use deluge::prelude::*;
use deluge_ui_toolkit::{Menu, MenuInput, MenuState, MenuStyle};

let mut oled = dlg.oled().await;
let mut nav = MenuState::new();
let style = MenuStyle { top_inset: deluge::Oled::VISIBLE_TOP as i32, ..MenuStyle::default() };

oled.clear();
Menu::show(&mut oled, &mut nav, MenuInput::None, &style, |ui| {
    ui.title("SOUND");
    ui.int("FREQ", &mut app.freq, 20..=20000);
    ui.float("RESO", &mut app.reso, 0.0..=1.0);
    ui.submenu("ADVANCED", |ui| {
        ui.toggle("MONO", &mut app.mono);
    });
});
oled.flush().await;
```

### Parameter columns

```rust,ignore
let mut ui = HMenu::begin(&mut oled, &mut nav, input, &style);
ui.title("FILTER");
ui.lpf("CUT", &mut s.cutoff, 0.0..=1.0);
ui.knob("RES", &mut s.res, 0.0..=1.0);
ui.pan("PAN", &mut s.pan, -1.0..=1.0);
ui.end();
```

### Text and graphics

```rust,ignore
use deluge_ui_toolkit::{graphics::draw_line, text::draw_text, Font, TextStyle};
use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point, text::Alignment};

let style = TextStyle::new(Font::MetricBold9px).with_alignment(Alignment::Center);
draw_text(&mut oled, "Hello Deluge", Point::new(64, 20), style)?;
draw_line(&mut oled, Point::new(0, 47), Point::new(127, 47), BinaryColor::On)?;
```

## Examples

[`examples/baremetal/oled_menu`](../../examples/baremetal/oled_menu) and
[`examples/baremetal/oled_hmenu`](../../examples/baremetal/oled_hmenu) are
complete SDK apps built on this crate.

## License

GPL-3.0-or-later. This is a standalone, opt-in crate — the permissive `deluge`
SDK facade does not depend on it, and an app that uses it becomes GPL.

**Note**: The Metric font family is proprietary and licensed to Synthstrom
Audible Limited.
