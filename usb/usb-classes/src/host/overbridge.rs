//! USB **host** class for Elektron Overbridge 2 devices (Digitakt, Digitone, Syntakt, Analog
//! Four/Rytm/Heat MKII, Digitakt II, Digitone II).
//!
//! Setup selects alternate setting 3 on interfaces 1 (audio from the device) and 2 (audio to
//! it), then streams: each [`OverbridgeHost::pump_once`] reads one block, decodes it into an
//! [`OverbridgeHandle`], and sends one block back, so the device's clock paces both
//! directions. The endpoints are high-speed interrupt endpoints; the RZ/A1L HAL carries those
//! over 64 bytes on bulk pipes.

use embassy_usb_driver::host::{HostError, PipeError, SplitInfo, UsbHostAllocator, UsbPipe, pipe};
use embassy_usb_driver::{Direction, EndpointType};
use embassy_usb_host::control::{ControlType, Recipient, RequestType, SetupPacket};
use embassy_usb_host::descriptor::{ConfigurationDescriptor, EndpointDescriptor};

use crate::overbridge::proto::{
    self, CounterTracker, FRAMES_PER_BLOCK, Format, MAX_BLOCK, MAX_CHANNELS,
};
use crate::overbridge::stems::{OverbridgeHandle, OverbridgeInfo};

const REQ_SET_INTERFACE: u8 = 0x0B;
/// Consecutive bad blocks that end the stream.
const FRAMING_LIMIT: u32 = 64;
/// Silent blocks sent at setup, so the device sees host traffic at once.
const PRIME_BLOCKS: usize = 2;
/// Window for the rate measurement, in microseconds.
const RATE_WINDOW_US: u64 = 1_000_000;

/// A matched Overbridge device.
#[derive(Clone, Copy, Debug)]
pub struct OverbridgeMatch {
    pub pid: u16,
    pub name: &'static str,
    pub format: Format,
    /// Channels the device sends.
    pub ch_from: usize,
    /// Channels the device receives.
    pub ch_to: usize,
    pub ep_in: EndpointDescriptor,
    pub ep_out: EndpointDescriptor,
}

impl OverbridgeMatch {
    pub fn info(&self) -> OverbridgeInfo {
        OverbridgeInfo {
            pid: self.pid,
            name: self.name,
            ch_from: self.ch_from as u8,
            ch_to: self.ch_to as u8,
            format: self.format,
        }
    }
}

/// Errors from the Overbridge host class.
#[derive(Debug)]
pub enum OverbridgeError {
    /// Not Elektron's VID, or no interrupt endpoints in interfaces 1 and 2 at alt 3.
    NotOverbridge,
    /// An alt-3 endpoint's size isn't a whole, non-zero number of channels.
    BadBlockSize,
    /// More channels than a block can carry, or than the handle's rings hold.
    TooManyChannels,
    /// A pipe couldn't be allocated.
    NoPipe(HostError),
    /// `SET_INTERFACE` failed.
    SetupFailed(PipeError),
    /// A transfer failed (including a detach).
    Transport(PipeError),
    /// [`FRAMING_LIMIT`] bad blocks in a row.
    Framing,
}

/// The alt-3 interrupt endpoint of `interface` going `dir`.
fn stream_endpoint(
    cfg: &ConfigurationDescriptor<'_>,
    interface: u8,
    dir: Direction,
) -> Option<EndpointDescriptor> {
    for i in cfg.iter_interface() {
        if i.interface_number != interface || i.alternate_setting != proto::STREAM_ALT {
            continue;
        }
        if let Some(e) = i
            .iter_endpoints()
            .find(|e| e.ep_type() == EndpointType::Interrupt && e.ep_dir() == dir)
        {
            return Some(e);
        }
    }
    None
}

/// Match an enumerated device. Channel counts come from the endpoint sizes; the device table
/// supplies the name and the v2.1 layout, and the descriptors win when they disagree.
pub fn match_device(
    vid: u16,
    pid: u16,
    cfg: &ConfigurationDescriptor<'_>,
) -> Result<OverbridgeMatch, OverbridgeError> {
    if vid != proto::VID_ELEKTRON {
        return Err(OverbridgeError::NotOverbridge);
    }
    let ep_in = stream_endpoint(cfg, proto::IN_INTERFACE, Direction::In)
        .ok_or(OverbridgeError::NotOverbridge)?;
    let ep_out = stream_endpoint(cfg, proto::OUT_INTERFACE, Direction::Out)
        .ok_or(OverbridgeError::NotOverbridge)?;
    let listed = proto::lookup(pid);
    let format = listed.map_or(Format::V2, |d| d.format);
    let size = |e: &EndpointDescriptor| (e.max_packet_size & 0x7FF) as usize;
    if size(&ep_in) > MAX_BLOCK || size(&ep_out) > MAX_BLOCK {
        return Err(OverbridgeError::BadBlockSize);
    }
    let ch_from = format
        .channels_from_device(size(&ep_in))
        .ok_or(OverbridgeError::BadBlockSize)?;
    let ch_to = format
        .channels_to_device(size(&ep_out))
        .ok_or(OverbridgeError::BadBlockSize)?;
    if ch_from > MAX_CHANNELS || ch_to > MAX_CHANNELS {
        return Err(OverbridgeError::TooManyChannels);
    }
    if let Some(d) = listed.filter(|d| (d.ch_from as usize, d.ch_to as usize) != (ch_from, ch_to)) {
        log::debug!(
            "overbridge: {} declares {}/{} channels; the table says {}/{}",
            d.name,
            ch_from,
            ch_to,
            d.ch_from,
            d.ch_to
        );
    }
    Ok(OverbridgeMatch {
        pid,
        name: listed.map_or("Elektron device", |d| d.name),
        format,
        ch_from,
        ch_to,
        ep_in,
        ep_out,
    })
}

/// Frames received against local time, over [`RATE_WINDOW_US`] windows.
#[derive(Default)]
struct RateMeter {
    start_us: Option<u64>,
    frames: u64,
}

impl RateMeter {
    /// Count `frames` received at `now_us`; returns the ppm offset when a window closes.
    fn add(&mut self, frames: u64, now_us: u64) -> Option<i32> {
        let start = *self.start_us.get_or_insert(now_us);
        let elapsed = now_us.saturating_sub(start);
        if elapsed >= RATE_WINDOW_US {
            let ppm = (self.frames as i64 * 1_000_000_000_000
                / (elapsed as i64 * proto::SAMPLE_RATE as i64))
                - 1_000_000;
            self.start_us = Some(now_us);
            self.frames = frames;
            return Some(ppm as i32);
        }
        self.frames += frames;
        None
    }
}

/// A hosted Overbridge device.
pub struct OverbridgeHost<'d, A: UsbHostAllocator<'d>> {
    control: A::Pipe<pipe::Control, pipe::InOut>,
    rx: A::Pipe<pipe::Interrupt, pipe::In>,
    tx: A::Pipe<pipe::Interrupt, pipe::Out>,
    m: OverbridgeMatch,
    tx_counter: u16,
    rx_counter: CounterTracker,
    framing_run: u32,
    rate: RateMeter,
    rx_block: [u8; MAX_BLOCK],
    tx_block: [u8; MAX_BLOCK],
    frames: [i32; FRAMES_PER_BLOCK * MAX_CHANNELS],
    _p: core::marker::PhantomData<&'d ()>,
}

impl<'d, A: UsbHostAllocator<'d>> OverbridgeHost<'d, A> {
    /// Match, select alternate setting 3 on both audio interfaces, open the stream pipes, and
    /// prime the OUT stream.
    pub async fn try_register(
        alloc: &A,
        addr: u8,
        split: Option<SplitInfo>,
        vid: u16,
        pid: u16,
        cfg: &ConfigurationDescriptor<'_>,
    ) -> Result<Self, OverbridgeError> {
        let m = match_device(vid, pid, cfg)?;
        let mut control = alloc
            .alloc_pipe::<pipe::Control, pipe::InOut>(
                addr,
                &embassy_usb_driver::EndpointInfo {
                    addr: 0u8.into(),
                    ep_type: EndpointType::Control,
                    max_packet_size: 64,
                    interval_ms: 0,
                },
                split,
            )
            .map_err(OverbridgeError::NoPipe)?;
        Self::select_alternates(&mut control).await?;
        // Open the pipes only now: the device must not see traffic for an endpoint it hasn't
        // enabled.
        let rx = alloc
            .alloc_pipe::<pipe::Interrupt, pipe::In>(addr, &m.ep_in.into(), split)
            .map_err(OverbridgeError::NoPipe)?;
        let tx = alloc
            .alloc_pipe::<pipe::Interrupt, pipe::Out>(addr, &m.ep_out.into(), split)
            .map_err(OverbridgeError::NoPipe)?;
        log::info!(
            "overbridge: {} (PID {:04x}), {} ch from / {} ch to, {:?}",
            m.name,
            m.pid,
            m.ch_from,
            m.ch_to,
            m.format
        );
        let mut host = Self {
            control,
            rx,
            tx,
            m,
            tx_counter: 0,
            rx_counter: CounterTracker::default(),
            framing_run: 0,
            rate: RateMeter::default(),
            rx_block: [0; MAX_BLOCK],
            tx_block: [0; MAX_BLOCK],
            frames: [0; FRAMES_PER_BLOCK * MAX_CHANNELS],
            _p: core::marker::PhantomData,
        };
        host.prime().await?;
        Ok(host)
    }

    /// The matched device.
    pub fn matched(&self) -> &OverbridgeMatch {
        &self.m
    }

    async fn select_alternates(
        control: &mut A::Pipe<pipe::Control, pipe::InOut>,
    ) -> Result<(), OverbridgeError> {
        for interface in [proto::IN_INTERFACE, proto::OUT_INTERFACE] {
            let setup = SetupPacket {
                request_type: RequestType {
                    direction: Direction::Out,
                    control_type: ControlType::Standard,
                    recipient: Recipient::Interface,
                },
                request: REQ_SET_INTERFACE,
                value: proto::STREAM_ALT as u16,
                index: interface as u16,
                length: 0,
            };
            control
                .control_out(&setup.to_bytes(), &[])
                .await
                .map_err(OverbridgeError::SetupFailed)?;
        }
        Ok(())
    }

    /// Send one OUT block of `frames` (7 × ch_to, interleaved).
    async fn send(&mut self, silent: bool) -> Result<(), OverbridgeError> {
        let n = FRAMES_PER_BLOCK * self.m.ch_to;
        if silent {
            self.frames[..n].fill(0);
        }
        let len = proto::encode_to_device(
            self.m.format,
            self.m.ch_to,
            self.tx_counter,
            &self.frames[..n],
            &mut self.tx_block,
        );
        self.tx
            .request_out(&self.tx_block[..len], false)
            .await
            .map_err(OverbridgeError::Transport)?;
        self.tx_counter = self.tx_counter.wrapping_add(FRAMES_PER_BLOCK as u16);
        Ok(())
    }

    async fn prime(&mut self) -> Result<(), OverbridgeError> {
        for _ in 0..PRIME_BLOCKS {
            self.send(true).await?;
        }
        Ok(())
    }

    /// Receive one block into `h`, then send one block from it. `now_us` is local time, for
    /// the rate measurement.
    pub async fn pump_once<const RX: usize, const TX: usize>(
        &mut self,
        h: &OverbridgeHandle<RX, TX>,
        now_us: u64,
    ) -> Result<(), OverbridgeError> {
        let want = (self.m.ep_in.max_packet_size & 0x7FF) as usize;
        let got = self
            .rx
            .request_in(&mut self.rx_block[..want])
            .await
            .map_err(OverbridgeError::Transport)?;
        let n = FRAMES_PER_BLOCK * self.m.ch_from;
        match proto::decode_from_device(
            self.m.format,
            self.m.ch_from,
            &self.rx_block[..got],
            &mut self.frames[..n],
        ) {
            Ok(counter) => {
                self.framing_run = 0;
                let seen = self.rx_counter.observe(counter);
                h.push_rx(&self.frames[..n]);
                let ppm = self.rate.add(FRAMES_PER_BLOCK as u64, now_us);
                h.update_stats(|s| {
                    s.blocks_in = s.blocks_in.wrapping_add(1);
                    match seen {
                        proto::Counted::Missing(n) => {
                            s.counter_gaps = s.counter_gaps.saturating_add(n)
                        }
                        proto::Counted::Resync => {
                            s.counter_resyncs = s.counter_resyncs.saturating_add(1)
                        }
                    }
                    if let Some(ppm) = ppm {
                        s.rate_ratio_ppm = ppm;
                    }
                });
            }
            Err(_) => {
                self.framing_run += 1;
                h.update_stats(|s| s.framing_errors = s.framing_errors.saturating_add(1));
                if self.framing_run >= FRAMING_LIMIT {
                    return Err(OverbridgeError::Framing);
                }
            }
        }
        // One block out per block in, even after a bad one: the device's clock paces both.
        let n = FRAMES_PER_BLOCK * self.m.ch_to;
        h.pull_tx(&mut self.frames[..n]);
        self.send(false).await?;
        h.update_stats(|s| s.blocks_out = s.blocks_out.wrapping_add(1));
        Ok(())
    }

    /// Recover from a transport error: select the alternates again, reset the data toggles
    /// and the counter, and prime the OUT stream.
    pub async fn restart(&mut self) -> Result<(), OverbridgeError> {
        Self::select_alternates(&mut self.control).await?;
        self.rx.reset_data_toggle();
        self.tx.reset_data_toggle();
        self.rx_counter.reset();
        self.framing_run = 0;
        self.rate = RateMeter::default();
        self.prime().await
    }
}
#[cfg(all(test, not(target_os = "none")))]
mod tests {
    extern crate std;
    use super::*;
    use crate::host::mock::{MockAlloc, MockState};
    use crate::overbridge::stems::OverbridgeHandle;
    use embassy_futures::block_on;
    use std::vec::Vec;

    /// The Digitakt's Overbridge configuration descriptor (OS 1.54; spec Appendix A).
    const DIGITAKT_CONFIG: [u8; 263] = [
        0x09, 0x02, 0x07, 0x01, 0x06, 0x01, 0x00, 0xc0, 0x00, 0x08, 0x0b, 0x00, 0x04, 0xff, 0x00,
        0x00, 0x00, 0x09, 0x04, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x09, 0x04, 0x01, 0x00,
        0x00, 0xff, 0x00, 0x00, 0x00, 0x09, 0x04, 0x01, 0x01, 0x01, 0xff, 0x00, 0x00, 0x00, 0x07,
        0x05, 0x83, 0x03, 0x20, 0x00, 0x01, 0x09, 0x04, 0x01, 0x02, 0x01, 0xff, 0x00, 0x00, 0x00,
        0x07, 0x05, 0x83, 0x03, 0x58, 0x00, 0x01, 0x09, 0x04, 0x01, 0x03, 0x01, 0xff, 0x00, 0x00,
        0x00, 0x07, 0x05, 0x83, 0x03, 0x70, 0x01, 0x01, 0x09, 0x04, 0x02, 0x00, 0x00, 0xff, 0x00,
        0x00, 0x00, 0x09, 0x04, 0x02, 0x01, 0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x03, 0x03,
        0x20, 0x00, 0x01, 0x09, 0x04, 0x02, 0x02, 0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05, 0x03,
        0x03, 0x58, 0x00, 0x01, 0x09, 0x04, 0x02, 0x03, 0x01, 0xff, 0x00, 0x00, 0x00, 0x07, 0x05,
        0x03, 0x03, 0x58, 0x00, 0x01, 0x09, 0x04, 0x03, 0x00, 0x02, 0xff, 0x00, 0x00, 0x00, 0x07,
        0x05, 0x02, 0x02, 0x00, 0x02, 0x00, 0x07, 0x05, 0x82, 0x03, 0x40, 0x00, 0x01, 0x08, 0x0b,
        0x04, 0x02, 0x01, 0x01, 0x00, 0x00, 0x09, 0x04, 0x04, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00,
        0x09, 0x24, 0x01, 0x00, 0x01, 0x09, 0x00, 0x01, 0x05, 0x09, 0x04, 0x05, 0x00, 0x02, 0x01,
        0x03, 0x00, 0x00, 0x07, 0x24, 0x01, 0x00, 0x01, 0x41, 0x00, 0x06, 0x24, 0x02, 0x01, 0x01,
        0x00, 0x06, 0x24, 0x02, 0x02, 0x02, 0x00, 0x09, 0x24, 0x03, 0x01, 0x03, 0x01, 0x02, 0x01,
        0x00, 0x09, 0x24, 0x03, 0x02, 0x04, 0x01, 0x01, 0x01, 0x00, 0x09, 0x05, 0x01, 0x02, 0x00,
        0x02, 0x00, 0x00, 0x00, 0x05, 0x25, 0x01, 0x01, 0x01, 0x09, 0x05, 0x81, 0x02, 0x00, 0x02,
        0x00, 0x00, 0x00, 0x05, 0x25, 0x01, 0x01, 0x03,
    ];

    fn cfg(bytes: &[u8]) -> ConfigurationDescriptor<'_> {
        ConfigurationDescriptor::try_from_slice(bytes).unwrap()
    }

    /// A config with only interfaces 1 and 2, alt 3 interrupt endpoints of `in_mps`/`out_mps`.
    fn stream_config(in_mps: u16, out_mps: u16) -> Vec<u8> {
        let mut b = std::vec![0x09, 0x02, 0, 0, 2, 1, 0, 0xc0, 0];
        for (iface, ep, mps) in [(1u8, 0x83u8, in_mps), (2, 0x03, out_mps)] {
            b.extend_from_slice(&[9, 4, iface, 0, 0, 0xff, 0, 0, 0]);
            b.extend_from_slice(&[9, 4, iface, 3, 1, 0xff, 0, 0, 0]);
            b.extend_from_slice(&[7, 5, ep, 0x03, mps as u8, (mps >> 8) as u8, 1]);
        }
        let len = b.len() as u16;
        b[2..4].copy_from_slice(&len.to_le_bytes());
        b
    }

    /// A Digitakt from-device block: counter `counter`, sample (f, c) = (counter + f) * 16 + c.
    fn digitakt_block(counter: u16) -> heapless::Vec<u8, 1024> {
        let mut frames = [0i32; 7 * 12];
        for (i, s) in frames.iter_mut().enumerate() {
            *s = ((counter as i32 + (i / 12) as i32) * 16 + (i % 12) as i32) << 8;
        }
        let mut block = [0u8; 368];
        proto::encode_from_device(Format::V2, 12, counter, &frames, &mut block);
        heapless::Vec::from_slice(&block).unwrap()
    }

    fn digitakt(state: &'static MockState) -> OverbridgeHost<'static, MockAlloc> {
        let alloc = MockAlloc::new(state);
        block_on(OverbridgeHost::try_register(
            &alloc,
            1,
            None,
            0x1935,
            0x0B2C,
            &cfg(&DIGITAKT_CONFIG),
        ))
        .unwrap()
    }

    #[test]
    fn the_digitakt_matches_with_12_and_2_channels() {
        let m = match_device(0x1935, 0x0B2C, &cfg(&DIGITAKT_CONFIG)).unwrap();
        assert_eq!(
            (m.name, m.format, m.ch_from, m.ch_to),
            ("Digitakt", Format::V2, 12, 2)
        );
        assert_eq!(
            (m.ep_in.endpoint_address, m.ep_in.max_packet_size),
            (0x83, 368)
        );
        assert_eq!(
            (m.ep_out.endpoint_address, m.ep_out.max_packet_size),
            (0x03, 88)
        );
    }

    #[test]
    fn another_vendor_does_not_match() {
        assert!(matches!(
            match_device(0x1234, 0x0B2C, &cfg(&DIGITAKT_CONFIG)),
            Err(OverbridgeError::NotOverbridge)
        ));
    }

    #[test]
    fn an_elektron_device_without_alt_3_does_not_match() {
        // Audio/MIDI mode: only a MIDIStreaming interface.
        let b = [
            0x09, 0x02, 25, 0, 1, 1, 0, 0xc0, 0, 9, 4, 0, 0, 1, 0x01, 0x03, 0, 0, 7, 5, 0x81, 0x02,
            0, 2, 0,
        ];
        assert!(matches!(
            match_device(0x1935, 0x102C, &cfg(&b)),
            Err(OverbridgeError::NotOverbridge)
        ));
    }

    #[test]
    fn a_digitakt_ii_matches_as_v2_1_with_42_and_8_channels() {
        let b = stream_config(1012, 256);
        let m = match_device(0x1935, 0x0B2B, &cfg(&b)).unwrap();
        assert_eq!(
            (m.name, m.format, m.ch_from, m.ch_to),
            ("Digitakt II", Format::V2_1, 42, 8)
        );
    }

    #[test]
    fn an_unlisted_elektron_device_is_driven_as_v2_from_its_sizes() {
        let b = stream_config(32 + 7 * 16, 32 + 7 * 8);
        let m = match_device(0x1935, 0x0BEE, &cfg(&b)).unwrap();
        assert_eq!((m.format, m.ch_from, m.ch_to), (Format::V2, 4, 2));
    }

    #[test]
    fn a_non_integral_block_size_is_refused() {
        let b = stream_config(369, 88);
        assert!(matches!(
            match_device(0x1935, 0x0B2C, &cfg(&b)),
            Err(OverbridgeError::BadBlockSize)
        ));
    }

    #[test]
    fn setup_selects_alt_3_on_both_interfaces_then_opens_pipes_and_primes_out() {
        let state = MockState::leak();
        let _h = digitakt(state);
        let setups = state.setups.borrow();
        assert_eq!(
            &setups[..],
            &[
                [0x01, 0x0B, 3, 0, 1, 0, 0, 0],
                [0x01, 0x0B, 3, 0, 2, 0, 0, 0]
            ]
        );
        let allocs = state.allocs.borrow();
        let data: Vec<_> = allocs
            .iter()
            .filter(|a| a.ep_addr != 0)
            .map(|a| (a.ep_addr, a.max_packet_size, a.setups_before))
            .collect();
        assert_eq!(
            data,
            [(0x83, 368, 2), (0x03, 88, 2)],
            "pipes open after both alternates are selected"
        );
        let sent = state.log.borrow();
        assert_eq!(
            sent.sent.len(),
            2 * 88,
            "two silent blocks prime the OUT stream"
        );
        assert_eq!(&sent.sent[..4], &[0x07, 0xFF, 0x00, 0x00]);
        assert_eq!(&sent.sent[88..92], &[0x07, 0xFF, 0x00, 0x07]);
        assert!(sent.sent[32..88].iter().all(|&b| b == 0));
    }

    #[test]
    fn streamed_blocks_reach_the_handle_in_order_and_out_follows_in() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        for c in [0u16, 7, 14] {
            state
                .script
                .borrow_mut()
                .reads
                .push(digitakt_block(c))
                .unwrap();
        }
        h.write(&[0x0100_0000, -0x0100_0000]);
        for t in 0..3 {
            block_on(host.pump_once(&h, t * 146)).unwrap();
        }
        let mut out = [0i32; 21 * 12];
        assert_eq!(h.read(&mut out), 21);
        for f in 0..21 {
            for c in 0..12 {
                assert_eq!(
                    out[f * 12 + c],
                    ((f as i32 * 16) + c as i32) << 8,
                    "frame {f} channel {c}"
                );
            }
        }
        let s = h.stats();
        assert_eq!(
            (s.blocks_in, s.blocks_out, s.counter_gaps, s.framing_errors),
            (3, 3, 0, 0)
        );
        let sent = &state.log.borrow().sent;
        assert_eq!(sent.len(), 5 * 88, "2 priming + 3 paced blocks");
        let first = &sent[2 * 88..3 * 88];
        assert_eq!(
            &first[..4],
            &[0x07, 0xFF, 0x00, 0x0E],
            "counter continues after priming"
        );
        assert_eq!(
            &first[32..40],
            &[0x01, 0, 0, 0, 0xFF, 0, 0, 0],
            "the app's frame goes out first"
        );
    }

    #[test]
    fn a_counter_gap_is_counted() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        for c in [0u16, 7, 28] {
            state
                .script
                .borrow_mut()
                .reads
                .push(digitakt_block(c))
                .unwrap();
        }
        for t in 0..3 {
            block_on(host.pump_once(&h, t)).unwrap();
        }
        assert_eq!(h.stats().counter_gaps, 2);
    }

    #[test]
    fn a_short_block_is_a_framing_error_and_out_still_runs() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        state
            .script
            .borrow_mut()
            .reads
            .push(heapless::Vec::from_slice(&[0x07, 0x00, 0, 0]).unwrap())
            .unwrap();
        // An empty script makes the mock return a zero-length read.
        block_on(host.pump_once(&h, 0)).unwrap();
        block_on(host.pump_once(&h, 1)).unwrap();
        let s = h.stats();
        assert_eq!((s.blocks_in, s.framing_errors, s.blocks_out), (0, 2, 2));
        assert_eq!(state.log.borrow().sent.len(), 4 * 88);
    }

    #[test]
    fn sixty_four_framing_errors_in_a_row_are_a_transport_error() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        for t in 0..63 {
            block_on(host.pump_once(&h, t)).unwrap();
        }
        assert!(matches!(
            block_on(host.pump_once(&h, 63)),
            Err(OverbridgeError::Framing)
        ));
    }

    #[test]
    fn a_pipe_error_is_a_transport_error() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        state.script.borrow_mut().read_err = Some(PipeError::BadResponse);
        assert!(matches!(
            block_on(host.pump_once(&h, 0)),
            Err(OverbridgeError::Transport(PipeError::BadResponse))
        ));
    }

    #[test]
    fn the_rate_ratio_is_measured_over_a_second() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<65536, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        // 6858 blocks = 48 006 frames in 1 000 000 µs: +125 ppm.
        let blocks = 6858u64;
        for b in 0..=blocks {
            if state.script.borrow().reads.is_empty() {
                for k in 0..64u16 {
                    state
                        .script
                        .borrow_mut()
                        .reads
                        .push(digitakt_block(((b as u16).wrapping_add(k)).wrapping_mul(7)))
                        .unwrap();
                }
            }
            block_on(host.pump_once(&h, b * 1_000_000 / blocks)).unwrap();
            let mut sink = [0i32; 7 * 12];
            h.read(&mut sink);
            state.log.borrow_mut().sent.clear();
        }
        assert_eq!(h.stats().rate_ratio_ppm, 125);
    }

    #[test]
    fn restart_selects_the_alternates_again_and_reprimes() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        block_on(host.restart()).unwrap();
        assert_eq!(state.setups.borrow().len(), 4);
        assert_eq!(state.log.borrow().sent.len(), 4 * 88);
        assert_eq!(state.log.borrow().toggle_resets, 2);
    }

    /// An outage longer than gap counting can follow is still visible.
    #[test]
    fn a_long_outage_counts_as_a_resync() {
        let state = MockState::leak();
        let mut host = digitakt(state);
        let h: OverbridgeHandle<4096, 512> = OverbridgeHandle::new();
        h.begin(host.matched().info(), 12, 2).unwrap();
        for c in [0u16, 7, 7 * 2000] {
            state
                .script
                .borrow_mut()
                .reads
                .push(digitakt_block(c))
                .unwrap();
        }
        for t in 0..3 {
            block_on(host.pump_once(&h, t)).unwrap();
        }
        let s = h.stats();
        assert_eq!((s.counter_gaps, s.counter_resyncs), (0, 1));
    }

    /// A block over the 1024-byte high-speed limit is refused, not handed to a 1 KB buffer.
    #[test]
    fn a_block_over_1024_bytes_is_refused() {
        let b = stream_config(32 + 7 * 4 * 36, 88);
        assert!(matches!(
            match_device(0x1935, 0x0BEE, &cfg(&b)),
            Err(OverbridgeError::BadBlockSize)
        ));
    }

    /// An odd descriptor with many endpoints in alt 3 is searched, not overflowed.
    #[test]
    fn many_endpoints_in_alt_3_do_not_panic() {
        let mut b = std::vec![0x09, 0x02, 0, 0, 2, 1, 0, 0xc0, 0];
        b.extend_from_slice(&[9, 4, 1, 3, 6, 0xff, 0, 0, 0]);
        for ep in [0x84u8, 0x85, 0x86, 0x87, 0x88] {
            b.extend_from_slice(&[7, 5, ep, 0x02, 0, 2, 0]);
        }
        b.extend_from_slice(&[7, 5, 0x83, 0x03, 0x70, 0x01, 1]);
        b.extend_from_slice(&[9, 4, 2, 3, 1, 0xff, 0, 0, 0, 7, 5, 0x03, 0x03, 88, 0, 1]);
        let len = b.len() as u16;
        b[2..4].copy_from_slice(&len.to_le_bytes());
        let m = match_device(0x1935, 0x0B2C, &cfg(&b)).unwrap();
        assert_eq!(m.ep_in.endpoint_address, 0x83);
    }
}
