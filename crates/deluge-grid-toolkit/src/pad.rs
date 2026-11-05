//! Pad addressing for the 18 × 8 grid.

/// Grid rows (matches [`deluge_bsp::rgb::ROWS`]).
pub const GRID_ROWS: usize = deluge_bsp::rgb::ROWS;
/// Main-grid columns (the 16 square pads, excluding the sidebar).
pub const GRID_MAIN_COLS: usize = 16;
/// Sidebar columns.
pub const GRID_SIDE_COLS: usize = 2;
/// Total columns (16 main + 2 sidebar, matches [`deluge_bsp::rgb::COLS`]).
pub const GRID_COLS: usize = deluge_bsp::rgb::COLS;

/// Position of a pad in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pad {
    /// Row (0–7).
    pub row: usize,
    /// Column (0–17, where 0–15 is the main grid, 16–17 the sidebar).
    pub col: usize,
}

impl Pad {
    /// Create a new pad position.
    ///
    /// # Panics
    /// If `row`/`col` are outside the grid.
    pub fn new(row: usize, col: usize) -> Self {
        if row >= GRID_ROWS || col >= GRID_COLS {
            panic!("Invalid pad position: row {}, col {}", row, col);
        }
        Self { row, col }
    }

    /// Whether this position is in the main 8×16 grid.
    pub fn is_main_grid(&self) -> bool {
        self.row < GRID_ROWS && self.col < GRID_MAIN_COLS
    }

    /// Whether this position is in the 8×2 sidebar.
    pub fn is_side_buttons(&self) -> bool {
        self.row < GRID_ROWS && self.col >= GRID_MAIN_COLS && self.col < GRID_COLS
    }

    /// Whether this position is valid for the grid.
    pub fn is_valid_position(&self) -> bool {
        self.row < GRID_ROWS && self.col < GRID_COLS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pad_position() {
        let pos = Pad::new(3, 7);
        assert!(pos.is_main_grid());
        assert!(!pos.is_side_buttons());

        let side = Pad::new(3, 16);
        assert!(!side.is_main_grid());
        assert!(side.is_side_buttons());
    }
}
