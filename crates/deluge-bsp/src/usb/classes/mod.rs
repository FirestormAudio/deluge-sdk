//! USB class drivers — device-side class implementations.
//!
//! ## Available classes
//!
//! - [`audio`]: Custom USB Audio Class 2.0 (UAC2) device — 44.1 kHz stereo
//!   24-bit audio out (speaker) and optional feedback endpoint.
//!   Embassy's built-in `UsbAudioClass` only supports UAC1; this module
//!   provides the UAC2 descriptor and endpoint logic required by the
//!   Deluge's high-quality audio interface.
//!
//! - [`msc`]: USB Mass Storage Class (Bulk-Only Transport, SCSI transparent
//!   command set).  Exposes a single bulk IN + bulk OUT endpoint pair plus a
//!   control handler for Get Max LUN / Bulk-Only Mass Storage Reset.  The SCSI
//!   command loop itself lives in the firmware that owns the block device.

pub mod audio;
#[cfg(target_os = "none")]
pub mod midi;
pub mod msc;
