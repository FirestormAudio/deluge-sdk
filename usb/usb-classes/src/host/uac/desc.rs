//! The parts of a UAC2 function's descriptors this driver needs, per alternate setting.
//!
//! `embassy-usb-host`'s `AudioInterfaceCollection` keeps one format, channel count and endpoint
//! per streaming *interface*, so an interface with several alternate settings (24-bit and
//! 16-bit, say) reports only the last. This walks the raw configuration instead and keeps
//! every alternate setting, the clock entities, and each terminal's clock.
//!
//! Only descriptors belonging to the UAC2 function count: a MIDIStreaming interface's jack
//! descriptors share subtype numbers with UAC terminals, and devices pair it with a UAC1
//! AudioControl interface that is not the audio function's.

use embassy_usb_driver::Direction;
use embassy_usb_host::descriptor::{ConfigurationDescriptor, EndpointDescriptor};
use heapless::Vec;

use super::pcm::SampleFormat;
use super::{MAX_CHANNELS, UacError};

/// Most streaming alternate settings kept.
pub const MAX_STREAM_ALTS: usize = 16;
/// Most clock entities kept.
pub const MAX_CLOCKS: usize = 8;
/// Most terminals kept.
pub const MAX_TERMINALS: usize = 16;
/// Most inputs kept for one clock selector.
pub const MAX_SELECTOR_INPUTS: usize = 8;
/// Largest data packet the pumps' fixed scratch buffers hold.
pub const MAX_PACKET: u16 = 1024;

/// A clock entity in the AudioControl interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClockEntity {
    /// A clock source. `programmable` when the host may set its frequency
    /// (`bmControls` frequency control = host programmable).
    Source { id: u8, programmable: bool },
    /// A clock selector choosing one of `inputs` (clock entity ids, pin 1 first).
    Selector {
        id: u8,
        inputs: Vec<u8, MAX_SELECTOR_INPUTS>,
    },
    /// A clock multiplier fed by `source`.
    Multiplier { id: u8, source: u8 },
}

impl ClockEntity {
    /// The entity's id.
    pub fn id(&self) -> u8 {
        match self {
            Self::Source { id, .. } | Self::Selector { id, .. } | Self::Multiplier { id, .. } => {
                *id
            }
        }
    }
}

/// One alternate setting of an AudioStreaming interface.
#[derive(Clone, Copy, Debug)]
pub struct StreamAlt {
    pub interface: u8,
    pub alternate_setting: u8,
    /// The terminal the stream connects to (`bTerminalLink`).
    pub terminal_link: u8,
    pub channels: u8,
    /// Type-I PCM in a subslot size the codec handles, else `None`.
    pub format: Option<SampleFormat>,
    pub bit_resolution: u8,
    /// The isochronous data endpoint (an explicit-feedback endpoint is not one).
    pub endpoint: Option<EndpointDescriptor>,
}

/// A UAC2 function: its AudioControl interface, clock entities, terminals and streams.
#[derive(Debug)]
pub struct AudioFunction {
    pub ac_interface: u8,
    pub clocks: Vec<ClockEntity, MAX_CLOCKS>,
    /// `(terminal id, clock entity id)`.
    pub terminals: Vec<(u8, u8), MAX_TERMINALS>,
    pub streams: Vec<StreamAlt, MAX_STREAM_ALTS>,
}

impl AudioFunction {
    /// The clock entity with `id`.
    pub fn clock(&self, id: u8) -> Option<&ClockEntity> {
        self.clocks.iter().find(|c| c.id() == id)
    }

    /// The clock entity feeding terminal `terminal`.
    pub fn terminal_clock(&self, terminal: u8) -> Option<u8> {
        self.terminals.iter().find(|t| t.0 == terminal).map(|t| t.1)
    }

    /// The best usable alternate setting streaming in `dir` (`In` = capture): an isochronous
    /// data endpoint in that direction, a codec format, 1..=MAX_CHANNELS channels and a
    /// packet the pumps hold. Highest bit resolution wins, then the narrower subslot.
    pub fn best_stream(&self, dir: Direction) -> Option<&StreamAlt> {
        let usable = |s: &&StreamAlt| {
            let Some(ep) = s.endpoint else { return false };
            ep.ep_dir() == dir
                && s.format.is_some()
                && (1..=MAX_CHANNELS).contains(&(s.channels as usize))
                && ep.max_packet_size <= MAX_PACKET
        };
        let rank = |s: &StreamAlt| {
            let bytes = s.format.map(|f| f.bytes()).unwrap_or(usize::MAX);
            (s.bit_resolution, core::cmp::Reverse(bytes))
        };
        // The first of equally ranked settings, as the device lists them.
        self.streams
            .iter()
            .filter(usable)
            .fold(None, |best: Option<&StreamAlt>, s| match best {
                Some(b) if rank(b) >= rank(s) => Some(b),
                _ => Some(s),
            })
    }

    /// Whether any alternate setting has a data endpoint streaming in `dir`.
    pub fn has_stream(&self, dir: Direction) -> bool {
        self.streams
            .iter()
            .any(|s| s.endpoint.is_some_and(|e| e.ep_dir() == dir))
    }
}

const INTERFACE: u8 = 0x04;
const ENDPOINT: u8 = 0x05;
const CS_INTERFACE: u8 = 0x24;
const AUDIO: u8 = 0x01;
const AUDIOCONTROL: u8 = 0x01;
const AUDIOSTREAMING: u8 = 0x02;
const UAC2: u8 = 0x20;
const ISOCHRONOUS: u8 = 0x01;
const FEEDBACK: u8 = 0x01;

/// Which interface the descriptors being walked belong to.
enum Owner {
    Control,
    Streaming,
    Other,
}

/// Parse the UAC2 function in `cfg`.
pub fn parse_audio_function(cfg: &ConfigurationDescriptor<'_>) -> Result<AudioFunction, UacError> {
    let mut ac_interface = None;
    let mut clocks = Vec::new();
    let mut terminals = Vec::new();
    let mut streams: Vec<StreamAlt, MAX_STREAM_ALTS> = Vec::new();
    let mut owner = Owner::Other;
    // The streaming alternate setting being read, and its PCM flag (AS_GENERAL bmFormats).
    let mut alt: Option<(StreamAlt, bool)> = None;
    let mut finish = |alt: &mut Option<(StreamAlt, bool)>| {
        if let Some((s, _)) = alt.take()
            && s.endpoint.is_some()
        {
            streams.push(s).ok();
        }
    };

    for (_, d) in cfg.iter_descriptors() {
        match (d[1], &owner) {
            (INTERFACE, _) if d.len() >= 9 => {
                finish(&mut alt);
                owner = match (d[5], d[6], d[7]) {
                    (AUDIO, AUDIOCONTROL, UAC2) => {
                        ac_interface.get_or_insert(d[2]);
                        Owner::Control
                    }
                    (AUDIO, AUDIOSTREAMING, UAC2) => {
                        let s = StreamAlt {
                            interface: d[2],
                            alternate_setting: d[3],
                            terminal_link: 0,
                            channels: 0,
                            format: None,
                            bit_resolution: 0,
                            endpoint: None,
                        };
                        alt = Some((s, false));
                        Owner::Streaming
                    }
                    _ => Owner::Other,
                };
            }
            (CS_INTERFACE, Owner::Control) if d.len() >= 4 => match d[2] {
                0x0A if d.len() >= 8 => {
                    let programmable = d[5] & 0b11 == 0b11;
                    clocks
                        .push(ClockEntity::Source {
                            id: d[3],
                            programmable,
                        })
                        .ok();
                }
                0x0B if d.len() >= 5 => {
                    let n = (d[4] as usize).min(d.len().saturating_sub(5));
                    let mut inputs = Vec::new();
                    for &i in d[5..5 + n].iter().take(MAX_SELECTOR_INPUTS) {
                        inputs.push(i).ok();
                    }
                    clocks.push(ClockEntity::Selector { id: d[3], inputs }).ok();
                }
                0x0C if d.len() >= 5 => {
                    clocks
                        .push(ClockEntity::Multiplier {
                            id: d[3],
                            source: d[4],
                        })
                        .ok();
                }
                0x02 if d.len() >= 8 => {
                    terminals.push((d[3], d[7])).ok();
                }
                0x03 if d.len() >= 9 => {
                    terminals.push((d[3], d[8])).ok();
                }
                _ => {}
            },
            (CS_INTERFACE, Owner::Streaming) if d.len() >= 4 => {
                if let Some((s, pcm)) = alt.as_mut() {
                    match d[2] {
                        // AS_GENERAL: terminal link, format type, bmFormats, channels.
                        0x01 if d.len() >= 11 => {
                            s.terminal_link = d[3];
                            *pcm = d[5] == 1 && d[6] & 1 != 0;
                            s.channels = d[10];
                        }
                        // FORMAT_TYPE I: subslot size, bit resolution.
                        0x02 if d.len() >= 6 && d[3] == 1 => {
                            s.format = if *pcm {
                                SampleFormat::from_subslot(d[4])
                            } else {
                                None
                            };
                            s.bit_resolution = d[5];
                        }
                        _ => {}
                    }
                }
            }
            (ENDPOINT, Owner::Streaming) if d.len() >= 7 => {
                let attributes = d[3];
                let data = attributes & 0b11 == ISOCHRONOUS && (attributes >> 4) & 0b11 != FEEDBACK;
                if let Some((s, _)) = alt.as_mut()
                    && data
                    && s.endpoint.is_none()
                {
                    s.endpoint = Some(EndpointDescriptor {
                        len: d[0],
                        descriptor_type: d[1],
                        endpoint_address: d[2],
                        attributes,
                        max_packet_size: u16::from_le_bytes([d[4], d[5]]),
                        interval: d[6],
                    });
                }
            }
            _ => {}
        }
    }
    finish(&mut alt);

    Ok(AudioFunction {
        ac_interface: ac_interface.ok_or(UacError::BadDescriptors)?,
        clocks,
        terminals,
        streams,
    })
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// A USB-C UAC2 audio interface (VID 2b5a, PID 0028), as read on hardware: clock source 41
    /// (programmable) behind selector 40; playback IF1 alt 1 24-in-32 / alt 2 16-bit, 4 ch,
    /// with explicit feedback; capture IF2 alt 1 24-in-32, 4 ch; a UAC1 AC + MIDIStreaming pair.
    const INTERFACE_2B5A: &[u8] = include_bytes!("../../../testdata/uac2-interface-2b5a-0028.bin");

    /// An Elektron Digitakt in AUDIO/MIDI mode, from its `lsusb -v`: one fixed 48 kHz clock
    /// (frequency read-only); playback IF1 alt 1 and capture IF2 alt 1, both 2 ch 32-bit in
    /// 4-byte subslots; playback with an explicit-feedback IN endpoint 0x82, capture with
    /// implicit feedback; a UAC1 AC + MIDIStreaming pair.
    fn digitakt() -> std::vec::Vec<u8> {
        let mut b = std::vec::Vec::new();
        let mut push = |s: &[u8]| b.extend_from_slice(s);
        push(&[8, 0x0B, 0, 3, 1, 0, 0x20, 0]);
        push(&[9, 4, 0, 0, 0, 1, 1, 0x20, 0]);
        push(&[9, 0x24, 1, 0x00, 0x02, 0, 0, 0, 0]);
        push(&[8, 0x24, 0x0A, 1, 0x01, 0x01, 0, 0]); // fixed, frequency read-only
        push(&[17, 0x24, 2, 3, 0x01, 0x01, 0, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0]); // IT 3 USB streaming
        push(&[18, 0x24, 6, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // FU 2 <- 3
        push(&[12, 0x24, 3, 4, 0x01, 0x03, 0, 2, 1, 0, 0, 0]); // OT 4 speaker <- 2
        push(&[17, 0x24, 2, 5, 0x02, 0x06, 0, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0]); // IT 5 digital
        push(&[12, 0x24, 3, 6, 0x01, 0x01, 0, 5, 1, 0, 0, 0]); // OT 6 USB streaming <- 5
        push(&[9, 4, 1, 0, 0, 1, 2, 0x20, 0]);
        push(&[9, 4, 1, 1, 2, 1, 2, 0x20, 0]);
        push(&[16, 0x24, 1, 3, 0, 1, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0]);
        push(&[6, 0x24, 2, 1, 4, 32]);
        push(&[7, 5, 0x03, 0x05, 56, 0, 1]); // iso OUT async data
        push(&[8, 0x25, 1, 0, 0, 0, 0, 0]);
        push(&[7, 5, 0x82, 0x11, 4, 0, 1]); // iso IN explicit feedback
        push(&[9, 4, 2, 0, 0, 1, 2, 0x20, 0]);
        push(&[9, 4, 2, 1, 1, 1, 2, 0x20, 0]);
        push(&[16, 0x24, 1, 6, 0, 1, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0]);
        push(&[6, 0x24, 2, 1, 4, 32]);
        push(&[7, 5, 0x83, 0x25, 56, 0, 1]); // iso IN async, implicit feedback data
        push(&[8, 0x25, 1, 0, 0, 0, 0, 0]);
        push(&[9, 4, 3, 0, 0, 1, 1, 0, 0]); // UAC1 AudioControl for MIDI
        push(&[9, 0x24, 1, 0x00, 0x01, 9, 0, 1, 4]);
        push(&[9, 4, 4, 0, 2, 1, 3, 0, 0]); // MIDIStreaming
        push(&[7, 0x24, 1, 0x00, 0x01, 0, 0]);
        push(&[6, 0x24, 2, 1, 1, 0]); // MIDI IN jack: subtype 2, like an input terminal
        push(&[9, 0x24, 3, 1, 3, 1, 1, 1, 0]); // MIDI OUT jack: subtype 3
        push(&[9, 5, 0x01, 2, 0x00, 0x02, 0, 0, 0]);
        push(&[5, 0x25, 1, 1, 1]);
        push(&[9, 5, 0x81, 2, 0x00, 0x02, 0, 0, 0]);
        push(&[5, 0x25, 1, 1, 3]);
        let total = (9 + b.len()) as u16;
        let mut cfg = std::vec![9, 2, total as u8, (total >> 8) as u8, 5, 1, 0, 0xC0, 3];
        cfg.extend_from_slice(&b);
        cfg
    }

    fn parse(raw: &[u8]) -> AudioFunction {
        parse_audio_function(&ConfigurationDescriptor::try_from_slice(raw).unwrap()).unwrap()
    }

    #[test]
    fn the_interface_keeps_every_playback_alternate_setting() {
        let f = parse(INTERFACE_2B5A);
        assert_eq!(f.ac_interface, 0);
        let pb: std::vec::Vec<_> = f.streams.iter().filter(|s| s.interface == 1).collect();
        assert_eq!(pb.len(), 2, "alt 0 has no endpoint and is not a stream");
        assert_eq!(pb[0].alternate_setting, 1);
        assert_eq!(pb[0].format, Some(SampleFormat::S32));
        assert_eq!(pb[0].bit_resolution, 24);
        assert_eq!(pb[1].alternate_setting, 2);
        assert_eq!(pb[1].format, Some(SampleFormat::S16));
        assert_eq!(pb[1].endpoint.unwrap().endpoint_address, 0x01);
    }

    #[test]
    fn the_interfaces_best_streams_are_its_24_bit_alternate_settings() {
        let f = parse(INTERFACE_2B5A);
        let pb = f.best_stream(Direction::Out).expect("playback");
        assert_eq!((pb.interface, pb.alternate_setting), (1, 1));
        assert_eq!(pb.channels, 4);
        assert_eq!(pb.endpoint.unwrap().max_packet_size, 400);
        let cap = f.best_stream(Direction::In).expect("capture");
        assert_eq!((cap.interface, cap.alternate_setting), (2, 1));
        assert_eq!(cap.endpoint.unwrap().endpoint_address, 0x82);
        assert_eq!(cap.terminal_link, 22);
    }

    #[test]
    fn the_interfaces_clock_is_a_selector_in_front_of_a_programmable_source() {
        let f = parse(INTERFACE_2B5A);
        assert_eq!(f.terminal_clock(22), Some(40));
        assert_eq!(f.terminal_clock(2), Some(40));
        assert_eq!(
            f.clock(40),
            Some(&ClockEntity::Selector {
                id: 40,
                inputs: Vec::from_slice(&[41]).unwrap()
            })
        );
        assert_eq!(
            f.clock(41),
            Some(&ClockEntity::Source {
                id: 41,
                programmable: true
            })
        );
    }

    #[test]
    fn midi_jacks_are_not_audio_terminals() {
        // The MIDIStreaming jacks have ids 1 and 2, subtypes 2 and 3.
        let f = parse(INTERFACE_2B5A);
        let ids: std::vec::Vec<u8> = f.terminals.iter().map(|t| t.0).collect();
        assert_eq!(ids, [2, 20, 1, 22], "the UAC2 terminals only, in order");
        let d = parse(&digitakt());
        assert!(
            d.terminals.iter().all(|t| t.1 == 1),
            "every terminal on the one clock"
        );
    }

    #[test]
    fn the_digitakts_feedback_endpoint_is_not_its_playback_endpoint() {
        let f = parse(&digitakt());
        let pb = f.best_stream(Direction::Out).expect("playback");
        assert_eq!(pb.endpoint.unwrap().endpoint_address, 0x03);
        assert_eq!(pb.format, Some(SampleFormat::S32));
        let cap = f.best_stream(Direction::In).expect("capture");
        assert_eq!(
            cap.endpoint.unwrap().endpoint_address,
            0x83,
            "implicit feedback data counts"
        );
        assert_eq!((cap.interface, cap.channels), (2, 2));
    }

    #[test]
    fn the_digitakts_clock_is_fixed() {
        let f = parse(&digitakt());
        assert_eq!(f.terminal_clock(6), Some(1));
        assert_eq!(
            f.clock(1),
            Some(&ClockEntity::Source {
                id: 1,
                programmable: false
            })
        );
    }

    #[test]
    fn unusable_streams_are_not_best() {
        let mut raw = digitakt();
        // Make capture 9 channels (> MAX_CHANNELS) and playback a 1-byte subslot.
        let caps = raw.windows(4).position(|w| w == [16, 0x24, 1, 6]).unwrap();
        raw[caps + 10] = 9;
        let fmt = raw
            .windows(5)
            .position(|w| w == [6, 0x24, 2, 1, 4])
            .unwrap();
        raw[fmt + 4] = 1;
        let f = parse(&raw);
        assert!(f.best_stream(Direction::In).is_none());
        assert!(f.best_stream(Direction::Out).is_none());
        assert!(f.has_stream(Direction::In) && f.has_stream(Direction::Out));
    }

    #[test]
    fn a_configuration_without_a_uac2_function_is_rejected() {
        let mut raw = digitakt();
        raw[9 + 8 + 7] = 0; // the AudioControl interface's protocol: UAC1, not UAC2
        let cfg = ConfigurationDescriptor::try_from_slice(&raw).unwrap();
        assert!(matches!(
            parse_audio_function(&cfg),
            Err(UacError::BadDescriptors)
        ));
    }
}
