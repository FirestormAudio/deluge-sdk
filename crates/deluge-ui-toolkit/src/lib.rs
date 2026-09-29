//! Deluge UI Toolkit
//!
//! Graphics and menu system for the **Synthstrom Audible Deluge 128×48 OLED display**.
//!
//! This toolkit provides:
//! - **Menus**: immediate-mode vertical ([`Menu`]) and param-column ([`HMenu`]) menus
//! - **Parameter visualizations** (knobs, sliders, bars, filters, envelopes)
//! - **Text rendering** with Deluge fonts
//! - **Graphics primitives** (lines, polygons, icons) and layout helpers
//!
//! # Display
//!
//! The panel is 128×48, monochrome (1 bit per pixel); the faceplate hides the
//! top 5 rows, leaving a visible 128×43 area.
//!
//! # Rendering
//!
//! Everything here draws onto any `embedded-graphics`
//! [`DrawTarget<Color = BinaryColor>`](embedded_graphics::draw_target::DrawTarget).
//! In the Deluge SDK that target is `deluge::Oled` (a 128×48 `DrawTarget`); set
//! [`MenuStyle::top_inset`] to `deluge::Oled::VISIBLE_TOP` so content lands in the
//! visible area, then flush.
//!
//! ```ignore
//! use deluge::prelude::*;
//! use deluge_ui_toolkit::{Menu, MenuInput, MenuState, MenuStyle};
//!
//! let mut oled = dlg.oled().await;       // a DrawTarget<Color = BinaryColor>
//! let mut nav = MenuState::new();
//! let style = MenuStyle { top_inset: deluge::Oled::VISIBLE_TOP as i32, ..MenuStyle::default() };
//!
//! oled.clear();
//! Menu::show(&mut oled, &mut nav, MenuInput::None, &style, |ui| {
//!     ui.title("SOUND");
//!     ui.int("FREQ", &mut app.freq, 20..=20000);
//!     ui.float("RESO", &mut app.reso, 0.0..=1.0);
//! });
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
    ADSR, Envelope, EnvelopeStage, ListMenuView, LoopedWaveform, RowIcon, Scrollbar,
    SlicedWaveform, Waveform,
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

/// Display width in pixels.
///
/// This toolkit is display-agnostic (it renders onto any
/// `DrawTarget<Color = BinaryColor>`), so it does **not** hardcode the panel
/// height or the Deluge faceplate cutoff. On the SDK that hardware fact lives in
/// `deluge::Oled::VISIBLE_TOP` / `VISIBLE_HEIGHT` (= `deluge_bsp::oled`); pass the
/// top offset via [`menu::MenuStyle::top_inset`].
pub const DISPLAY_WIDTH: u32 = 128;
