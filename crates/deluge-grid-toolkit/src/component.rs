//! The resizable-component contract.
//!
//! The hardware grid is a fixed 18×8, but components placed onto it are
//! resizable: a [`FlexibleComponent`] can be told its [`Size`] (in grid units)
//! so it lays out within whatever region it has been given.

use crate::grid::Grid;

/// A component's size, in grid units (rows × cols).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub rows: usize,
    pub cols: usize,
}

impl Size {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self { rows, cols }
    }
}

/// A renderable grid component.
pub trait Component {
    /// Render the component to a grid buffer.
    fn render(&self) -> Grid;
    /// Whether the component needs to be redrawn.
    fn needs_redraw(&self) -> bool;
    /// The size of the component, in grid units.
    fn get_size(&self) -> Size;
}

/// A [`Component`] that can be resized after construction.
pub trait FlexibleComponent: Component {
    /// Set the size of the component.
    fn set_size(&mut self, size: Size);
}
