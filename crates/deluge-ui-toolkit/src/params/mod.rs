//! Parameter control visualizations
//!
//! Provides object-oriented parameter controls for the Deluge interface.
//! Each control is represented as a struct that encapsulates position, size,
//! and value, with methods for drawing and updating.
//!
//! # Example
//! ```
//! use deluge_ui_toolkit::params::{UnipolarBar, Pan, Slider, LowPassFilter};
//! use embedded_graphics::prelude::*;
//! use embedded_graphics_simulator::SimulatorDisplay;
//! use embedded_graphics::pixelcolor::BinaryColor;
//!
//! let mut display = SimulatorDisplay::<BinaryColor>::new(Size::new(128, 64));
//!
//! // Create controls
//! let bar = UnipolarBar::new(Point::new(10, 10), Size::new(100, 8), 0.5);
//! let pan = Pan::new(Point::new(10, 30), -0.3);
//! let slider = Slider::new(Point::new(50, 30), 0.7);
//! let lpf = LowPassFilter::new(Point::new(90, 30), 0.7);
//!
//! // Draw them
//! bar.draw(&mut display).unwrap();
//! pan.draw(&mut display).unwrap();
//! slider.draw(&mut display).unwrap();
//! lpf.draw(&mut display).unwrap();
//! ```

mod attack;
mod bipolar_bar;
mod hpf;
mod length_slider;
mod lpf;
mod pan;
mod percent;
mod release;
mod sidechain;
mod slider;
mod unipolar_bar;
mod unipolar_knob;

pub use attack::Attack;
pub use bipolar_bar::BipolarBar;
pub use hpf::HighPassFilter;
pub use length_slider::LengthSlider;
pub use lpf::LowPassFilter;
pub use pan::Pan;
pub use percent::Percent;
pub use release::Release;
pub use sidechain::SidechainDucking;
pub use slider::Slider;
pub use unipolar_bar::UnipolarBar;
pub use unipolar_knob::UnipolarKnob;

// Re-export old function-based API for backward compatibility
// These will be deprecated in a future version
