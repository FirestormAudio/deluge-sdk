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
//! | `host-overbridge` | `host::overbridge`, `overbridge::{proto, stems}` (Elektron Overbridge 2) |
//! | `device-overbridge` | `overbridge::device`, `overbridge::{proto, stems}` |
//!
//! Device classes build only for the bare-metal target (`target_os = "none"`), where
//! `embassy-usb` is a dependency. The packet queues and the host drivers also build, and are
//! tested, under QEMU.
//!
//! The device classes keep their state in statics, so each is a singleton: one instance of a
//! class per firmware. The board crate picks the embassy-time tick rate; this crate only waits
//! on `Timer`.

#![cfg_attr(target_os = "none", no_std)]

#[cfg(feature = "device-midi")]
pub mod midi_packets;

#[cfg(any(
    feature = "host-midi",
    feature = "host-uac",
    feature = "host-overbridge"
))]
pub mod host;

#[cfg(any(feature = "host-overbridge", feature = "device-overbridge"))]
pub mod overbridge;

#[cfg(all(feature = "device-audio", any(target_os = "none", test)))]
pub mod audio;
#[cfg(all(target_os = "none", feature = "device-msc"))]
pub mod bot;
#[cfg(all(target_os = "none", feature = "device-midi"))]
pub mod midi;
#[cfg(all(target_os = "none", feature = "device-msc"))]
pub mod msc;

#[cfg(test)]
mod test_usb;
