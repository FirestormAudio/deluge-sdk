//! USB **product** identity (VID / PID / bcdDevice) for the shipping Deluge
//! firmware pipeline — i.e. the app-loader (second-stage bootloader) and the
//! device classes it presents.
//!
//! Example / reference firmwares (`demo-firmware`, `msc-firmware`,
//! `controller-firmware`) deliberately **do not** use this module — each
//! declares its own self-contained development identity, so SDK example code
//! never ships under, or collides with, the real product identity.  See the
//! `USB_VID` / `USB_PID` constants at the top of each example's `main.rs`.
//!
//! # Why identity is centralized here
//!
//! A USB host — macOS especially, and Windows — caches a device's capabilities
//! keyed by the triple **VID : PID : bcdDevice**, and the USB spec forbids a
//! device from changing its descriptors under a fixed identity.  The dangerous
//! moment for this pipeline is the **app-loader → application hand-off**: the
//! loader disconnects and the launched app re-enumerates, back-to-back on the
//! same host.  If the loader's CDC face and a different-class app share one
//! identity, the host can serve stale/cached descriptors and fail to
//! re-enumerate.  The remedy is one PID per distinct descriptor *shape* (not per
//! binary), or at minimum a distinct [`BCD_DEVICE`].
//!
//! # Ownership — READ BEFORE SHIPPING
//!
//! [`VID`] `0x16D0` is **MCS Electronics'** shared vendor ID; individual PIDs
//! are purchased per product.  Stock DelugeFirmware uses the legitimately
//! purchased PID `0x16D0:0x0CE2` (see its `r_usb_pmidi_descriptor.c`, which even
//! links the MCS shop page).
//!
//! The PIDs below currently resolve to [`PID_UNALLOCATED`] (`0x0EDA`) — a value
//! of **unverified ownership** kept only so this stays behavior-neutral.  Before
//! shipping, replace each with a PID we are entitled to use:
//!
//! - **Preferred:** ask Synthstrom to reserve a PID (or small block) under their
//!   `0x16D0` allocation, then give each distinct loader face its own value.
//! - **Independent alternative:** obtain a block from <https://pid.codes>
//!   (VID `0x1209`, free for open-source hardware) and set [`VID`] accordingly.
//!
//! Do **not** reuse stock's `0x0CE2` for a *different* device class: that both
//! re-introduces the stale-descriptor problem and collides with stock firmware's
//! identity.  Reusing `0x0CE2` is valid only for an image that is a
//! stock-equivalent USB-MIDI Deluge (same class, same descriptors).

/// USB Vendor ID.  `0x16D0` = MCS Electronics (shared VID); see the module docs
/// re: ownership.  TODO(usb-ids): confirm or replace once the allocation is
/// settled (Synthstrom block under `0x16D0`, or a pid.codes `0x1209` block).
pub const VID: u16 = 0x16D0;

/// bcdDevice — device release number.  Bump this for real firmware *versions*.
/// It is *also* part of the host's identity key, so a distinct value per loader
/// face is the fallback differentiator when distinct PIDs are unavailable.
pub const BCD_DEVICE: u16 = 0x0010;

/// Placeholder PID of **unverified ownership** that the loader faces currently
/// resolve to (keeps this behavior-neutral).  TODO(usb-ids): allocate real PIDs
/// and stop using this — see the module docs.
pub const PID_UNALLOCATED: u16 = 0x0EDA;

/// App-loader (second-stage bootloader) CDC-ACM dev-upload listener.
/// TODO(usb-ids): allocate a dedicated PID for the loader's CDC identity.
pub const PID_APP_LOADER_CDC: u16 = PID_UNALLOCATED;

/// App-loader (second-stage bootloader) MSC "data transfer" mode (raw SD card).
/// A distinct shape from [`PID_APP_LOADER_CDC`], and the loader switches between
/// the two within one session — so it needs its own identity.
/// TODO(usb-ids): allocate a dedicated PID for the loader's MSC identity.
pub const PID_APP_LOADER_MSC: u16 = PID_UNALLOCATED;
