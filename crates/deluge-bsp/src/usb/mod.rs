//! The Deluge's USB layer.
//!
//! The chip-level RUSB1 device/host drivers (register map, FIFO/pipe layers,
//! `embassy-usb-driver` implementations, `init_device_mode` /
//! `init_host_mode`) live in [`rza1_hal::usb`], and the board-agnostic class
//! layer (UAC2, USB-MIDI, MSC + Bulk-Only Transport, host MIDI/UAC drivers) in
//! the `usb-classes` crate. This module keeps what is specific to the Deluge:
//!
//! - [`ids`]: the Deluge's USB vendor/product identities.
//! - `sd_block` (feature `usb-msc`): the SD card as a Mass Storage medium for `usb_classes::bot`.
//! - `host` (feature `usb-host-core`, plus `usb-host-uac` / `usb-host-overbridge` for audio):
//!   the host-role supervisor and the hosted-MIDI
//!   device registry.
//!
//! See [`rza1_hal::usb`] for the quick-start examples (device mode, host
//! mode, ISR wiring).

// Host-role supervisor; opt-in via the `usb-host-core` feature (only host-role
// firmwares need `embassy-usb-host`).
#[cfg(feature = "usb-host-core")]
pub mod host;
pub mod ids;
/// The SD card as a Mass Storage medium for `usb_classes::bot`; opt-in via the
/// `usb-msc` feature.
#[cfg(all(target_os = "none", feature = "usb-msc"))]
pub mod sd_block;
