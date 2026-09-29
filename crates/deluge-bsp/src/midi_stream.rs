//! MIDI 1.0 byte stream ↔ USB MIDI 1.0 event packets (USB MIDI 1.0 §4).
//!
//! [`DinParser`] turns a DIN byte stream into 4-byte event packets;
//! [`payload_len`] gives how many MIDI bytes a received packet carries. Both
//! are pure, so firmware bridges share them and they are tested on the host.

/// Number of raw MIDI bytes for each Code Index Number (CIN).
/// Index `i` = byte count for CIN `i`.  0 = reserved/skip.
pub const CIN_BYTES: [u8; 16] = [
    0, // 0x0 reserved
    0, // 0x1 cable event / MTC (skip)
    2, // 0x2 two-byte system common
    3, // 0x3 three-byte system common (SPP)
    3, // 0x4 SysEx start/continue
    1, // 0x5 single SysEx end / tune request
    2, // 0x6 two-byte SysEx end
    3, // 0x7 three-byte SysEx end
    3, // 0x8 Note Off
    3, // 0x9 Note On
    3, // 0xA Poly Aftertouch
    3, // 0xB Control Change
    2, // 0xC Program Change
    2, // 0xD Channel Pressure
    3, // 0xE Pitch Bend
    1, // 0xF single byte (realtime)
];

/// MIDI bytes carried by a USB MIDI event packet with this Code Index Number.
/// 0 for reserved and cable-event CINs, which carry nothing to forward.
pub fn payload_len(cin: u8) -> usize {
    CIN_BYTES[(cin & 0x0F) as usize] as usize
}

/// MIDI DIN RX parser state.
#[derive(Default)]
pub struct DinParser {
    /// Accumulated message bytes (status + up to 2 data).
    buf: [u8; 3],
    buf_pos: usize,
    /// Last channel-voice / system-common status byte (running status).
    running_status: u8,
    /// True while accumulating a SysEx message.
    in_sysex: bool,
    /// SysEx accumulation buffer (filled in 3-byte chunks).
    sysex_buf: [u8; 3],
    sysex_pos: usize,
}

impl DinParser {
    /// Push one raw byte from the DIN stream.
    ///
    /// Returns `Some([cin, b0, b1, b2])` when a complete USB MIDI packet is
    /// ready to be written; `None` if more bytes are needed.
    pub fn push(&mut self, byte: u8) -> Option<[u8; 4]> {
        // Real-time messages (0xF8–0xFF) pass through immediately.
        if byte >= 0xF8 {
            return Some([0x0F, byte, 0, 0]);
        }

        // ---- SysEx body accumulation ----------------------------------------
        if self.in_sysex {
            if byte & 0x80 == 0 {
                // SysEx data byte.
                self.sysex_buf[self.sysex_pos] = byte;
                self.sysex_pos += 1;
                if self.sysex_pos == 3 {
                    let pkt = [
                        0x04,
                        self.sysex_buf[0],
                        self.sysex_buf[1],
                        self.sysex_buf[2],
                    ];
                    self.sysex_pos = 0;
                    return Some(pkt);
                }
                return None;
            } else if byte == 0xF7 {
                // End of SysEx.
                self.in_sysex = false;
                let pkt = match self.sysex_pos {
                    0 => [0x05, 0xF7, 0, 0],
                    1 => [0x06, self.sysex_buf[0], 0xF7, 0],
                    2 => [0x07, self.sysex_buf[0], self.sysex_buf[1], 0xF7],
                    _ => unreachable!(),
                };
                self.sysex_pos = 0;
                return Some(pkt);
            } else {
                // Non-RT status inside SysEx — abort SysEx, fall through.
                self.in_sysex = false;
                self.sysex_pos = 0;
                self.running_status = 0;
            }
        }

        // ---- Status byte ----------------------------------------------------
        if byte & 0x80 != 0 {
            if byte == 0xF0 {
                self.in_sysex = true;
                self.sysex_buf[0] = 0xF0;
                self.sysex_pos = 1;
                self.running_status = 0;
                self.buf_pos = 0;
                return None;
            }
            if byte >= 0xF0 {
                // Other system-common (0xF1–0xF6) — clear running status.
                self.running_status = 0;
            } else {
                // Channel voice message — update running status.
                self.running_status = byte;
            }
            self.buf[0] = byte;
            self.buf_pos = 1;
            return None;
        }

        // ---- Data byte ------------------------------------------------------
        if self.running_status == 0 {
            return None; // no active status — discard
        }
        if self.buf_pos == 0 {
            // Running-status re-insert.
            self.buf[0] = self.running_status;
            self.buf_pos = 1;
        }
        self.buf[self.buf_pos] = byte;
        self.buf_pos += 1;

        // ---- Check completeness ---------------------------------------------
        let status = self.buf[0];
        let (required, cin) = message_size(status);

        if required > 0 && self.buf_pos >= required {
            self.buf_pos = 0;
            let b1 = self.buf[0];
            let b2 = if required >= 2 { self.buf[1] } else { 0 };
            let b3 = if required >= 3 { self.buf[2] } else { 0 };
            return Some([cin, b1, b2, b3]);
        }
        None
    }
}

/// Returns `(byte_count, cin)` for a given status byte.
/// Returns `(0, 0)` for unrecognised bytes.
fn message_size(status: u8) -> (usize, u8) {
    match status & 0xF0 {
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => (3, (status >> 4) & 0x0F),
        0xC0 | 0xD0 => (2, (status >> 4) & 0x0F),
        0xF0 => match status {
            0xF2 => (3, 0x03), // Song Position Pointer
            0xF3 => (2, 0x02), // Song Select
            0xF6 => (1, 0x05), // Tune Request
            _ => (0, 0),
        },
        _ => (0, 0),
    }
}
