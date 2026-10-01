//! USB-MIDI 1.0 event packets, passed through whole.
//!
//! The byte API in [`classes::midi`](super::classes::midi) decodes received
//! packets into 3-byte messages and re-packetises a raw byte stream on the way
//! out, which suits apps that think in MIDI messages but cannot carry SysEx or
//! virtual cable numbers. A consumer that already speaks USB-MIDI 1.0 event
//! packets (CIN/cable header + 3 bytes, USB MIDI 1.0 §4) selects packet mode
//! with [`use_packets`] once at startup; from then on every packet the host
//! sends — all Code Index Numbers, SysEx included, cable number preserved —
//! lands in the packet RX queue byte for byte, and packets the consumer queues
//! are written to the host unchanged.
//!
//! The module has two sides. The consumer side
//! ([`try_recv_packet_from_host`], [`try_send_packet_to_host`],
//! [`tx_packet_free`], [`tx_packet_pending`]) is what an app calls. The
//! transport side ([`route_rx_packet`], [`deliver_packet_from_host`],
//! [`try_deliver_packet_from_host`], [`next_packet_to_host`],
//! [`drain_packets_to_host`]) is what the USB class's endpoint tasks call;
//! it is not tied to the USB driver, so it builds and is tested off-target.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

/// One USB-MIDI 1.0 event packet: `[cable << 4 | CIN, midi0, midi1, midi2]`.
pub type Packet = [u8; 4];

/// Packets in one full-speed 64-byte bulk transfer.
pub const PACKETS_PER_TRANSFER: usize = 64 / 4;

/// Depth of the host → device packet queue: four bulk transfers, so a SysEx
/// burst survives a few consumer polls without the RX task stalling the host.
pub const RX_PACKET_CAPACITY: usize = 4 * PACKETS_PER_TRANSFER;

/// Depth of the device → host packet queue. A consumer that checks
/// [`tx_packet_free`] before a long SysEx needs room for the whole message;
/// 1024 packets (4 KiB) holds ~3 KiB of SysEx payload.
pub const TX_PACKET_CAPACITY: usize = 1024;

/// Packets received from the host, in packet mode.
static RX_PACKETS: Channel<CriticalSectionRawMutex, Packet, RX_PACKET_CAPACITY> = Channel::new();

/// Packets queued for transmission to the host, in packet mode.
static TX_PACKETS: Channel<CriticalSectionRawMutex, Packet, TX_PACKET_CAPACITY> = Channel::new();

/// `true` once a consumer has selected packet mode.
static PACKET_MODE: AtomicBool = AtomicBool::new(false);

// ============================================================================
// Consumer side
// ============================================================================

/// Select packet mode: route every MIDI 1.0 packet through the packet queues
/// instead of the byte API.
///
/// Call once at startup, before the MIDI endpoint tasks run. The selection is
/// permanent: the byte API's queues receive nothing afterwards.
#[inline]
pub fn use_packets() {
    PACKET_MODE.store(true, Ordering::Release);
}

/// Returns `true` once [`use_packets`] has selected packet mode.
#[inline]
pub fn packets_enabled() -> bool {
    PACKET_MODE.load(Ordering::Acquire)
}

/// Try to receive the next USB-MIDI 1.0 event packet from the host, exactly
/// as it arrived on the wire.
/// Only meaningful in packet mode and when `!midi2_active()`.
#[inline]
pub fn try_recv_packet_from_host() -> Option<Packet> {
    RX_PACKETS.try_receive().ok()
}

/// Queue one USB-MIDI 1.0 event packet for transmission to the host, unchanged.
/// Returns `true` if queued, `false` if the queue is full.
/// Only meaningful in packet mode and when `!midi2_active()`.
#[inline]
pub fn try_send_packet_to_host(packet: Packet) -> bool {
    TX_PACKETS.try_send(packet).is_ok()
}

/// Free space, in packets, in the host-bound packet queue.
#[inline]
pub fn tx_packet_free() -> usize {
    TX_PACKETS.free_capacity()
}

/// Packets queued for transmission to the host but not yet on the wire.
#[inline]
pub fn tx_packet_pending() -> usize {
    TX_PACKETS.len()
}

// ============================================================================
// Transport side
// ============================================================================

/// Where the RX task sends one received packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxRoute {
    /// Packet mode: the packet itself, for the packet queue.
    Packet(Packet),
    /// Byte mode: the decoded message, zero-padded to 3 bytes, for the byte
    /// API's message queue.
    Message([u8; 3]),
    /// Byte mode: a packet the byte API cannot represent (see
    /// [`route_rx_packet`]).
    Drop,
}

/// Decide where one received packet goes.
///
/// In packet mode every packet passes through untouched; otherwise it is
/// decoded to a channel-voice / system message, or dropped.
pub fn route_rx_packet(packet: Packet, packet_mode: bool) -> RxRoute {
    if packet_mode {
        return RxRoute::Packet(packet);
    }
    match parse_usb_midi_packet(packet) {
        Some(msg) => RxRoute::Message(msg),
        None => RxRoute::Drop,
    }
}

/// Decode a USB-MIDI 1.0 event packet into a zero-padded 3-byte MIDI message.
/// `None` for SysEx start/continue (CIN 0x4), 3-byte SysEx end (CIN 0x7) and
/// reserved / cable-event CINs; the 1- and 2-byte end CINs share their layout
/// with system-common messages and decode as those.
pub(crate) fn parse_usb_midi_packet(packet: Packet) -> Option<[u8; 3]> {
    let [header, b1, b2, b3] = packet;
    match header & 0x0F {
        0x03 | 0x08 | 0x09 | 0x0A | 0x0B | 0x0E => Some([b1, b2, b3]),
        0x02 | 0x06 | 0x0C | 0x0D => Some([b1, b2, 0]),
        0x05 | 0x0F => Some([b1, 0, 0]),
        _ => None,
    }
}

/// Split one bulk OUT transfer into its whole packets; a trailing partial
/// packet is ignored.
pub fn packets_in(transfer: &[u8]) -> impl Iterator<Item = Packet> + '_ {
    transfer.as_chunks::<4>().0.iter().copied()
}

/// Queue a packet received from the host, waiting while the queue is full.
///
/// Waiting holds back the next OUT transfer, so the host is flow-controlled
/// (NAKed) instead of losing packets mid-SysEx.
pub async fn deliver_packet_from_host(packet: Packet) {
    RX_PACKETS.send(packet).await;
}

/// Queue a packet received from the host without waiting.
/// Returns `false` (and drops the packet) if the queue is full.
#[inline]
pub fn try_deliver_packet_from_host(packet: Packet) -> bool {
    RX_PACKETS.try_send(packet).is_ok()
}

/// Wait for the next packet the consumer queued for the host.
pub async fn next_packet_to_host() -> Packet {
    TX_PACKETS.receive().await
}

/// Move queued host-bound packets into `buf` without waiting, while a whole
/// packet still fits. Returns the bytes written (a multiple of 4).
///
/// The TX task uses it to fill the rest of a bulk IN transfer behind the
/// packet [`next_packet_to_host`] returned.
pub fn drain_packets_to_host(buf: &mut [u8]) -> usize {
    let mut n = 0;
    while buf.len() - n >= 4 {
        let Ok(packet) = TX_PACKETS.try_receive() else {
            break;
        };
        buf[n..n + 4].copy_from_slice(&packet);
        n += 4;
    }
    n
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// `F0 00 21 7B 01 02 03 F7` as a USB-MIDI SysEx stream on cable 2:
    /// start/continue (CIN 4) packets, then a 2-byte end (CIN 6).
    const SYSEX_CABLE2: [Packet; 3] = [
        [0x24, 0xF0, 0x00, 0x21],
        [0x24, 0x7B, 0x01, 0x02],
        [0x26, 0x03, 0xF7, 0x00],
    ];

    #[test]
    fn packet_mode_passes_every_packet_through_unchanged() {
        let ends = [
            [0x05, 0xF7, 0x00, 0x00], // 1-byte SysEx end
            [0x06, 0x7F, 0xF7, 0x00], // 2-byte SysEx end
            [0x07, 0x01, 0x02, 0xF7], // 3-byte SysEx end
        ];
        let note_on = [0x19, 0x91, 0x3C, 0x64]; // cable 1, channel 2
        let mut stream = Vec::from(SYSEX_CABLE2);
        stream.extend_from_slice(&ends);
        stream.push(note_on);
        for packet in stream {
            assert_eq!(route_rx_packet(packet, true), RxRoute::Packet(packet));
        }
    }

    #[test]
    fn byte_mode_decodes_voice_messages_and_drops_sysex() {
        assert_eq!(
            route_rx_packet([0x09, 0x90, 0x3C, 0x64], false),
            RxRoute::Message([0x90, 0x3C, 0x64])
        );
        assert_eq!(
            route_rx_packet([0x0C, 0xC0, 0x05, 0x00], false),
            RxRoute::Message([0xC0, 0x05, 0x00])
        );
        for packet in [SYSEX_CABLE2[0], SYSEX_CABLE2[1], [0x07, 0x01, 0x02, 0xF7]] {
            assert_eq!(route_rx_packet(packet, false), RxRoute::Drop);
        }
    }

    #[test]
    fn a_transfer_splits_into_whole_packets_byte_identically() {
        let mut transfer = Vec::new();
        for p in SYSEX_CABLE2 {
            transfer.extend_from_slice(&p);
        }
        transfer.extend_from_slice(&[0x09, 0x90, 0x3C, 0x64]);
        transfer.extend_from_slice(&[0x0B, 0xB0]); // a trailing partial packet
        let packets: Vec<Packet> = packets_in(&transfer).collect();
        assert_eq!(
            packets,
            [
                SYSEX_CABLE2[0],
                SYSEX_CABLE2[1],
                SYSEX_CABLE2[2],
                [0x09, 0x90, 0x3C, 0x64]
            ]
        );
    }

    /// The only test touching the queues: they are process-wide statics.
    #[test]
    fn queues_carry_packets_unchanged_in_both_directions() {
        // Host → device: what the RX task delivers is what the consumer reads.
        let note_on = [0x09, 0x90, 0x3C, 0x64];
        for p in SYSEX_CABLE2.iter().chain([&note_on]) {
            assert!(try_deliver_packet_from_host(*p));
        }
        let received: Vec<Packet> = core::iter::from_fn(try_recv_packet_from_host).collect();
        assert_eq!(
            received,
            [SYSEX_CABLE2[0], SYSEX_CABLE2[1], SYSEX_CABLE2[2], note_on]
        );

        // Device → host: what the consumer queues is what the TX task writes.
        assert_eq!(tx_packet_free(), TX_PACKET_CAPACITY);
        for p in SYSEX_CABLE2 {
            assert!(try_send_packet_to_host(p));
        }
        assert_eq!(tx_packet_pending(), 3);
        assert_eq!(tx_packet_free(), TX_PACKET_CAPACITY - 3);
        let first = embassy_futures::block_on(next_packet_to_host());
        assert_eq!(first, SYSEX_CABLE2[0]);
        // Room for one packet and a half: only the whole packet moves.
        let mut buf = [0u8; 6];
        assert_eq!(drain_packets_to_host(&mut buf), 4);
        assert_eq!(buf[..4], SYSEX_CABLE2[1]);
        let mut buf = [0u8; 64];
        assert_eq!(drain_packets_to_host(&mut buf), 4);
        assert_eq!(buf[..4], SYSEX_CABLE2[2]);
        assert_eq!(drain_packets_to_host(&mut buf), 0);
        assert_eq!(tx_packet_pending(), 0);
    }
}
