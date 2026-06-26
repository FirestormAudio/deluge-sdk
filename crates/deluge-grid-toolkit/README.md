# deluge-grid-toolkit

Reusable, resizable UI components for the Synthstrom Deluge's 18 × 8 RGB pad grid.

A portable toolkit extracted from the [`spark`](https://github.com/FirestormAudio/spark)
project:

- **Colour maths** — the plain `Color` type lives in `deluge-bsp`; the `ColorExt`
  trait here layers on HSV-from-float, hue ramps, blending, tinting and derived
  tail/blur colours.
- **`Grid`** — an 18 × 8 frame buffer with drawing ops; `Grid::blit` writes it
  into a `deluge_bsp::rgb::PadLeds` for the hardware.
- **`GridLayer` / `GridCompositor`** — compositable colour layers.
- **`Component` / `FlexibleComponent`** — the resizable-component contract. The
  grid is a fixed 18 × 8, but components placed onto it are resizable.
- **`animations`** — transition animations (fade, scroll, smear, zoom, explode,
  expand/collapse) that interpolate one `Grid` to another.
- **`widgets`** — stateless render widgets.

`no_std` by default. The optional `simd` feature enables NEON-accelerated colour
interpolation (requires nightly `core::simd`); a scalar fallback is always
available.

## Licence

GPL-3.0-or-later. This is a standalone, opt-in crate — the permissive `deluge`
SDK facade does not depend on it.
