//! Deluge hardware control taxonomy (buttons, encoders, indicator LEDs).
//!
//! Vendored from spark's `spark-deluge-core` so the simulator has no cross-repo
//! dependency. These enums name the physical controls; the wire-id ↔ control mapping
//! (which the panel needs to encode input and place inbound LED writes) lives in
//! [`crate::link`], backed by `deluge-protocol`.

pub mod buttons;
pub mod encoders;
pub mod leds;

pub use buttons::HardwareButton;
pub use encoders::HardwareEncoder;
pub use leds::HardwareLED;
