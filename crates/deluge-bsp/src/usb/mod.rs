//! USB class layer for the Deluge.
//!
//! The chip-level RUSB1 device/host drivers (register map, FIFO/pipe layers,
//! `embassy-usb-driver` implementations, `init_device_mode` /
//! `init_host_mode`) live in [`rza1l_hal::usb`].  This module holds only the
//! board/class layer on top:
//!
//! - [`classes`]: UAC2 audio, USB-MIDI, and MSC class implementations
//!   (generic over any `embassy_usb::driver::Driver`).
//! - [`bot`]: USB Mass Storage Bulk-Only Transport engine.
//!
//! See [`rza1l_hal::usb`] for the quick-start examples (device mode, host
//! mode, ISR wiring).

pub mod bot;
pub mod classes;
