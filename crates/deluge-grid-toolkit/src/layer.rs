//! `GridLayer` — compositable pad-colour layers for the grid renderer.
//!
//! A view that needs overlaid rendering can decompose its output into named
//! [`GridLayer`]s that are then flattened into a single [`Grid`] by
//! [`GridCompositor::composite`].
//!
//! | Mode | Behaviour |
//! |------|-----------|
//! | `Replace` | Cell always overwrites the destination. |
//! | `Overlay` | Cell is drawn only where the destination is black. |
//! | `Tint`    | Source colour is multiplied with the destination colour. |

use crate::grid::Grid;
use crate::pad::{GRID_COLS, GRID_ROWS};
use deluge_bsp::rgb::Color;

/// Controls how a [`GridLayer`] is composited onto the output buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    /// Draw this cell unconditionally, overwriting whatever is below.
    Replace,
    /// Draw this cell only where the destination is currently black.
    Overlay,
    /// Multiply this cell's colour with the destination colour component-wise.
    Tint,
}

/// A single layer of pad colours to be composited onto a [`Grid`].
///
/// Each cell holds `Option<Color>`: `None` means "transparent".
#[derive(Debug, Clone)]
pub struct GridLayer {
    /// Per-cell colour data: `None` = transparent, `Some(c)` = opaque.
    pub cells: [[Option<Color>; GRID_COLS]; GRID_ROWS],
    /// How this layer blends with the layers beneath it.
    pub blend_mode: BlendMode,
}

impl GridLayer {
    /// Create an empty (fully transparent) layer with the given blend mode.
    pub fn empty(blend_mode: BlendMode) -> Self {
        Self {
            cells: [[None; GRID_COLS]; GRID_ROWS],
            blend_mode,
        }
    }

    /// Create a layer pre-populated from a [`Grid`] (every cell opaque).
    pub fn from_grid(grid: &Grid, blend_mode: BlendMode) -> Self {
        let mut cells = [[None; GRID_COLS]; GRID_ROWS];
        for (cells_row, grid_row) in cells.iter_mut().zip(grid.buffer.iter()) {
            for (cell, &grid_cell) in cells_row.iter_mut().zip(grid_row.iter()) {
                *cell = grid_cell;
            }
        }
        Self { cells, blend_mode }
    }

    /// Set a single cell's colour. Pass `None` to make the cell transparent.
    pub fn set_cell(&mut self, row: usize, col: usize, color: Option<Color>) {
        if row < GRID_ROWS && col < GRID_COLS {
            self.cells[row][col] = color;
        }
    }

    /// Paint a cell with a solid colour.
    pub fn paint(&mut self, row: usize, col: usize, color: Color) {
        self.set_cell(row, col, Some(color));
    }
}

/// Flattens a slice of [`GridLayer`]s onto a base [`Grid`] in order.
pub struct GridCompositor;

impl GridCompositor {
    /// Composite `layers` on top of `base` (`layers[0]` first, `layers[N-1]` last).
    pub fn composite(mut base: Grid, layers: &[GridLayer]) -> Grid {
        for layer in layers {
            Self::apply_layer(&mut base, layer);
        }
        base
    }

    fn apply_layer(dst: &mut Grid, layer: &GridLayer) {
        for row in 0..GRID_ROWS {
            for col in 0..GRID_COLS {
                let Some(src) = layer.cells[row][col] else {
                    continue;
                };
                match layer.blend_mode {
                    BlendMode::Replace => {
                        dst.buffer[row][col] = Some(src);
                    }
                    BlendMode::Overlay => {
                        let dst_cell = dst.buffer[row][col].unwrap_or(Color::BLACK);
                        if dst_cell == Color::BLACK {
                            dst.buffer[row][col] = Some(src);
                        }
                    }
                    BlendMode::Tint => {
                        let dst_cell = dst.buffer[row][col].unwrap_or(Color::BLACK);
                        dst.buffer[row][col] = Some(Color::rgb(
                            ((dst_cell.r as u16 * src.r as u16) / 255) as u8,
                            ((dst_cell.g as u16 * src.g as u16) / 255) as u8,
                            ((dst_cell.b as u16 * src.b as u16) / 255) as u8,
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_overwrites_all() {
        let base = Grid::new();
        let mut layer = GridLayer::empty(BlendMode::Replace);
        layer.paint(0, 0, Color::rgb(255, 0, 0));
        let out = GridCompositor::composite(base, &[layer]);
        assert_eq!(out.get_pad(0, 0), Color::rgb(255, 0, 0));
    }

    #[test]
    fn overlay_skips_non_black() {
        let mut base = Grid::new();
        base.set_pad(0, 0, Color::rgb(100, 100, 100));
        let mut layer = GridLayer::empty(BlendMode::Overlay);
        layer.paint(0, 0, Color::rgb(255, 0, 0));
        let out = GridCompositor::composite(base, &[layer]);
        assert_eq!(out.get_pad(0, 0), Color::rgb(100, 100, 100));
    }

    #[test]
    fn tint_multiplies_channels() {
        let mut base = Grid::new();
        base.set_pad(0, 0, Color::rgb(255, 128, 64));
        let mut layer = GridLayer::empty(BlendMode::Tint);
        layer.paint(0, 0, Color::rgb(255, 255, 128));
        let out = GridCompositor::composite(base, &[layer]);
        assert_eq!(out.get_pad(0, 0), Color::rgb(255, 128, 32));
    }
}
