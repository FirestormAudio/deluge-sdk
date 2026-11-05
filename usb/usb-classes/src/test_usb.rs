//! A do-nothing embassy-usb driver for descriptor tests. It hands out endpoints with the
//! requested type and size, and nothing it returns is ever polled.

extern crate std;
use std::vec::Vec;

use embassy_usb::driver::{
    Bus, ControlPipe, Direction, Driver, Endpoint, EndpointAddress, EndpointAllocError,
    EndpointError, EndpointIn, EndpointInfo, EndpointOut, EndpointType, Event, Unsupported,
};
use embassy_usb::{Builder, Config};

pub struct FakeDriver {
    next: [u8; 2],
}

impl FakeDriver {
    pub fn new() -> Self {
        Self { next: [1, 1] }
    }

    fn alloc(
        &mut self,
        dir: Direction,
        ep_type: EndpointType,
        addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> FakeEp {
        // Like a real driver (RUSB1's included), keep only the requested endpoint number and
        // apply this allocation's own direction.
        let addr = addr
            .map(|a| EndpointAddress::from_parts(a.index(), dir))
            .unwrap_or_else(|| {
                let n = &mut self.next[(dir == Direction::In) as usize];
                *n += 1;
                EndpointAddress::from_parts((*n - 1) as usize, dir)
            });
        FakeEp(EndpointInfo {
            addr,
            ep_type,
            max_packet_size,
            interval_ms,
        })
    }
}

pub struct FakeEp(EndpointInfo);

impl Endpoint for FakeEp {
    fn info(&self) -> &EndpointInfo {
        &self.0
    }
    async fn wait_enabled(&mut self) {
        unreachable!("descriptor tests never run the device")
    }
}

impl EndpointIn for FakeEp {
    async fn write(&mut self, _: &[u8]) -> Result<(), EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
}

impl EndpointOut for FakeEp {
    async fn read(&mut self, _: &mut [u8]) -> Result<usize, EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
}

pub struct FakeBus;

impl Bus for FakeBus {
    async fn enable(&mut self) {}
    async fn disable(&mut self) {}
    async fn poll(&mut self) -> Event {
        unreachable!("descriptor tests never run the device")
    }
    fn endpoint_set_enabled(&mut self, _: EndpointAddress, _: bool) {}
    fn endpoint_set_stalled(&mut self, _: EndpointAddress, _: bool) {}
    fn endpoint_is_stalled(&mut self, _: EndpointAddress) -> bool {
        false
    }
    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        Err(Unsupported)
    }
}

pub struct FakeControl;

impl ControlPipe for FakeControl {
    fn max_packet_size(&self) -> usize {
        64
    }
    async fn setup(&mut self) -> [u8; 8] {
        unreachable!("descriptor tests never run the device")
    }
    async fn data_out(&mut self, _: &mut [u8], _: bool, _: bool) -> Result<usize, EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
    async fn data_in(&mut self, _: &[u8], _: bool, _: bool) -> Result<(), EndpointError> {
        unreachable!("descriptor tests never run the device")
    }
    async fn accept(&mut self) {}
    async fn reject(&mut self) {}
    async fn accept_set_address(&mut self, _: u8) {}
}

impl<'a> Driver<'a> for FakeDriver {
    type EndpointOut = FakeEp;
    type EndpointIn = FakeEp;
    type ControlPipe = FakeControl;
    type Bus = FakeBus;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<FakeEp, EndpointAllocError> {
        Ok(self.alloc(
            Direction::Out,
            ep_type,
            ep_addr,
            max_packet_size,
            interval_ms,
        ))
    }
    fn alloc_endpoint_in(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<FakeEp, EndpointAllocError> {
        Ok(self.alloc(
            Direction::In,
            ep_type,
            ep_addr,
            max_packet_size,
            interval_ms,
        ))
    }
    fn start(self, _: u16) -> (FakeBus, FakeControl) {
        (FakeBus, FakeControl)
    }
}

/// Build an IAD-composite device with `f` adding its classes, and return the configuration
/// descriptor (wTotalLength bytes).
pub fn config_descriptor(f: impl FnOnce(&mut Builder<'_, FakeDriver>)) -> Vec<u8> {
    let mut cfg = [0u8; 2048];
    let mut bos = [0u8; 256];
    let mut msos = [0u8; 256];
    let mut ctl = [0u8; 256];
    let mut config = Config::new(0x1209, 0x0001);
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;
    {
        let mut builder = Builder::new(
            FakeDriver::new(),
            config,
            &mut cfg,
            &mut bos,
            &mut msos,
            &mut ctl,
        );
        f(&mut builder);
        drop(builder.build());
    }
    let total = u16::from_le_bytes([cfg[2], cfg[3]]) as usize;
    cfg[..total].to_vec()
}

/// One endpoint descriptor, with the interface and alternate setting it sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpDesc {
    pub iface: u8,
    pub alt: u8,
    pub addr: u8,
    pub attributes: u8,
    pub max_packet: u16,
    pub interval: u8,
}

/// Every endpoint descriptor in a configuration descriptor, in order.
pub fn endpoints(desc: &[u8]) -> Vec<EpDesc> {
    let (mut iface, mut alt) = (0, 0);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < desc.len() && desc[i] >= 2 {
        let d = &desc[i..i + desc[i] as usize];
        match d[1] {
            4 => (iface, alt) = (d[2], d[3]),
            5 => out.push(EpDesc {
                iface,
                alt,
                addr: d[2],
                attributes: d[3],
                max_packet: u16::from_le_bytes([d[4], d[5]]),
                interval: d[6],
            }),
            _ => {}
        }
        i += d.len();
    }
    out
}
