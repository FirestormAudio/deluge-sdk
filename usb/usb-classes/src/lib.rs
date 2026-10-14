//! Board-agnostic USB classes on Embassy.
//!
//! Device classes are generic over any `embassy_usb::driver::Driver`, and host class drivers
//! over any `embassy_usb_driver::host::UsbHostAllocator`, so nothing here knows which chip or
//! board it runs on. Enable only the classes a firmware uses:
//!
//! | Feature        | Modules |
//! |----------------|---------|
//! | `device-audio` | `audio` (UAC2) |
//! | `device-midi`  | `midi` (USB-MIDI), [`midi_packets`] (packet-mode queues, UMP sizing) |
//! | `device-msc`   | `msc` (Mass Storage), `bot` (Bulk-Only Transport / SCSI) |
//! | `host-midi`    | `host::midi` |
//! | `host-uac`     | `host::uac` |
//!
//! Device classes build only for the bare-metal target (`target_os = "none"`), where
//! `embassy-usb` is a dependency. The packet queues and the host drivers also build, and are
//! tested, under QEMU.

#![cfg_attr(target_os = "none", no_std)]

#[cfg(feature = "device-midi")]
pub mod midi_packets;

#[cfg(any(feature = "host-midi", feature = "host-uac"))]
pub mod host;

#[cfg(all(target_os = "none", feature = "device-audio"))]
pub mod audio;
#[cfg(all(target_os = "none", feature = "device-msc"))]
pub mod bot;
#[cfg(all(target_os = "none", feature = "device-midi"))]
pub mod midi;
#[cfg(all(target_os = "none", feature = "device-msc"))]
pub mod msc;
