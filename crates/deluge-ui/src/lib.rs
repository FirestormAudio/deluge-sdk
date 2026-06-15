//! Deluge UI Toolkit
//!
//! Graphics and menu system for the **Synthstrom Audible Deluge 128×48 OLED display**.
//!
//! This toolkit provides:
//! - **Display abstraction** for 128×48 horizontal monochrome OLED
//! - **Menu building** and navigation
//! - **Text rendering** with Deluge fonts
//! - **Graphics primitives** (lines, rectangles, icons)
//! - **Layout helpers** for consistent UI
//!
//! # Display Specs
//!
//! - **Resolution**: 128 pixels wide × 43 pixels tall (visible area)
//! - **Orientation**: Horizontal
//! - **Color**: Monochrome (1-bit per pixel)
//! - **Buffer size**: 688 bytes (128 × 43 / 8)
//!
//! # Rendering
//!
//! Everything here draws onto any `embedded-graphics`
//! [`DrawTarget<Color = BinaryColor>`](embedded_graphics::draw_target::DrawTarget).
//! In the Deluge SDK that target is `deluge::Oled` (a 128×48 `DrawTarget`); the
//! faceplate hides the top 5 rows, so offset toolkit content down by 5 px to land
//! it in the visible 43-row area, then flush.
//!
//! ```ignore
//! use deluge::prelude::*;
//! use deluge_ui_toolkit::{Menu, MenuListBuilder};
//! use embedded_graphics::prelude::*;
//!
//! let mut oled = dlg.oled().await;       // a DrawTarget<Color = BinaryColor>
//! let menu = MenuListBuilder::new("SOUND")
//!     .integer("FREQUENCY", 440, 20, 20000)
//!     .float("RESONANCE", 0.5, 0.0, 1.0)
//!     .build();
//!
//! oled.clear();
//! menu.render(&mut oled.translated(Point::new(0, 5)))?;  // +5 px: skip hidden rows
//! oled.flush().await;
//! ```
// `no_std` for the embedded target; host unit tests link std for the harness.
#![cfg_attr(not(test), no_std)]
extern crate alloc;

/// Re-export alloc items so every sub-module can `use crate::prelude::*`.
pub(crate) mod prelude {
    pub use alloc::{format, string::String, vec, vec::Vec};

    /// Provides `sin`, `cos`, `round`, `ceil`, `exp` as methods on `f32` in no_std context.
    pub trait F32Ext: Sized {
        fn sin(self) -> Self;
        fn cos(self) -> Self;
        fn round(self) -> Self;
        fn ceil(self) -> Self;
        fn exp(self) -> Self;
    }

    impl F32Ext for f32 {
        #[inline(always)]
        fn sin(self) -> f32 {
            libm::sinf(self)
        }
        #[inline(always)]
        fn cos(self) -> f32 {
            libm::cosf(self)
        }
        #[inline(always)]
        fn round(self) -> f32 {
            libm::roundf(self)
        }
        #[inline(always)]
        fn ceil(self) -> f32 {
            libm::ceilf(self)
        }
        #[inline(always)]
        fn exp(self) -> f32 {
            libm::expf(self)
        }
    }
}

pub mod components;
pub mod editors;
pub mod graphics;
pub mod hmenu;
pub mod icons;
pub mod menu;
pub mod params;
pub mod positionable;
pub mod primitives;
pub mod text;

pub use components::{
    ADSR, Envelope, EnvelopeStage, ListMenuView, LoopedWaveform, RowIcon, SlicedWaveform, Waveform,
};
pub use hmenu::HMenu;
pub use icons::IconData;
pub use positionable::Positionable;
pub use primitives::{DottedLine, FilledPolygon};

pub use menu::{Menu, MenuEnum, MenuInput, MenuState, MenuStyle, Response};

pub use text::{
    Font, TextStyle, VariFont, VariTextStyle, VariTextStyleBuilder,
    fonts::{
        FONT_5PX, FONT_APPLE, FONT_METRIC_BOLD_9PX, FONT_METRIC_BOLD_13PX, FONT_METRIC_BOLD_20PX,
    },
};

pub use editors::{
    BasicEditor, BipolarValueEditor, FloatEditor, TextValueEditor, UnipolarValueEditor,
};

/// Display dimensions
///
/// The Deluge OLED is physically 128×64 with a 128×48 framebuffer,
/// but the faceplate hides the top 5 rows.  Only 43 rows are visible.
pub const DISPLAY_WIDTH: u32 = 128;
pub const DISPLAY_HEIGHT: u32 = 43;
pub const DISPLAY_BUFFER_SIZE: usize = (DISPLAY_WIDTH * DISPLAY_HEIGHT / 8) as usize;
