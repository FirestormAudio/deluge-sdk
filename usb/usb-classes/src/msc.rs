//! USB Mass Storage Class (MSC) — Bulk-Only Transport (BOT) device side.
//!
//! This module allocates the MSC interface descriptor (class `0x08`, subclass
//! `0x06` SCSI transparent command set, protocol `0x50` Bulk-Only Transport)
//! together with one **bulk IN** and one **bulk OUT** endpoint, and provides a
//! [`Handler`] for the two class-specific control requests:
//!
//! - **Get Max LUN** (`0xFE`, device→host) — we report a single logical unit.
//! - **Bulk-Only Mass Storage Reset** (`0xFF`, host→device) — acknowledged and
//!   surfaced to the transport loop via [`take_reset`] so it can resynchronise.
//!
//! The SCSI command loop (CBW → data → CSW) is **not** implemented here: it
//! lives in the firmware that owns the block device, which drives the returned
//! endpoints.  This keeps the BSP free of any storage-medium policy.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! let (msc, ep_out, ep_in) = MscClass::new(&mut builder, 512);
//! // store `msc` in a 'static slot and register it:
//! builder.handler(msc_ref);
//! // hand `ep_in` / `ep_out` to the SCSI transport task.
//! ```

use core::sync::atomic::{AtomicBool, Ordering};
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::Driver;
use embassy_usb::types::InterfaceNumber;
use embassy_usb::{Builder, Handler};

/// MSC interface class — Mass Storage.
const USB_CLASS_MSC: u8 = 0x08;
/// MSC subclass — SCSI transparent command set.
const MSC_SUBCLASS_SCSI: u8 = 0x06;
/// MSC protocol — Bulk-Only Transport (BBB).
const MSC_PROTOCOL_BBB: u8 = 0x50;

/// Class request: Bulk-Only Mass Storage Reset (host→device, wLength = 0).
const MSC_REQ_RESET: u8 = 0xFF;
/// Class request: Get Max LUN (device→host, wLength = 1).
const MSC_REQ_GET_MAX_LUN: u8 = 0xFE;

/// Highest logical unit number.  We expose a single LUN (the SD card), so the
/// Get Max LUN response is `0`.
const MAX_LUN: u8 = 0;

/// Set by [`Handler::control_out`] when the host issues a Bulk-Only Reset.
/// The transport loop clears it via [`take_reset`] and resynchronises to the
/// next CBW.
static MSC_RESET: AtomicBool = AtomicBool::new(false);

/// Returns `true` (consuming the flag) if a Bulk-Only Mass Storage Reset has
/// been requested since the last call.  The BOT transport loop polls this and,
/// when set, abandons any in-flight data/status phase and waits for a fresh CBW.
pub fn take_reset() -> bool {
    MSC_RESET.swap(false, Ordering::AcqRel)
}

/// USB MSC (Bulk-Only Transport) class handler.
///
/// Create via [`MscClass::new`] and register with `builder.handler()`.  Holds
/// only the interface number so it can match incoming class control requests.
pub struct MscClass {
    iface: InterfaceNumber,
}

impl MscClass {
    /// Allocate the MSC interface plus its bulk IN/OUT endpoint pair.
    ///
    /// `max_packet_size` should be `512` on this board: the RUSB1 PHY always
    /// negotiates high speed, and USB 2.0 requires high-speed bulk endpoints to
    /// use a 512-byte max packet size (the same constraint the MIDI/CDC classes
    /// document in the firmware).
    ///
    /// Returns `(handler, ep_out, ep_in)`.
    pub fn new<'d, D: Driver<'d>>(
        builder: &mut Builder<'d, D>,
        max_packet_size: u16,
    ) -> (Self, D::EndpointOut, D::EndpointIn) {
        let mut func = builder.function(USB_CLASS_MSC, MSC_SUBCLASS_SCSI, MSC_PROTOCOL_BBB);
        let mut iface_builder = func.interface();
        let iface = iface_builder.interface_number();
        let mut alt =
            iface_builder.alt_setting(USB_CLASS_MSC, MSC_SUBCLASS_SCSI, MSC_PROTOCOL_BBB, None);

        // Bulk OUT (host→device) and bulk IN (device→host).
        let ep_out = alt.endpoint_bulk_out(None, max_packet_size);
        let ep_in = alt.endpoint_bulk_in(None, max_packet_size);

        (Self { iface }, ep_out, ep_in)
    }

    fn targets_iface(&self, req: &Request) -> bool {
        req.request_type == RequestType::Class
            && req.recipient == Recipient::Interface
            && (req.index as u8) == u8::from(self.iface)
    }
}

impl Handler for MscClass {
    fn control_out(&mut self, req: Request, _data: &[u8]) -> Option<OutResponse> {
        if self.targets_iface(&req) && req.request == MSC_REQ_RESET {
            // Bulk-Only Mass Storage Reset: acknowledge and ask the transport
            // loop to resynchronise.  Per the BOT spec the host follows this
            // with Clear-Feature(HALT) on both bulk endpoints.
            MSC_RESET.store(true, Ordering::Release);
            return Some(OutResponse::Accepted);
        }
        None
    }

    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if self.targets_iface(&req) && req.request == MSC_REQ_GET_MAX_LUN && !buf.is_empty() {
            buf[0] = MAX_LUN;
            return Some(InResponse::Accepted(&buf[..1]));
        }
        None
    }

    fn reset(&mut self) {
        // USB bus reset — drop any pending Bulk-Only Reset request.
        MSC_RESET.store(false, Ordering::Release);
    }
}
