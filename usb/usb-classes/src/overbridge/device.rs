//! The Overbridge 2 **device** class (embassy-usb, chip-agnostic): what a Digitakt presents to
//! Overwitch, and what the Deluge host class binds.
//!
//! The layout copies the Digitakt's (spec §2.1). Interfaces 0-3 are vendor class under one IAD.
//! Interfaces 1 (to the host) and 2 (from it) carry alternate settings 0-3, with alt 3 the one
//! hosts select. Interface 3 is the parameter channel, which isn't implemented; it is present
//! so the interface numbers match. A board composes USB-MIDI after it (interfaces 4-5, as on
//! the Digitakt) and supplies the identity: VID/PID through the embassy-usb `Config`, and the
//! name and channel counts here.

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::{Direction, Driver, EndpointAddress};
use embassy_usb::types::InterfaceNumber;
use embassy_usb::{Builder, Handler};

use super::proto::{self, CounterTracker, FRAMES_PER_BLOCK, Format, MAX_BLOCK, MAX_CHANNELS};

const VENDOR_CLASS: u8 = 0xFF;
/// Audio endpoints' number: Overwitch addresses 0x83 and 0x03 directly.
const EP_AUDIO: usize = 3;
/// Interface 3's endpoints, as on the Digitakt.
const EP_AUX: usize = 2;
/// Vendor `bRequest` for the name (IN, and OUT to set the host's), and the second IN request.
const REQ_NAME: u8 = 1;
const REQ_NAME_2: u8 = 2;
/// Bytes of a name.
pub const NAME_LEN: usize = 32;

/// What the device presents, besides the VID/PID the board gives embassy-usb.
#[derive(Clone, Copy, Debug)]
pub struct OverbridgeConfig {
    pub format: Format,
    /// Channels this device sends to the host (Digitakt: 12).
    pub ch_to_host: u8,
    /// Channels this device receives from the host (Digitakt: 2).
    pub ch_from_host: u8,
    /// Returned by the name requests, padded to 32 bytes.
    pub name: &'static str,
}

/// State the control handler and the board share.
pub struct Shared {
    host_name: Mutex<CriticalSectionRawMutex, RefCell<[u8; NAME_LEN]>>,
    alt_in: AtomicU8,
    alt_out: AtomicU8,
}

impl Shared {
    pub const fn new() -> Self {
        Self {
            host_name: Mutex::new(RefCell::new([0; NAME_LEN])),
            alt_in: AtomicU8::new(0),
            alt_out: AtomicU8::new(0),
        }
    }

    /// The name the host last set (vendor OUT request 1), or zeros.
    pub fn host_name(&self) -> [u8; NAME_LEN] {
        self.host_name.lock(|n| *n.borrow())
    }

    /// The host has selected alternate setting 3 on both audio interfaces.
    pub fn streaming(&self) -> bool {
        self.alt_in.load(Ordering::Relaxed) == proto::STREAM_ALT
            && self.alt_out.load(Ordering::Relaxed) == proto::STREAM_ALT
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// The class's control handler.
pub struct Control<'a> {
    shared: &'a Shared,
    name: [u8; NAME_LEN],
    if_in: InterfaceNumber,
    if_out: InterfaceNumber,
}

impl<'a> Control<'a> {
    fn new(
        shared: &'a Shared,
        name: &str,
        if_in: InterfaceNumber,
        if_out: InterfaceNumber,
    ) -> Self {
        let mut padded = [0u8; NAME_LEN];
        let n = name.len().min(NAME_LEN);
        padded[..n].copy_from_slice(&name.as_bytes()[..n]);
        Self {
            shared,
            name: padded,
            if_in,
            if_out,
        }
    }

    fn is_ours(req: &Request) -> bool {
        req.request_type == RequestType::Vendor && req.recipient == Recipient::Device
    }
}

impl Handler for Control<'_> {
    fn reset(&mut self) {
        self.shared.alt_in.store(0, Ordering::Relaxed);
        self.shared.alt_out.store(0, Ordering::Relaxed);
    }

    fn set_alternate_setting(&mut self, iface: InterfaceNumber, alternate_setting: u8) {
        if iface == self.if_in {
            self.shared
                .alt_in
                .store(alternate_setting, Ordering::Relaxed);
        } else if iface == self.if_out {
            self.shared
                .alt_out
                .store(alternate_setting, Ordering::Relaxed);
        }
    }

    fn control_out(&mut self, req: Request, data: &[u8]) -> Option<OutResponse> {
        if !Self::is_ours(&req) || req.request != REQ_NAME {
            return None;
        }
        let mut name = [0u8; NAME_LEN];
        let n = data.len().min(NAME_LEN);
        name[..n].copy_from_slice(&data[..n]);
        self.shared.host_name.lock(|h| *h.borrow_mut() = name);
        Some(OutResponse::Accepted)
    }

    fn control_in<'b>(&'b mut self, req: Request, buf: &'b mut [u8]) -> Option<InResponse<'b>> {
        if !Self::is_ours(&req) || !matches!(req.request, REQ_NAME | REQ_NAME_2) {
            return None;
        }
        let n = NAME_LEN.min(buf.len()).min(req.length as usize);
        buf[..n].copy_from_slice(&self.name[..n]);
        Some(InResponse::Accepted(&buf[..n]))
    }
}

/// Storage for the class; give it a `'static` (or `'d`) home, as with embassy-usb's classes.
pub struct State<'a> {
    control: MaybeUninit<Control<'a>>,
    shared: Shared,
}

impl State<'_> {
    pub const fn new() -> Self {
        Self {
            control: MaybeUninit::uninit(),
            shared: Shared::new(),
        }
    }
}

impl Default for State<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sends blocks to the host (interface 1, endpoint 0x83).
pub struct OverbridgeSender<E> {
    ep: E,
    format: Format,
    ch: usize,
    counter: u16,
    block: [u8; MAX_BLOCK],
    frames: [i32; FRAMES_PER_BLOCK * MAX_CHANNELS],
}

/// Receives blocks from the host (interface 2, endpoint 0x03).
pub struct OverbridgeReceiver<E> {
    ep: E,
    format: Format,
    ch: usize,
    counter: CounterTracker,
    block: [u8; MAX_BLOCK],
    frames: [i32; FRAMES_PER_BLOCK * MAX_CHANNELS],
}

/// Add the Overbridge interfaces (0-3) to `builder`. Returns the sender, the receiver, and the
/// shared state. Stream through these handles: they are the largest alternate setting's, and
/// the driver may have moved the endpoint's pipe when that alt was added.
pub fn new<'d, D: Driver<'d>>(
    builder: &mut Builder<'d, D>,
    state: &'d mut State<'d>,
    config: OverbridgeConfig,
) -> (
    OverbridgeSender<D::EndpointIn>,
    OverbridgeReceiver<D::EndpointOut>,
    &'d Shared,
) {
    let (to_host, from_host) = (config.ch_to_host as usize, config.ch_from_host as usize);
    assert!(
        to_host <= MAX_CHANNELS && from_host <= MAX_CHANNELS,
        "an Overbridge block carries at most 64 channels"
    );
    let in_sizes = [
        proto::HEADER_LEN,
        proto::block_len(config.format.from_device_frame_bytes(2)),
        proto::block_len(config.format.from_device_frame_bytes(to_host)),
    ];
    let out_sizes = [
        proto::HEADER_LEN,
        proto::block_len(config.format.to_device_frame_bytes(2)),
        proto::block_len(config.format.to_device_frame_bytes(from_host)),
    ];
    let ep_in_addr = EndpointAddress::from_parts(EP_AUDIO, Direction::In);
    let ep_out_addr = EndpointAddress::from_parts(EP_AUDIO, Direction::Out);

    let mut func = builder.function(VENDOR_CLASS, 0, 0);
    // Interface 0: no endpoints.
    func.interface().alt_setting(VENDOR_CLASS, 0, 0, None);
    // Interface 1: audio to the host, alts 0-3.
    let mut i1 = func.interface();
    let if_in = i1.interface_number();
    i1.alt_setting(VENDOR_CLASS, 0, 0, None);
    let mut ep_in = None;
    for size in in_sizes {
        ep_in = Some(
            i1.alt_setting(VENDOR_CLASS, 0, 0, None)
                .endpoint_interrupt_in(Some(ep_in_addr), size as u16, 1),
        );
    }
    // Interface 2: audio from the host, alts 0-3.
    let mut i2 = func.interface();
    let if_out = i2.interface_number();
    i2.alt_setting(VENDOR_CLASS, 0, 0, None);
    let mut ep_out = None;
    for size in out_sizes {
        ep_out = Some(
            i2.alt_setting(VENDOR_CLASS, 0, 0, None)
                .endpoint_interrupt_out(Some(ep_out_addr), size as u16, 1),
        );
    }
    // Interface 3: the parameter channel (not implemented), present so the numbering matches.
    let mut i3 = func.interface();
    let mut a3 = i3.alt_setting(VENDOR_CLASS, 0, 0, None);
    let _ = a3.endpoint_bulk_out(
        Some(EndpointAddress::from_parts(EP_AUX, Direction::Out)),
        512,
    );
    let _ = a3.endpoint_interrupt_in(
        Some(EndpointAddress::from_parts(EP_AUX, Direction::In)),
        64,
        1,
    );
    drop(func);

    let State { control, shared } = state;
    let shared: &'d Shared = shared;
    let control = control.write(Control::new(shared, config.name, if_in, if_out));
    builder.handler(control);

    let sender = OverbridgeSender {
        ep: ep_in.unwrap(),
        format: config.format,
        ch: to_host,
        counter: 0,
        block: [0; MAX_BLOCK],
        frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS],
    };
    let receiver = OverbridgeReceiver {
        ep: ep_out.unwrap(),
        format: config.format,
        ch: from_host,
        counter: CounterTracker::default(),
        block: [0; MAX_BLOCK],
        frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS],
    };
    (sender, receiver, shared)
}
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::test_usb::{EpDesc, config_descriptor, endpoints};
    use embassy_usb::control::{Recipient, Request, RequestType};
    use embassy_usb::driver::Direction;
    use std::vec::Vec;

    fn digitakt() -> OverbridgeConfig {
        OverbridgeConfig {
            format: Format::V2,
            ch_to_host: 12,
            ch_from_host: 2,
            name: "Digitakt",
        }
    }

    fn ep(iface: u8, alt: u8, addr: u8, attributes: u8, max_packet: u16, interval: u8) -> EpDesc {
        EpDesc {
            iface,
            alt,
            addr,
            attributes,
            max_packet,
            interval,
        }
    }

    /// Spec §2.1 / Appendix A, interfaces 0-3.
    #[test]
    fn the_descriptors_match_the_digitakts() {
        let desc = config_descriptor(|b| {
            let state = std::boxed::Box::leak(std::boxed::Box::new(State::new()));
            let _ = new(b, state, digitakt());
        });
        assert_eq!(
            endpoints(&desc),
            [
                ep(1, 1, 0x83, 3, 32, 1),
                ep(1, 2, 0x83, 3, 88, 1),
                ep(1, 3, 0x83, 3, 368, 1),
                ep(2, 1, 0x03, 3, 32, 1),
                ep(2, 2, 0x03, 3, 88, 1),
                ep(2, 3, 0x03, 3, 88, 1),
                ep(3, 0, 0x02, 2, 512, 0),
                ep(3, 0, 0x82, 3, 64, 1),
            ]
        );
        // Interfaces 0-3 are vendor class 0xFF, grouped by one IAD.
        let ifaces: Vec<(u8, u8, u8)> = desc
            .windows(9)
            .filter(|d| d[0] == 9 && d[1] == 4)
            .map(|d| (d[2], d[3], d[5]))
            .collect();
        assert_eq!(ifaces.iter().filter(|i| i.2 == 0xFF).count(), 1 + 4 + 4 + 1);
        assert!(
            desc.windows(8).any(|d| d[0] == 8
                && d[1] == 0x0B
                && d[2] == 0
                && d[3] == 4
                && d[4] == 0xFF),
            "IAD over interfaces 0-3"
        );
    }

    #[test]
    fn a_digitakt_ii_identity_sizes_alt_3_from_its_channels() {
        let desc = config_descriptor(|b| {
            let state = std::boxed::Box::leak(std::boxed::Box::new(State::new()));
            let _ = new(
                b,
                state,
                OverbridgeConfig {
                    format: Format::V2_1,
                    ch_to_host: 42,
                    ch_from_host: 8,
                    name: "Digitakt II",
                },
            );
        });
        let alt3: Vec<(u8, u16)> = endpoints(&desc)
            .iter()
            .filter(|e| e.alt == 3)
            .map(|e| (e.addr, e.max_packet))
            .collect();
        assert_eq!(alt3, [(0x83, 1012), (0x03, 256)]);
    }

    fn vendor(direction: Direction, request: u8, recipient: Recipient, length: u16) -> Request {
        Request {
            direction,
            request_type: RequestType::Vendor,
            recipient,
            request,
            value: 0,
            index: 0,
            length,
        }
    }

    fn control() -> (Control<'static>, &'static Shared) {
        let shared: &'static Shared = std::boxed::Box::leak(std::boxed::Box::new(Shared::new()));
        (
            Control::new(shared, "Digitakt", InterfaceNumber(1), InterfaceNumber(2)),
            shared,
        )
    }

    #[test]
    fn vendor_requests_1_and_2_return_the_name_padded_to_32_bytes() {
        let (mut c, _) = control();
        for r in [1, 2] {
            let mut buf = [0xAAu8; 64];
            let Some(InResponse::Accepted(data)) =
                c.control_in(vendor(Direction::In, r, Recipient::Device, 32), &mut buf)
            else {
                panic!("request {r} not answered");
            };
            assert_eq!(data.len(), 32);
            assert_eq!(&data[..8], b"Digitakt");
            assert!(data[8..].iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn vendor_out_request_1_stores_the_host_name() {
        let (mut c, shared) = control();
        let mut name = [0u8; 32];
        name[..6].copy_from_slice(b"Deluge");
        assert!(matches!(
            c.control_out(vendor(Direction::Out, 1, Recipient::Device, 32), &name),
            Some(OutResponse::Accepted)
        ));
        assert_eq!(shared.host_name(), name);
    }

    #[test]
    fn other_vendor_requests_are_not_claimed() {
        let (mut c, _) = control();
        let mut buf = [0u8; 64];
        assert!(
            c.control_in(vendor(Direction::In, 3, Recipient::Device, 32), &mut buf)
                .is_none()
        );
        assert!(
            c.control_in(vendor(Direction::In, 1, Recipient::Interface, 32), &mut buf)
                .is_none()
        );
        assert!(
            c.control_out(vendor(Direction::Out, 2, Recipient::Device, 32), &[0; 32])
                .is_none()
        );
    }

    #[test]
    fn streaming_needs_alt_3_on_both_audio_interfaces() {
        let (mut c, shared) = control();
        assert!(!shared.streaming());
        c.set_alternate_setting(InterfaceNumber(1), 3);
        assert!(!shared.streaming());
        c.set_alternate_setting(InterfaceNumber(2), 3);
        assert!(shared.streaming());
        c.set_alternate_setting(InterfaceNumber(1), 0);
        assert!(!shared.streaming());
        c.set_alternate_setting(InterfaceNumber(1), 3);
        c.reset();
        assert!(!shared.streaming(), "a bus reset drops the alternates");
    }
}
