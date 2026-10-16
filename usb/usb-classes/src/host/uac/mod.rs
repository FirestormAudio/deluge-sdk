//! USB **host**-side USB Audio Class (UAC2) capture driver.
//!
//! Records multichannel audio from a class-compliant USB audio interface, and
//! plays back to it too when the device is full-duplex (iso OUT, built on the
//! primitives in [`out`]). Mirrors [`super::midi`]: generic over the allocator
//! so it unit-tests against [`super::mock`].

pub mod desc;
pub mod out;
pub mod pcm;
pub mod resample;
pub mod ring;

/// Widest capture channel count this driver supports. Bounds the fixed-size
/// per-frame scratch and the ring; a device declaring more is rejected.
pub const MAX_CHANNELS: usize = 8;

use embassy_usb_driver::Direction;
use embassy_usb_driver::EndpointType;
use embassy_usb_driver::host::{HostError, PipeError, SplitInfo, UsbHostAllocator, UsbPipe, pipe};
use embassy_usb_host::class::uac::codes;
use embassy_usb_host::control::{ControlType, Recipient, RequestType, SetupPacket};
use embassy_usb_host::descriptor::{ConfigurationDescriptor, EndpointDescriptor};

use self::desc::{AudioFunction, ClockEntity};
use self::out::PlaybackResampler;
use self::pcm::SampleFormat;
use self::resample::{PiController, Resampler};
use self::ring::SampleRing;

/// Errors from the host UAC capture driver.
#[derive(Debug)]
pub enum UacError {
    /// No UAC2 audio-streaming interface with an input (capture) endpoint.
    NoInputInterface,
    /// Found capture, but no alternate setting this driver can stream: Type-I PCM in a 2-,
    /// 3- or 4-byte subslot, 1..=MAX_CHANNELS channels, packets of at most 1024 bytes.
    UnsupportedFormat,
    /// The device cannot run the requested sample rate.
    UnsupportedRate,
    /// A required pipe could not be allocated.
    NoPipe(HostError),
    /// A control or data transfer failed.
    Transfer(PipeError),
    /// The device's descriptors could not be parsed as UAC2.
    BadDescriptors,
}

impl From<PipeError> for UacError {
    fn from(e: PipeError) -> Self {
        UacError::Transfer(e)
    }
}

/// Decode one signed 24-bit little-endian sample (`b.len() == 3`) to `f32` in
/// `[-1.0, 1.0)`.
pub fn decode_s24le(b: &[u8]) -> f32 {
    let raw = (b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16);
    let signed = (raw << 8) >> 8; // sign-extend 24 -> 32
    signed as f32 / 8_388_608.0 // 2^23
}

/// A matched UAC2 capture interface and the facts needed to negotiate + stream.
#[derive(Clone, Copy, Debug)]
pub struct UacCaptureMatch {
    /// AudioControl interface number (`wIndex` low byte for clock requests).
    pub ac_interface: u8,
    /// AudioStreaming interface number (target of SET_INTERFACE).
    pub streaming_interface: u8,
    /// Alternate setting carrying the stream (SET_INTERFACE `wValue`).
    pub alternate_setting: u8,
    /// The isochronous IN data endpoint.
    pub endpoint: EndpointDescriptor,
    /// Channels the device declares (app-visible; no stereo assumption).
    pub num_channels: u8,
    /// Sample encoding on the wire.
    pub format: SampleFormat,
    /// Clock entity feeding the stream's terminal: a clock source, or a selector or
    /// multiplier that registration follows to one.
    pub clock_id: u8,
}

fn capture_match(f: &AudioFunction) -> Result<UacCaptureMatch, UacError> {
    let Some(s) = f.best_stream(Direction::In) else {
        return Err(if f.has_stream(Direction::In) {
            UacError::UnsupportedFormat
        } else {
            UacError::NoInputInterface
        });
    };
    // `best_stream` only returns settings with an endpoint and a format.
    let (Some(endpoint), Some(format)) = (s.endpoint, s.format) else {
        return Err(UacError::BadDescriptors);
    };
    Ok(UacCaptureMatch {
        ac_interface: f.ac_interface,
        streaming_interface: s.interface,
        alternate_setting: s.alternate_setting,
        endpoint,
        num_channels: s.channels,
        format,
        clock_id: f
            .terminal_clock(s.terminal_link)
            .ok_or(UacError::BadDescriptors)?,
    })
}

/// Find the best usable UAC2 capture alternate setting in `cfg` (see
/// [`AudioFunction::best_stream`]). Returns [`UacError::NoInputInterface`] when the device
/// streams nothing in, [`UacError::UnsupportedFormat`] when it does but in no setting this
/// driver can stream.
pub fn find_uac_capture(cfg: &ConfigurationDescriptor<'_>) -> Result<UacCaptureMatch, UacError> {
    capture_match(&desc::parse_audio_function(cfg)?)
}

/// A matched UAC2 playback (host->device) streaming interface.
#[derive(Clone, Copy, Debug)]
pub struct UacPlaybackMatch {
    /// AudioStreaming interface number (target of SET_INTERFACE).
    pub streaming_interface: u8,
    /// Alternate setting carrying the stream.
    pub alternate_setting: u8,
    /// The isochronous OUT data endpoint.
    pub endpoint: EndpointDescriptor,
    /// Channels the device declares for playback (independent of capture).
    pub num_channels: u8,
    /// Sample encoding on the wire.
    pub format: SampleFormat,
}

fn playback_match(f: &AudioFunction) -> Option<UacPlaybackMatch> {
    let s = f.best_stream(Direction::Out)?;
    Some(UacPlaybackMatch {
        streaming_interface: s.interface,
        alternate_setting: s.alternate_setting,
        endpoint: s.endpoint?,
        num_channels: s.channels,
        format: s.format?,
    })
}

/// Find the best usable UAC2 playback alternate setting in `cfg`. `None` if the device has
/// no playback (capture-only devices are valid) or none this driver can stream.
pub fn find_uac_playback(cfg: &ConfigurationDescriptor<'_>) -> Option<UacPlaybackMatch> {
    playback_match(&desc::parse_audio_function(cfg).ok()?)
}

/// USB standard SET_INTERFACE request code (USB 2.0 §9.4).
const REQ_SET_INTERFACE: u8 = 11;

/// Sampling-frequency SET_CUR/GET_CUR wValue: SAMPLING_FREQ_CONTROL in the high
/// byte (the `codes` constant is pre-shifted), channel number 0 (master).
const SAMPLING_FREQ_WVALUE: u16 = codes::control_selector::clock_source::SAMPLING_FREQ_CONTROL;

/// Clock selector CUR wValue: CX_CLOCK_SELECTOR_CONTROL (1) in the high byte.
const CLOCK_SELECTOR_WVALUE: u16 = 0x01 << 8;

/// Device sample rates the pumps and resamplers handle.
const DEVICE_RATES: core::ops::RangeInclusive<u32> = 8_000..=192_000;

/// A hosted UAC2 capture device.
///
/// Owns the control pipe and the isochronous IN data pipe. Generic over the
/// allocator so it unit-tests against [`super::mock`].
/// Playback (host->device) state, present only for a full-duplex device.
struct PlaybackState<'d, A: UsbHostAllocator<'d>> {
    iso_out: A::Pipe<pipe::Isochronous, pipe::Out>,
    ring: SampleRing,
    resampler: PlaybackResampler,
    channels: u8,
    format: SampleFormat,
    max_packet: u16,
    _p: core::marker::PhantomData<&'d ()>,
}

pub struct Uac<'d, A: UsbHostAllocator<'d>> {
    control: A::Pipe<pipe::Control, pipe::InOut>,
    iso_in: A::Pipe<pipe::Isochronous, pipe::In>,
    num_channels: u8,
    format: SampleFormat,
    /// The device's sample rate (the stream's clock source).
    rate: u32,
    max_packet: u16,
    ring: SampleRing,
    resampler: Resampler,
    pi: PiController,
    r: f32,
    /// Playback path; `None` on a capture-only device.
    playback: Option<PlaybackState<'d, A>>,
    /// Isochronous transfers lost, both directions.
    transfer_errors: u32,
    _p: core::marker::PhantomData<&'d ()>,
}

impl<'d, A: UsbHostAllocator<'d>> Uac<'d, A> {
    /// Channels this device declared. `read`'s output length should be a
    /// multiple of this.
    pub fn channels(&self) -> u8 {
        self.num_channels
    }

    /// Isochronous transfers lost so far, capture and playback together.
    pub fn transfer_errors(&self) -> u32 {
        self.transfer_errors
    }

    /// Count a lost isochronous transfer, warning on the 1st, 2nd, 4th, 8th... so a
    /// persistent fault (a packet per microframe) cannot flood the log.
    fn lost(&mut self, dir: &str, e: PipeError) {
        self.transfer_errors = self.transfer_errors.saturating_add(1);
        if self.transfer_errors.is_power_of_two() {
            log::warn!(
                "uac: iso {} transfer failed ({:?}); {} lost so far",
                dir,
                e,
                self.transfer_errors
            );
        }
    }

    /// The device's sample rate. When it differs from the engine's, the resamplers convert.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Playback channels the device declared, or 0 if capture-only.
    pub fn playback_channels(&self) -> u8 {
        self.playback.as_ref().map(|p| p.channels).unwrap_or(0)
    }

    /// Match, claim pipes, negotiate `sample_rate_hz` on the clock source, and select the
    /// streaming alt.
    pub async fn try_register(
        alloc: &A,
        addr: u8,
        split: Option<SplitInfo>,
        cfg: &ConfigurationDescriptor<'_>,
        sample_rate_hz: u32,
    ) -> Result<Self, UacError> {
        let f = desc::parse_audio_function(cfg)?;
        let m = capture_match(&f)?;

        // Control pipe (EP0) + the iso IN data pipe.
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
            .map_err(UacError::NoPipe)?;
        let iso_in = alloc
            .alloc_pipe::<pipe::Isochronous, pipe::In>(addr, &m.endpoint.into(), split)
            .map_err(UacError::NoPipe)?;

        // Ask the clock source for `sample_rate_hz` when the host may set it; a fixed clock,
        // or one without that rate, runs at its own rate and the resamplers convert.
        log::debug!(
            "uac: capture IF{} alt {} {:?} {} ch, EP {:#04x} mps {}; clock entity {}",
            m.streaming_interface,
            m.alternate_setting,
            m.format,
            m.num_channels,
            m.endpoint.endpoint_address,
            m.endpoint.max_packet_size,
            m.clock_id
        );
        let (clock, programmable) = Self::resolve_clock(&mut control, &f, m.clock_id)
            .await
            .inspect_err(|e| log::debug!("uac: clock {} unresolved: {:?}", m.clock_id, e))?;
        let mut rate = Self::get_sampling_freq(&mut control, m.ac_interface, clock)
            .await
            .inspect_err(|e| log::debug!("uac: GET CUR rate on clock {} failed: {:?}", clock, e))?;
        log::debug!(
            "uac: clock source {} at {} Hz (programmable: {})",
            clock,
            rate,
            programmable
        );
        if rate != sample_rate_hz && programmable {
            if let Err(e) =
                Self::set_sampling_freq(&mut control, m.ac_interface, clock, sample_rate_hz).await
            {
                log::warn!("uac: SET CUR {} Hz failed ({:?})", sample_rate_hz, e);
            }
            rate = Self::get_sampling_freq(&mut control, m.ac_interface, clock).await?;
        }
        if !DEVICE_RATES.contains(&rate) {
            return Err(UacError::UnsupportedRate);
        }

        // Optional playback (full-duplex). Absent on capture-only devices; the
        // clock is shared across directions, so no additional SET_CUR is needed.
        let playback = match playback_match(&f) {
            Some(p) => {
                log::debug!(
                    "uac: playback IF{} alt {} {:?} {} ch, EP {:#04x} mps {}",
                    p.streaming_interface,
                    p.alternate_setting,
                    p.format,
                    p.num_channels,
                    p.endpoint.endpoint_address,
                    p.endpoint.max_packet_size
                );
                Self::set_interface(&mut control, p.streaming_interface, p.alternate_setting)
                    .await
                    .inspect_err(|e| log::debug!("uac: SET_INTERFACE playback failed: {:?}", e))?;
                // Claim the pipe only now: the HAL arms an isochronous OUT pipe as it opens it,
                // and the device must not get packets for an endpoint it has not enabled.
                let iso_out = alloc
                    .alloc_pipe::<pipe::Isochronous, pipe::Out>(addr, &p.endpoint.into(), split)
                    .map_err(UacError::NoPipe)?;
                let mut pring = SampleRing::new();
                pring.reset(p.num_channels as usize);
                let mut presampler = PlaybackResampler::new();
                presampler.reset(p.num_channels as usize);
                Some(PlaybackState {
                    iso_out,
                    ring: pring,
                    resampler: presampler,
                    channels: p.num_channels,
                    format: p.format,
                    max_packet: p.endpoint.max_packet_size,
                    _p: core::marker::PhantomData,
                })
            }
            None => None,
        };

        // Activate capture last (alt 0 is zero-bandwidth; the real stream is 1+): the pump
        // starts polling it next, and no control request comes between.
        Self::set_interface(&mut control, m.streaming_interface, m.alternate_setting)
            .await
            .inspect_err(|e| log::debug!("uac: SET_INTERFACE capture failed: {:?}", e))?;

        let mut ring = SampleRing::new();
        ring.reset(m.num_channels as usize);
        let mut resampler = Resampler::new();
        resampler.reset(m.num_channels as usize);
        let setpoint = (ring.capacity_frames() / 2) as f32;
        let nominal = rate as f32 / sample_rate_hz as f32;
        Ok(Self {
            control,
            iso_in,
            num_channels: m.num_channels,
            format: m.format,
            rate,
            max_packet: m.endpoint.max_packet_size,
            ring,
            resampler,
            pi: PiController::with_nominal(setpoint, nominal),
            r: nominal,
            playback,
            transfer_errors: 0,
            _p: core::marker::PhantomData,
        })
    }

    /// Follow clock entity `id` through selectors (at their current input; the first when
    /// the selector does not say) and multipliers to a clock source. Returns the source's id
    /// and whether the host may set its frequency.
    async fn resolve_clock(
        control: &mut A::Pipe<pipe::Control, pipe::InOut>,
        f: &AudioFunction,
        mut id: u8,
    ) -> Result<(u8, bool), UacError> {
        for _ in 0..desc::MAX_CLOCKS {
            match f.clock(id) {
                Some(ClockEntity::Source { programmable, .. }) => return Ok((id, *programmable)),
                Some(ClockEntity::Selector { inputs, .. }) => {
                    let pin = Self::get_selector(control, f.ac_interface, id)
                        .await
                        .unwrap_or(0);
                    let input = (pin as usize).checked_sub(1).and_then(|i| inputs.get(i));
                    id = *input.or(inputs.first()).ok_or(UacError::BadDescriptors)?;
                }
                Some(ClockEntity::Multiplier { source, .. }) => id = *source,
                None => return Err(UacError::BadDescriptors),
            }
        }
        // A cycle, or a chain longer than the entities parsed.
        Err(UacError::BadDescriptors)
    }

    /// GET CUR on clock selector `id`: its current input pin (1-based).
    async fn get_selector(
        control: &mut A::Pipe<pipe::Control, pipe::InOut>,
        ac_interface: u8,
        id: u8,
    ) -> Result<u8, UacError> {
        let setup = SetupPacket {
            request_type: RequestType {
                direction: Direction::In,
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
            },
            request: codes::request_code::CUR,
            value: CLOCK_SELECTOR_WVALUE,
            index: (id as u16) << 8 | ac_interface as u16,
            length: 1,
        };
        let mut buf = [0u8; 1];
        match control.control_in(&setup.to_bytes(), &mut buf).await? {
            0 => Err(UacError::Transfer(PipeError::BadResponse)),
            _ => Ok(buf[0]),
        }
    }

    async fn get_sampling_freq(
        control: &mut A::Pipe<pipe::Control, pipe::InOut>,
        ac_interface: u8,
        clock_id: u8,
    ) -> Result<u32, UacError> {
        let setup = SetupPacket {
            request_type: RequestType {
                direction: Direction::In,
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
            },
            request: codes::request_code::CUR,
            value: SAMPLING_FREQ_WVALUE,
            index: (clock_id as u16) << 8 | ac_interface as u16,
            length: 4,
        };
        let mut buf = [0u8; 4];
        let n = control.control_in(&setup.to_bytes(), &mut buf).await?;
        if n < 4 {
            return Err(UacError::Transfer(PipeError::BadResponse));
        }
        Ok(u32::from_le_bytes(buf))
    }

    async fn set_sampling_freq(
        control: &mut A::Pipe<pipe::Control, pipe::InOut>,
        ac_interface: u8,
        clock_id: u8,
        freq: u32,
    ) -> Result<(), UacError> {
        let setup = SetupPacket {
            request_type: RequestType {
                direction: Direction::Out,
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
            },
            request: codes::request_code::CUR,
            value: SAMPLING_FREQ_WVALUE,
            index: (clock_id as u16) << 8 | ac_interface as u16,
            length: 4,
        };
        control
            .control_out(&setup.to_bytes(), &freq.to_le_bytes())
            .await?;
        Ok(())
    }

    async fn set_interface(
        control: &mut A::Pipe<pipe::Control, pipe::InOut>,
        interface: u8,
        alt: u8,
    ) -> Result<(), UacError> {
        let setup = SetupPacket {
            request_type: RequestType {
                direction: Direction::Out,
                control_type: ControlType::Standard,
                recipient: Recipient::Interface,
            },
            request: REQ_SET_INTERFACE,
            value: alt as u16,
            index: interface as u16,
            length: 0,
        };
        control.control_out(&setup.to_bytes(), &[]).await?;
        Ok(())
    }

    /// Do one isochronous IN transfer: decode → resample → ring, then update
    /// the drift ratio from the new ring fill.
    ///
    /// Iso has no retries: a failed or empty transfer logs and returns `Ok`
    /// (the ring absorbs the gap) — except [`PipeError::BadResponse`], which
    /// is returned as `Err` to end the stream. That is the RUSB1 HAL's detach
    /// signal: its DTCH handler (the `INTSTS1_DTCH` arm of `hcd_int_handler`)
    /// force-sets NRDY on every pipe, and `poll_pipe` maps NRDY to
    /// `BadResponse` (the HAL has no `PipeError::Disconnected`). Absorbing it
    /// would make the next `request_in` await a pipe the idle (UACT=0) bus
    /// never services, hanging the task with its pipes and address never
    /// freed.
    pub async fn pump_once(&mut self) -> Result<(), UacError> {
        let ch = self.num_channels as usize;
        let mut buf = [0u8; 1024]; // >= any HS iso mps
        let cap = (self.max_packet as usize).min(buf.len());
        let n = match self.iso_in.request_in(&mut buf[..cap]).await {
            Ok(n) => n,
            // BadResponse is the HAL's detach; Canceled its stopped port (no transfer
            // completes until the device is reset). Either ends the stream: absorbing
            // them spins this task and starves the reset that would restart the port.
            Err(e @ (PipeError::BadResponse | PipeError::Canceled)) => {
                // Detach (see doc comment above): propagate so the caller's
                // loop exits and drops the pipes.
                return Err(e.into());
            }
            Err(e) => {
                // Lost packet: ring absorbs it. Do not tear down the stream.
                self.lost("IN", e);
                self.r = self.pi.update(self.ring.fill_frames() as f32);
                return Ok(());
            }
        };
        let b = self.format.bytes();
        let frame_bytes = ch * b;
        let mut frame = [0.0f32; MAX_CHANNELS];
        for chunk in buf[..n].chunks_exact(frame_bytes) {
            for c in 0..ch {
                frame[c] = self.format.decode(&chunk[c * b..c * b + b]);
            }
            let ring = &mut self.ring;
            self.resampler
                .feed(&frame[..ch], self.r, |o| ring.push_frame(o));
        }
        self.r = self.pi.update(self.ring.fill_frames() as f32);
        // Full-duplex: emit one playback packet per capture tick (this cycle's
        // device-frame count), paced by the same clock. No-op if capture-only.
        let frames = n / frame_bytes;
        self.send_playback(frames).await;
        Ok(())
    }

    /// Emit `frames` device-rate playback frames on the iso OUT endpoint,
    /// resampled engine->device at `step = 1.0/self.r` (one shared device
    /// clock) and encoded in the playback format. No-op on a capture-only device.
    /// Underrun is silence (handled in `PlaybackResampler`); a failed OUT
    /// transfer is logged + absorbed (detach is governed by the capture side).
    async fn send_playback(&mut self, frames: usize) {
        if frames == 0 || self.playback.is_none() {
            return;
        }
        let step = if self.r > 0.0 { 1.0 / self.r } else { 1.0 };
        let pb = self.playback.as_mut().unwrap();
        let ch = pb.channels as usize;
        let b = pb.format.bytes();
        let frame_bytes = ch * b;
        let mut buf = [0u8; 1024];
        let max_by_buf = buf.len() / frame_bytes;
        let max_by_pkt = (pb.max_packet as usize) / frame_bytes;
        // Clamped to one OUT packet: "emit == captured frame count" holds while
        // `frames <= max_by_pkt`. If capture ever delivers more frames than one
        // playback packet holds, staging backs up and `playback_write` applies
        // backpressure (short write) — it never overflows.
        let to_send = frames.min(max_by_buf).min(max_by_pkt);
        let mut w = 0usize;
        let format = pb.format;
        pb.resampler.produce(&mut pb.ring, step, to_send, |frame| {
            for c in 0..ch {
                format.encode(frame[c], &mut buf[w..w + b]);
                w += b;
            }
        });
        if w == 0 {
            return;
        }
        let sent = pb.iso_out.request_out(&buf[..w], false).await;
        if let Err(e) = sent {
            self.lost("OUT", e);
        }
    }

    /// Drain up to `out.len()` samples of captured interleaved `f32`. Short
    /// return = underrun; never blocks. `out.len()` should be a multiple of
    /// `channels()`.
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        self.ring.read(out)
    }

    /// Frames currently buffered (for diagnostics / the validation firmware).
    pub fn fill_frames(&self) -> usize {
        self.ring.fill_frames()
    }

    /// Like [`Self::pump_once`], but publishes to the process-wide capture
    /// bridge so a different task (the engine) can `capture_read`. Used by the
    /// host task.
    ///
    /// Same detach handling as [`Self::pump_once`]: see its doc comment for
    /// why [`PipeError::BadResponse`] specifically propagates as `Err` while
    /// every other iso error is absorbed.
    #[cfg(target_os = "none")]
    pub async fn pump_once_shared(&mut self) -> Result<(), UacError> {
        let ch = self.num_channels as usize;
        let mut buf = [0u8; 1024];
        let cap = (self.max_packet as usize).min(buf.len());
        let n = match self.iso_in.request_in(&mut buf[..cap]).await {
            Ok(n) => n,
            // BadResponse is the HAL's detach; Canceled its stopped port (no transfer
            // completes until the device is reset). Either ends the stream: absorbing
            // them spins this task and starves the reset that would restart the port.
            Err(e @ (PipeError::BadResponse | PipeError::Canceled)) => {
                // Detach: propagate so the capture task exits and drops the
                // pipes (see `pump_once`'s doc comment).
                return Err(e.into());
            }
            Err(e) => {
                // Lost packet: the shared ring absorbs it; keep streaming.
                self.lost("IN", e);
                self.r = self.pi.update(shared::fill_frames() as f32);
                return Ok(());
            }
        };
        let b = self.format.bytes();
        let frame_bytes = ch * b;
        let mut frame = [0.0f32; MAX_CHANNELS];
        let mut out = [0.0f32; MAX_CHANNELS];
        for chunk in buf[..n].chunks_exact(frame_bytes) {
            for c in 0..ch {
                frame[c] = self.format.decode(&chunk[c * b..c * b + b]);
            }
            let mut emit = |o: &[f32]| {
                out[..o.len()].copy_from_slice(o);
                shared::push(&out[..o.len()], ch);
            };
            self.resampler.feed(&frame[..ch], self.r, &mut emit);
        }
        // Single source of truth: the shared ring the app actually drains.
        self.r = self.pi.update(shared::fill_frames() as f32);
        // Full-duplex playback: move app-written frames into staging (one short
        // lock), then emit this cycle's device-frame count. No-op on a
        // capture-only device.
        if let Some(pb) = self.playback.as_mut() {
            shared::playback_drain(&mut pb.ring);
        }
        let frames = n / frame_bytes;
        self.send_playback(frames).await;
        Ok(())
    }

    /// Test hook: enqueue one interleaved playback frame into the local staging
    /// ring (the host-tested `pump_once` playback source).
    #[cfg(all(test, not(target_os = "none")))]
    fn push_playback_frame(&mut self, f: &[f32]) {
        if let Some(pb) = self.playback.as_mut() {
            pb.ring.push_frame(f);
        }
    }
}

/// Cross-task capture bridge: the app-facing static that
/// [`Uac::pump_once_shared`] publishes into and [`capture_read`] drains.
///
/// Device-only: it uses a `CriticalSectionRawMutex` static, which only makes
/// sense once there is a real interrupt-driven executor. Host tests exercise
/// the correctness reference (`pump_once` + `self.ring`) directly instead.
#[cfg(target_os = "none")]
pub mod shared {
    use core::cell::RefCell;

    use embassy_sync::blocking_mutex::Mutex;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    use super::MAX_CHANNELS;
    use super::ring::SampleRing;

    struct Capture {
        ring: SampleRing,
        channels: u8,
        active: bool,
    }

    static CAPTURE: Mutex<CriticalSectionRawMutex, RefCell<Capture>> =
        Mutex::new(RefCell::new(Capture {
            ring: SampleRing::new(),
            channels: 0,
            active: false,
        }));

    /// Called by the capture task when a device is registered.
    pub fn begin(channels: u8) {
        CAPTURE.lock(|c| {
            let mut c = c.borrow_mut();
            let channels = channels.clamp(1, MAX_CHANNELS as u8);
            c.ring.reset(channels as usize);
            c.channels = channels;
            c.active = true;
        });
    }

    /// Called by the capture task on detach.
    pub fn end() {
        CAPTURE.lock(|c| {
            let mut c = c.borrow_mut();
            c.active = false;
            c.channels = 0;
            c.ring.reset(1);
        });
    }

    /// Push captured interleaved frames from the task.
    pub fn push(samples: &[f32], channels: usize) {
        CAPTURE.lock(|c| {
            let mut c = c.borrow_mut();
            for frame in samples.chunks_exact(channels) {
                c.ring.push_frame(frame);
            }
        });
    }

    /// Frames currently buffered — the PI controller's single source of truth.
    pub fn fill_frames() -> usize {
        CAPTURE.lock(|c| c.borrow().ring.fill_frames())
    }

    /// App-facing drain. Short return = underrun; never blocks.
    pub fn capture_read(out: &mut [f32]) -> usize {
        CAPTURE.lock(|c| c.borrow_mut().ring.read(out))
    }

    /// Channels the hosted device declared, or 0 if none.
    pub fn capture_channels() -> u8 {
        CAPTURE.lock(|c| c.borrow().channels)
    }

    // --- Playback (host->device) bridge ---

    struct Playback {
        ring: SampleRing,
        channels: u8,
        active: bool,
    }

    static PLAYBACK: Mutex<CriticalSectionRawMutex, RefCell<Playback>> =
        Mutex::new(RefCell::new(Playback {
            ring: SampleRing::new(),
            channels: 0,
            active: false,
        }));

    /// Called by the capture task when a full-duplex device is registered.
    pub fn playback_begin(channels: u8) {
        PLAYBACK.lock(|c| {
            let mut c = c.borrow_mut();
            let channels = channels.clamp(1, MAX_CHANNELS as u8);
            c.ring.reset(channels as usize);
            c.channels = channels;
            c.active = true;
        });
    }

    /// Called by the capture task on detach.
    pub fn playback_end() {
        PLAYBACK.lock(|c| {
            let mut c = c.borrow_mut();
            c.active = false;
            c.channels = 0;
            c.ring.reset(1);
        });
    }

    /// Move all available whole frames from the shared playback ring into `dst`
    /// (the pump's local staging ring) under ONE short critical section
    /// (bounded by ring capacity, no await). The resampler then pulls from
    /// `dst` outside the lock.
    pub fn playback_drain(dst: &mut SampleRing) {
        PLAYBACK.lock(|c| {
            let mut c = c.borrow_mut();
            let ch = c.channels.max(1) as usize;
            let mut frame = [0.0f32; MAX_CHANNELS];
            while c.ring.fill_frames() >= 1 && dst.fill_frames() < dst.capacity_frames() {
                c.ring.read(&mut frame[..ch]);
                dst.push_frame(&frame[..ch]);
            }
        });
    }

    /// App-facing: enqueue interleaved playback frames. Short return = ring
    /// full; never blocks.
    pub fn playback_write(samples: &[f32]) -> usize {
        PLAYBACK.lock(|c| {
            let mut c = c.borrow_mut();
            let ch = c.channels.max(1) as usize;
            let mut n = 0;
            for frame in samples.chunks_exact(ch) {
                if c.ring.fill_frames() >= c.ring.capacity_frames() {
                    break;
                }
                c.ring.push_frame(frame);
                n += ch;
            }
            n
        })
    }

    /// Playback channels the hosted device declared, or 0 if none / capture-only.
    pub fn playback_channels() -> u8 {
        PLAYBACK.lock(|c| c.borrow().channels)
    }
}

#[cfg(target_os = "none")]
pub use shared::{capture_channels, capture_read, playback_channels, playback_write};

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use heapless::Vec;

    // --- UAC2 descriptor fixture bytes ---
    // Class/subclass/protocol: AUDIO=0x01, AUDIOCONTROL=0x01, AUDIOSTREAMING=0x02,
    // protocol IP_VERSION_02_00=0x20. CS_INTERFACE=0x24. subslot_size at the
    // FORMAT_TYPE descriptor; num_channels in the AS general descriptor.

    fn uac2_mic_cfg(subslot: u8, channels: u8) -> Vec<u8, 256> {
        uac2_mic_cfg_mps(subslot, channels, 0x0126) // 294, the real HS iso mps
    }

    /// Full-duplex UAC2 device: capture AS interface (IN 0x81) + playback AS
    /// interface (OUT 0x02), sharing one clock source.
    fn uac2_full_duplex_cfg(subslot: u8, cap_channels: u8, play_channels: u8) -> Vec<u8, 256> {
        let mut b: Vec<u8, 256> = Vec::new();
        let push = |s: &[u8], b: &mut Vec<u8, 256>| b.extend_from_slice(s).unwrap();

        // IAD: first iface 0, count 3 (AC + capture AS + playback AS).
        push(&[8, 0x0B, 0, 3, 0x01, 0x00, 0x20, 0], &mut b);
        // Std AC interface 0.
        push(&[9, 0x04, 0, 0, 0, 0x01, 0x01, 0x20, 0], &mut b);
        // CS AC header.
        push(&[9, 0x24, 0x01, 0x00, 0x02, 30, 0, 0x00, 0x00], &mut b);
        // CS Clock Source id 0x09.
        push(&[8, 0x24, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x00], &mut b);
        // CS Input Terminal id 0x01, clock 0x09, cap_channels.
        #[rustfmt::skip]
        push(&[17, 0x24, 0x02, 0x01, 0x01, 0x02, 0, 0x09, cap_channels, 0, 0, 0, 0, 0, 0, 0, 0], &mut b);

        // Capture AS interface 1 (iso IN 0x81).
        push(&[9, 0x04, 1, 0, 0, 0x01, 0x02, 0x20, 0], &mut b);
        push(&[9, 0x04, 1, 1, 1, 0x01, 0x02, 0x20, 0], &mut b);
        #[rustfmt::skip]
        push(&[16, 0x24, 0x01, 0x01, 0x00, 0x01, 0x01, 0, 0, 0, cap_channels, 0, 0, 0, 0, 0], &mut b);
        push(&[6, 0x24, 0x02, 0x01, subslot, subslot * 8], &mut b);
        push(&[7, 0x05, 0x81, 0x01, 0x26, 0x01, 0x04], &mut b);
        push(&[8, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00], &mut b);

        // Playback AS interface 2 (iso OUT 0x02).
        push(&[9, 0x04, 2, 0, 0, 0x01, 0x02, 0x20, 0], &mut b);
        push(&[9, 0x04, 2, 1, 1, 0x01, 0x02, 0x20, 0], &mut b);
        #[rustfmt::skip]
        push(&[16, 0x24, 0x01, 0x01, 0x00, 0x01, 0x01, 0, 0, 0, play_channels, 0, 0, 0, 0, 0], &mut b);
        push(&[6, 0x24, 0x02, 0x01, subslot, subslot * 8], &mut b);
        push(&[7, 0x05, 0x02, 0x01, 0x26, 0x01, 0x04], &mut b);
        push(&[8, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00], &mut b);

        let total = (9 + b.len()) as u16;
        let mut cfg: Vec<u8, 256> = Vec::new();
        cfg.extend_from_slice(&[9, 0x02, total as u8, (total >> 8) as u8, 3, 1, 0, 0x80, 50])
            .unwrap();
        cfg.extend_from_slice(&b).unwrap();
        cfg
    }

    #[test]
    fn matches_full_duplex_playback_interface() {
        let raw = uac2_full_duplex_cfg(3, 2, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let p = find_uac_playback(&cfg).expect("playback interface should match");
        assert_eq!(p.num_channels, 2);
        assert_eq!(p.endpoint.ep_dir(), Direction::Out);
        assert_eq!(p.endpoint.endpoint_address, 0x02);
        // Capture still matches independently.
        assert!(find_uac_capture(&cfg).is_ok());
    }

    #[test]
    fn capture_only_device_has_no_playback() {
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        assert!(find_uac_playback(&cfg).is_none());
    }

    #[test]
    fn rejects_oversized_playback_channels() {
        // 9 > MAX_CHANNELS: rejecting here is what keeps the fixed
        // `[f32; MAX_CHANNELS]` playback buffers panic-free downstream.
        let raw = uac2_full_duplex_cfg(3, 2, 9);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        assert!(find_uac_playback(&cfg).is_none());
    }

    #[test]
    fn register_opens_playback_on_full_duplex_device() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        let alloc = MockAlloc::new(state);
        let raw = uac2_full_duplex_cfg(3, 2, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        let host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert_eq!(host.channels(), 2); // capture
        assert_eq!(host.playback_channels(), 2); // playback opened
        // control + iso IN + iso OUT = 3 pipes.
        assert_eq!(state.allocs.borrow().len(), 3);
        // A SET_INTERFACE for the playback interface (2) was issued.
        let setups = state.setups.borrow();
        assert!(
            setups.iter().any(|s| s[1] == 11 && s[4] == 2),
            "expected SET_INTERFACE on playback interface 2"
        );
    }

    #[test]
    fn register_capture_only_has_no_playback() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        let host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert_eq!(host.playback_channels(), 0);
        assert_eq!(state.allocs.borrow().len(), 2); // control + iso IN only
    }

    #[test]
    fn full_duplex_pump_sends_encoded_playback() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        // One capture packet: two mono 24-bit frames (so 2 playback frames sent).
        let mut pkt = Vec::<u8, 64>::new();
        pkt.extend_from_slice(&[0, 0, 0x40, 0, 0, 0x40]).unwrap(); // +0.5, +0.5
        state.script.borrow_mut().reads.push(pkt).unwrap();

        let alloc = MockAlloc::new(state);
        let raw = uac2_full_duplex_cfg(3, 1, 1); // mono cap + mono play
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();

        // App-side playback data: known engine frames.
        host.push_playback_frame(&[0.5]);
        host.push_playback_frame(&[0.5]);

        block_on(host.pump_once()).expect("pump");
        let sent = state.log.borrow().sent.clone();
        assert!(!sent.is_empty(), "playback packet must be sent");
        assert_eq!(sent.len() % 3, 0);
        // 0.5 encodes to [0x00, 0x00, 0x40]; the emitted frames are all 0.5.
        assert_eq!(&sent[0..3], &[0x00, 0x00, 0x40]);
    }

    #[test]
    fn capture_only_pump_sends_no_playback() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        let mut pkt = Vec::<u8, 64>::new();
        pkt.extend_from_slice(&[0, 0, 0x40]).unwrap();
        state.script.borrow_mut().reads.push(pkt).unwrap();
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();

        block_on(host.pump_once()).expect("pump");
        assert!(
            state.log.borrow().sent.is_empty(),
            "capture-only sends no OUT"
        );
    }

    fn uac2_mic_cfg_mps(subslot: u8, channels: u8, mps: u16) -> Vec<u8, 256> {
        let mut b: Vec<u8, 256> = Vec::new();
        let push = |s: &[u8], b: &mut Vec<u8, 256>| b.extend_from_slice(s).unwrap();

        // IAD: first iface 0, count 2, AUDIO/undefined/UAC2.
        push(&[8, 0x0B, 0, 2, 0x01, 0x00, 0x20, 0], &mut b);
        // Std AC interface 0, alt 0, 0 eps, AUDIO/AUDIOCONTROL/UAC2.
        push(&[9, 0x04, 0, 0, 0, 0x01, 0x01, 0x20, 0], &mut b);
        // CS AC header (UAC2 CLOCK etc. are inside; length total little-endian).
        push(&[9, 0x24, 0x01, 0x00, 0x02, 30, 0, 0x00, 0x00], &mut b);
        // CS Clock Source: subtype 0x0A, clock id 0x09, attrs, controls, assoc.
        push(&[8, 0x24, 0x0A, 0x09, 0x01, 0x07, 0x00, 0x00], &mut b);
        // CS Input Terminal (subtype 0x02): term id 0x01, type MIC 0x0201,
        // assoc 0, clock source id 0x09, nrchannels, ...
        push(
            &[
                17, 0x24, 0x02, 0x01, 0x01, 0x02, 0, 0x09, channels, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            &mut b,
        );
        // Std AS interface 1, alt 0 (zero bandwidth): 0 eps.
        push(&[9, 0x04, 1, 0, 0, 0x01, 0x02, 0x20, 0], &mut b);
        // Std AS interface 1, alt 1: 1 ep, AUDIO/AUDIOSTREAMING/UAC2.
        push(&[9, 0x04, 1, 1, 1, 0x01, 0x02, 0x20, 0], &mut b);
        // CS AS general (subtype 0x01): terminal link 0x01, controls, FORMAT_TYPE_I=1,
        // formats bitmap(4), nrchannels, channel config(4), name.
        push(
            &[
                16, 0x24, 0x01, 0x01, 0x00, 0x01, 0x01, 0, 0, 0, channels, 0, 0, 0, 0, 0,
            ],
            &mut b,
        );
        // CS AS FORMAT_TYPE (subtype 0x02): FORMAT_TYPE_I=1, subslot, bit_res.
        push(&[6, 0x24, 0x02, 0x01, subslot, subslot * 8], &mut b);
        // Std iso IN endpoint 0x81, iso(0x01) async, mps, interval 4.
        push(
            &[7, 0x05, 0x81, 0x01, mps as u8, (mps >> 8) as u8, 0x04],
            &mut b,
        );
        // CS AS iso endpoint (0x25): general, attrs, controls, lock delay units/val.
        push(&[8, 0x25, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00], &mut b);

        // Configuration header: len 9, CONFIGURATION=0x02, total_len LE, 2 ifaces.
        let total = (9 + b.len()) as u16;
        let mut cfg: Vec<u8, 256> = Vec::new();
        cfg.extend_from_slice(&[9, 0x02, total as u8, (total >> 8) as u8, 2, 1, 0, 0x80, 50])
            .unwrap();
        cfg.extend_from_slice(&b).unwrap();
        cfg
    }

    #[test]
    fn matches_24bit_capture_interface() {
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let m = find_uac_capture(&cfg).expect("should match a 24-bit UAC2 mic");
        assert_eq!(m.streaming_interface, 1);
        assert_eq!(m.alternate_setting, 1);
        assert_eq!(m.num_channels, 2);
        assert_eq!(m.clock_id, 0x09);
        assert_eq!(m.format, pcm::SampleFormat::S24);
        assert_eq!(m.ac_interface, 0);
        assert_eq!(m.endpoint.endpoint_address, 0x81);
        assert_eq!(m.endpoint.ep_dir(), Direction::In);
    }

    #[test]
    fn accepts_16_bit_and_rejects_an_unhandled_subslot() {
        let raw = uac2_mic_cfg(2, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let m = find_uac_capture(&cfg).expect("16-bit capture matches");
        assert_eq!(m.format, pcm::SampleFormat::S16);
        let raw = uac2_mic_cfg(1, 2); // 1-byte subslot: no codec
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        assert!(matches!(
            find_uac_capture(&cfg),
            Err(UacError::UnsupportedFormat)
        ));
    }

    #[test]
    fn rejects_out_of_range_channel_counts() {
        // > MAX_CHANNELS (8): `pump_once` would index `[f32; MAX_CHANNELS]`
        // out of bounds if this were allowed through.
        let raw = uac2_mic_cfg(3, 9);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        assert!(matches!(
            find_uac_capture(&cfg),
            Err(UacError::UnsupportedFormat)
        ));

        // 0 channels: `pump_once` would panic on `chunks_exact(0)` if this
        // were allowed through. Only assert if the fixture still parses as a
        // matched interface up to the channel check — a 0-channel descriptor
        // failing to parse for unrelated reasons wouldn't isolate the check.
        let raw0 = uac2_mic_cfg(3, 0);
        let cfg0 = ConfigurationDescriptor::try_from_slice(&raw0).unwrap();
        assert!(matches!(
            find_uac_capture(&cfg0),
            Err(UacError::UnsupportedFormat)
        ));
    }

    #[test]
    fn rejects_oversized_max_packet() {
        // Both pumps do `buf[..cap]` against a fixed `[0u8; 1024]` scratch
        // buffer. A descriptor declaring `wMaxPacketSize > 1024` must be
        // rejected here rather than allowed through to slice-panic later.
        let raw = uac2_mic_cfg_mps(3, 2, 2048);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        assert!(matches!(
            find_uac_capture(&cfg),
            Err(UacError::UnsupportedFormat)
        ));
    }

    // --- Uac::try_register negotiation ---

    use crate::host::mock::{MockAlloc, MockState};
    use embassy_futures::block_on;

    #[test]
    fn register_negotiates_when_rate_already_44100() {
        let state = MockState::leak();
        // GET_CUR returns 44100 -> no SET_CUR expected.
        state
            .control_reads
            .borrow_mut()
            .push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        let host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert_eq!(host.channels(), 2);
        // Two pipes claimed: control (EP0) + iso IN (0x81).
        assert_eq!(state.allocs.borrow().len(), 2);
        // Exactly one SETUP: the GET_CUR (no SET_CUR), plus the SET_INTERFACE.
        let setups = state.setups.borrow();
        // Last setup is SET_INTERFACE (bRequest 11, standard/interface/out).
        let last = setups.last().unwrap();
        assert_eq!(last[1], 11, "bRequest = SET_INTERFACE");
        assert_eq!(last[2], 1, "wValue lo = alt setting 1");
        assert_eq!(last[4], 1, "wIndex lo = streaming interface 1");
    }

    #[test]
    fn register_forces_rate_when_not_44100() {
        let state = MockState::leak();
        // GET_CUR -> 48000, then after SET_CUR the confirming GET_CUR -> 44100.
        {
            let mut r = state.control_reads.borrow_mut();
            r.push(heapless::Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
                .unwrap();
            r.push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
                .unwrap();
        }
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        let host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert_eq!(host.channels(), 1);
        // The SET_CUR data stage carried 44100 LE.
        assert_eq!(
            &state.control_out_data.borrow()[..4],
            &44_100u32.to_le_bytes()
        );
    }

    // --- decode_s24le ---

    #[test]
    fn decode_s24le_endpoints() {
        assert_eq!(decode_s24le(&[0, 0, 0]), 0.0);
        assert_eq!(decode_s24le(&[0, 0, 0x40]), 0.5); // 0x400000 / 2^23
        assert_eq!(decode_s24le(&[0, 0, 0x80]), -1.0); // 0x800000 sign-extended
    }

    // --- pump_once / read ---

    #[test]
    fn pump_decodes_iso_packet_into_ring() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        // One iso IN packet: two mono 24-bit frames, values +0.5 and -1.0.
        let mut pkt = heapless::Vec::<u8, 64>::new();
        pkt.extend_from_slice(&[0, 0, 0x40, 0, 0, 0x80]).unwrap();
        state.script.borrow_mut().reads.push(pkt).unwrap();

        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();

        block_on(host.pump_once()).expect("pump");
        // At r≈1.0 the resampler emits ~one frame per input frame after priming;
        // at least one decoded sample must be readable.
        let mut out = [0.0f32; 4];
        let got = host.read(&mut out);
        assert!(got >= 1, "expected decoded samples in the ring, got {got}");
        assert!(out[..got].iter().any(|&s| s < 0.0 || s > 0.0), "non-silent");
    }

    #[test]
    fn pump_tolerates_transfer_error() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        *state.script.borrow_mut() = crate::host::mock::Script {
            reads: Default::default(),
            read_err: Some(embassy_usb_driver::host::PipeError::Timeout),
        };
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();

        // A failed iso transfer must NOT error out — the stream survives.
        block_on(host.pump_once()).expect("iso error is absorbed, not fatal");
    }

    /// Lost isochronous packets are counted, not just logged: at a packet per microframe a
    /// warning each would fill the debug log and stall the firmware.
    #[test]
    fn lost_packets_are_counted() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        *state.script.borrow_mut() = crate::host::mock::Script {
            reads: Default::default(),
            read_err: Some(embassy_usb_driver::host::PipeError::Timeout),
        };
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();
        assert_eq!(host.transfer_errors(), 0);
        for _ in 0..3 {
            // The mock's scripted error is one-shot; re-arm it for every transfer.
            state.script.borrow_mut().read_err = Some(PipeError::Timeout);
            block_on(host.pump_once()).expect("absorbed");
        }
        assert_eq!(host.transfer_errors(), 3);
    }

    #[test]
    fn pump_propagates_on_detach() {
        // `PipeError::BadResponse` is what the RUSB1 HAL's DTCH handler
        // forces on every pipe on physical detach (see the doc comment on
        // `pump_once`). Unlike a transient error (`Timeout`, tested above),
        // this must come back as `Err` so `uac_capture_task`'s loop can
        // `break`, drop `host`, and free the pipes/address.
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        *state.script.borrow_mut() = crate::host::mock::Script {
            reads: Default::default(),
            read_err: Some(embassy_usb_driver::host::PipeError::BadResponse),
        };
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();

        let err = block_on(host.pump_once()).expect_err("detach must be propagated, not absorbed");
        assert!(matches!(err, UacError::Transfer(PipeError::BadResponse)));
    }

    /// `Canceled` is the HAL's "port stopped" (after an EOF error or a detach, until the
    /// device is reset again): every transfer fails at once, so absorbing it spun the pump
    /// task forever and starved the reset that would have restarted the port.
    #[test]
    fn pump_propagates_a_stopped_port() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        state.script.borrow_mut().read_err = Some(PipeError::Canceled);
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();

        let err = block_on(host.pump_once()).expect_err("a stopped port ends the stream");
        assert!(matches!(err, UacError::Transfer(PipeError::Canceled)));
    }

    #[test]
    fn pump_tolerates_torn_trailing_partial_sample() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(heapless::Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
            .unwrap();
        // Mono (channels=1) -> frame_bytes == 3. 7 bytes = two whole frames
        // (+0.5, -1.0) plus one torn trailing byte that `chunks_exact` must
        // silently drop rather than panic on.
        let mut pkt = heapless::Vec::<u8, 64>::new();
        pkt.extend_from_slice(&[0, 0, 0x40, 0, 0, 0x80, 0xAA])
            .unwrap();
        state.script.borrow_mut().reads.push(pkt).unwrap();

        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 1);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();

        // Must not panic on the non-multiple-of-frame_bytes packet length.
        block_on(host.pump_once()).expect("torn trailing sample is silently dropped, not fatal");
    }

    /// UAC2 `SET CUR` on a clock source: class request, interface recipient, host-to-device.
    fn is_set_cur(setup: &[u8]) -> bool {
        setup[0] == 0x21 && setup[1] == 0x01
    }

    #[test]
    fn register_at_the_requested_rate_skips_set_cur() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
            .unwrap();
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        block_on(Uac::try_register(&alloc, 1, None, &cfg, 48_000)).expect("register at 48 kHz");
        assert!(
            !state.setups.borrow().iter().any(|s| is_set_cur(s)),
            "no SET CUR expected"
        );
    }

    /// A device asked for 44.1 kHz that stays at 48 kHz (a programmable clock without the rate)
    /// runs at 48 kHz: the resampler converts, centred on 48000 / 44100.
    #[test]
    fn a_device_that_will_not_switch_runs_at_its_own_rate() {
        let state = MockState::leak();
        for _ in 0..2 {
            state
                .control_reads
                .borrow_mut()
                .push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
                .unwrap();
        }
        let alloc = MockAlloc::new(state);
        let raw = uac2_mic_cfg(3, 2);
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        let host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert_eq!(host.sample_rate(), 48_000);
        assert_eq!(host.r, 48_000.0 / 44_100.0);
    }

    use super::desc::fixtures::{INTERFACE_2B5A, digitakt};

    /// UAC2 `CUR` request (class, interface recipient) to `entity`, control selector `cs`.
    fn is_cur(setup: &[u8], dir_in: bool, entity: u8, cs: u8) -> bool {
        setup[0] == if dir_in { 0xA1 } else { 0x21 }
            && setup[1] == 0x01
            && setup[3] == cs
            && setup[5] == entity
    }

    #[test]
    fn registration_follows_a_clock_selector_to_its_source() {
        let state = MockState::leak();
        {
            let mut r = state.control_reads.borrow_mut();
            r.push(Vec::from_slice(&[1]).unwrap()).unwrap(); // selector 40: pin 1 (source 41)
            r.push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
                .unwrap();
            r.push(Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
                .unwrap();
        }
        let alloc = MockAlloc::new(state);
        let cfg = ConfigurationDescriptor::try_from_slice(INTERFACE_2B5A).unwrap();

        let host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert_eq!(host.channels(), 4);
        assert_eq!(host.playback_channels(), 4);
        assert_eq!(host.sample_rate(), 44_100);
        let setups = state.setups.borrow();
        assert!(is_cur(&setups[0], true, 40, 0x01), "GET CUR on selector 40");
        assert!(
            setups.iter().any(|s| is_cur(s, false, 41, 0x01)),
            "SET CUR on source 41"
        );
        assert!(
            !setups.iter().any(|s| is_cur(s, false, 40, 0x01)),
            "never SET CUR the selector"
        );
    }

    /// A selector that answers nothing is taken at its first input.
    #[test]
    fn a_silent_selector_is_taken_at_its_first_input() {
        let state = MockState::leak();
        {
            let mut r = state.control_reads.borrow_mut();
            r.push(Vec::new()).unwrap(); // selector GET CUR: no data
            r.push(Vec::from_slice(&44_100u32.to_le_bytes()).unwrap())
                .unwrap();
        }
        let alloc = MockAlloc::new(state);
        let cfg = ConfigurationDescriptor::try_from_slice(INTERFACE_2B5A).unwrap();

        block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert!(
            is_cur(&state.setups.borrow()[1], true, 41, 0x01),
            "frequency read on source 41"
        );
    }

    /// The Digitakt's clock is fixed at 48 kHz: no SET CUR, the stream runs at 48 kHz.
    #[test]
    fn a_fixed_clock_runs_at_its_own_rate_without_set_cur() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
            .unwrap();
        let alloc = MockAlloc::new(state);
        let raw = digitakt();
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();

        let host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");
        assert_eq!(host.sample_rate(), 48_000);
        assert_eq!((host.channels(), host.playback_channels()), (2, 2));
        assert!(
            !state.setups.borrow().iter().any(|s| is_set_cur(s)),
            "no SET CUR"
        );
        // control + capture IN 0x83 + playback OUT 0x03; never the feedback endpoint 0x82.
        let allocs = state.allocs.borrow();
        assert_eq!(allocs.len(), 3);
        assert!(
            allocs.iter().all(|a| a.ep_addr != 0x82),
            "feedback endpoint not claimed"
        );
    }

    /// The playback endpoint is claimed only once SET_INTERFACE has enabled it: the HAL arms
    /// an isochronous OUT pipe when it opens it, and the device must not be sent packets for an
    /// endpoint its interface has not selected yet (on hardware the next control transfer then
    /// failed).
    #[test]
    fn the_playback_pipe_opens_after_its_interface_is_selected() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
            .unwrap();
        let alloc = MockAlloc::new(state);
        let raw = digitakt();
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).expect("register");

        let setups = state.setups.borrow();
        let select = setups
            .iter()
            .position(|s| s[1] == 11 && s[4] == 1)
            .expect("SET_INTERFACE on playback interface 1");
        let out = state
            .allocs
            .borrow()
            .iter()
            .find(|a| a.ep_addr == 0x03)
            .copied()
            .unwrap();
        assert!(
            out.setups_before > select,
            "pipe opened after SET_INTERFACE"
        );
    }

    #[test]
    fn the_pump_decodes_and_encodes_4_byte_subslots() {
        let state = MockState::leak();
        state
            .control_reads
            .borrow_mut()
            .push(Vec::from_slice(&48_000u32.to_le_bytes()).unwrap())
            .unwrap();
        // Six stereo 32-bit frames: left +0.5, right -0.5.
        let mut pkt = Vec::<u8, 64>::new();
        for _ in 0..6 {
            pkt.extend_from_slice(&[0, 0, 0, 0x40, 0, 0, 0, 0xC0])
                .unwrap();
        }
        state.script.borrow_mut().reads.push(pkt).unwrap();
        let alloc = MockAlloc::new(state);
        let raw = digitakt();
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        let mut host = block_on(Uac::try_register(&alloc, 1, None, &cfg, 44_100)).unwrap();
        for _ in 0..4 {
            host.push_playback_frame(&[0.5, -0.5]);
        }

        block_on(host.pump_once()).expect("pump");
        let mut out = [0.0f32; 16];
        let got = host.read(&mut out);
        assert!(got >= 2 && got % 2 == 0, "whole stereo frames, got {got}");
        for f in out[..got].chunks(2) {
            assert_eq!(f, [0.5, -0.5]);
        }
        let sent = state.log.borrow().sent.clone();
        assert_eq!(sent.len() % 8, 0, "whole 4-byte stereo frames sent");
        assert_eq!(&sent[0..8], &[0, 0, 0, 0x40, 0, 0, 0, 0xC0]);
    }
}
