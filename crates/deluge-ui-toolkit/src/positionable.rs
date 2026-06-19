//! Positionable trait for drawable elements
//!
//! Provides a common interface for all positionable drawable elements.

use embedded_graphics::{Drawable, geometry::Point, pixelcolor::BinaryColor};

pub trait Positionable: Drawable<Color = BinaryColor> + core::fmt::Debug {
    /// Get the position of the drawable
    fn position(&self) -> Point;

    /// Set the position of the drawable
    fn set_position(&mut self, point: Point);

    fn with_position(mut self, point: Point) -> Self
    where
        Self: Sized,
    {
        self.set_position(point);
        self
    }
}
