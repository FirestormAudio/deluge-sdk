//! Fixed-width wire encoding for audio-graph [`Cmd`]s.
//!
//! Its reason to exist is the web simulator, where the VM thread ships
//! control-rate graph mutations to the AudioWorklet's render engine — a second
//! wasm instance with its own `Engine`. It lives here, beside the `Cmd` type it
//! serializes rather than beside that consumer, so it stays in step with the
//! command surface and so its round-trip tests run on the host (the web crate
//! only builds for wasm).
//!
//! Both ends are the same code compiled once, so the record layout is shared by
//! construction.
//!
//! ## Layout
//! Every record is [`REC`] bytes: a tag byte, then a per-variant payload laid
//! out by the `put_*` helpers. `REC` is sized by the largest payload —
//! `StreamFill`, which carries three `u64` cursors — and every other variant
//! simply leaves the tail zeroed. Fixed width keeps the ring arithmetic in
//! `lib.rs` a multiplication rather than a parse.
//!
//! An unknown tag decodes to `Cmd::Nop`. That is the forward-compatible
//! reading: a newer main thread talking to an older worklet drops the command
//! it cannot express rather than desynchronising the stream, because every
//! record is the same size.
//!
//! ## What does not survive the wire
//! `BindTable` with a **pooled** table round-trips its handle, but the sample
//! data it points at does not: the worklet has its own `Engine` and its own
//! pool, and the only channel into it is this command stream — there is no
//! upload path for pool contents. A pooled wavetable therefore renders from
//! whatever that pool holds, which on the worklet side is silence. Static
//! tables are compiled into both instances and are unaffected. Closing that
//! gap means a data channel, not a codec change.
//!
//! `Engine::apply_at` (G7) is not a `Cmd` and so has no encoding; scheduling on
//! the worklet's own clock would need a companion record carrying the sample
//! position. The web sim does not schedule.

use deluge_audio_graph::ids::CtrlBusId;
use deluge_audio_graph::node::{Rate, TableSrc};
use deluge_audio_graph::pool::PoolHandle;
use deluge_audio_graph::{BusId, Cmd, Input, Kind, NodeId, OutputSrc, TableId};

/// Bytes per command record. Sized by `StreamFill` (tag + node + voice + three
/// `u64`s = 28), rounded to 32.
pub const REC: usize = 32;

// Tags. Append only — these are a wire format, so a value never changes
// meaning. Gaps are fine; unknown tags decode to `Nop`.
const NOP: u8 = 0;
const NEW_NODE: u8 = 1;
const SET_INPUT: u8 = 2;
const GATE: u8 = 3;
const TRIGGER: u8 = 4;
const SET_ROOT: u8 = 5;
const RESET: u8 = 6;
const SET_PARAM: u8 = 7;
const BIND_TABLE: u8 = 8;
const GATE_VOICE: u8 = 9;
const TRIGGER_VOICE: u8 = 10;
const STREAM_FILL: u8 = 11;
const BUS_WRITE: u8 = 12;
const BUS_WRITE_GAINS: u8 = 13;
const SET_USB_OUT: u8 = 14;
const BUS_GAIN: u8 = 15;
const BUS_SEND: u8 = 16;
const SET_MASTER_LIMIT: u8 = 17;
const SET_MASTER_DC_BLOCK: u8 = 18;
const SET_MASTER_EQ: u8 = 19;
const SET_RATE: u8 = 20;
const MOVE_BEFORE: u8 = 21;
const MOVE_AFTER: u8 = 22;
const FREE: u8 = 23;
const BEGIN_UPDATE: u8 = 24;
const END_UPDATE: u8 = 25;
const CLEAR_SCHEDULE: u8 = 26;
const SET_CTRL: u8 = 27;
const CTRL_WRITE: u8 = 28;
const CLEAR_CTRL_WRITE: u8 = 29;
const MAP_PARAM: u8 = 30;
const UNMAP_PARAM: u8 = 31;

// ── Field helpers ────────────────────────────────────────────────────────────
// Each writes at a fixed offset so `encode`/`decode` stay symmetrical by
// inspection rather than by arithmetic.

fn put_u16(r: &mut [u8; REC], at: usize, v: u16) {
    r[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn get_u16(r: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([r[at], r[at + 1]])
}
fn put_u32(r: &mut [u8; REC], at: usize, v: u32) {
    r[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn get_u32(r: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([r[at], r[at + 1], r[at + 2], r[at + 3]])
}
fn put_u64(r: &mut [u8; REC], at: usize, v: u64) {
    r[at..at + 8].copy_from_slice(&v.to_le_bytes());
}
fn get_u64(r: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&r[at..at + 8]);
    u64::from_le_bytes(b)
}
fn put_f32(r: &mut [u8; REC], at: usize, v: f32) {
    put_u32(r, at, v.to_bits());
}
fn get_f32(r: &[u8], at: usize) -> f32 {
    f32::from_bits(get_u32(r, at))
}

/// An `Input` occupies 6 bytes: a variant tag, a `u8` (the port, else 0), and a
/// 4-byte payload.
fn put_input(r: &mut [u8; REC], at: usize, i: Input) {
    let (tag, port, val) = match i {
        Input::Const(c) => (0u8, 0u8, c.to_bits()),
        Input::Node { node, port } => (1, port, node.0 as u32),
        Input::Bus(b) => (2, 0, b.0 as u32),
        Input::CtrlBus(b) => (3, 0, b.0 as u32),
    };
    r[at] = tag;
    r[at + 1] = port;
    put_u32(r, at + 2, val);
}
fn get_input(r: &[u8], at: usize) -> Input {
    let val = get_u32(r, at + 2);
    match r[at] {
        1 => Input::Node {
            node: NodeId(val as u16),
            port: r[at + 1],
        },
        2 => Input::Bus(BusId(val as u16)),
        3 => Input::CtrlBus(CtrlBusId(val as u16)),
        // Tag 0 and anything unrecognised: a constant. An unknown input tag
        // cannot desync the stream (records are fixed width), and a constant is
        // the inert reading.
        _ => Input::Const(f32::from_bits(val)),
    }
}

/// An `OutputSrc` occupies 4 bytes: a variant tag, a port, and a `u16` id.
fn put_output_src(r: &mut [u8; REC], at: usize, s: OutputSrc) {
    let (tag, port, id) = match s {
        OutputSrc::Silent => (0u8, 0u8, 0u16),
        OutputSrc::BusL(b) => (1, 0, b.0),
        OutputSrc::BusR(b) => (2, 0, b.0),
        OutputSrc::Node { node, port } => (3, port, node.0),
    };
    r[at] = tag;
    r[at + 1] = port;
    put_u16(r, at + 2, id);
}
fn get_output_src(r: &[u8], at: usize) -> OutputSrc {
    let id = get_u16(r, at + 2);
    match r[at] {
        1 => OutputSrc::BusL(BusId(id)),
        2 => OutputSrc::BusR(BusId(id)),
        3 => OutputSrc::Node {
            node: NodeId(id),
            port: r[at + 1],
        },
        _ => OutputSrc::Silent,
    }
}

// ── Encode ───────────────────────────────────────────────────────────────────

/// Encode one command into a [`REC`]-byte record.
pub fn encode(c: Cmd) -> [u8; REC] {
    let mut r = [0u8; REC];
    match c {
        Cmd::Nop => r[0] = NOP,
        Cmd::NewNode { node, kind, args } => {
            r[0] = NEW_NODE;
            r[1] = kind.to_u8();
            put_u16(&mut r, 2, node.0);
            put_input(&mut r, 4, args[0]);
            put_input(&mut r, 10, args[1]);
            put_input(&mut r, 16, args[2]);
        }
        Cmd::SetInput { node, port, src } => {
            r[0] = SET_INPUT;
            r[1] = port;
            put_u16(&mut r, 2, node.0);
            put_input(&mut r, 4, src);
        }
        Cmd::SetParam { node, param, value } => {
            r[0] = SET_PARAM;
            r[1] = param;
            put_u16(&mut r, 2, node.0);
            put_f32(&mut r, 4, value);
        }
        Cmd::BindTable { node, src } => {
            r[0] = BIND_TABLE;
            put_u16(&mut r, 2, node.0);
            match src {
                TableSrc::Static(t) => {
                    r[1] = 0;
                    put_u16(&mut r, 4, t.0);
                }
                TableSrc::Pooled(h) => {
                    r[1] = 1;
                    put_u32(&mut r, 4, h.off());
                    put_u32(&mut r, 8, h.len());
                }
            }
        }
        Cmd::Gate { node, on } => {
            r[0] = GATE;
            r[1] = on as u8;
            put_u16(&mut r, 2, node.0);
        }
        Cmd::Trigger { node } => {
            r[0] = TRIGGER;
            put_u16(&mut r, 2, node.0);
        }
        Cmd::GateVoice { node, voice, on } => {
            r[0] = GATE_VOICE;
            r[1] = voice;
            put_u16(&mut r, 2, node.0);
            r[4] = on as u8;
        }
        Cmd::TriggerVoice { node, voice } => {
            r[0] = TRIGGER_VOICE;
            r[1] = voice;
            put_u16(&mut r, 2, node.0);
        }
        Cmd::StreamFill {
            node,
            voice,
            fill_lo,
            fill_hi,
            total,
        } => {
            r[0] = STREAM_FILL;
            r[1] = voice;
            put_u16(&mut r, 2, node.0);
            put_u64(&mut r, 4, fill_lo);
            put_u64(&mut r, 12, fill_hi);
            put_u64(&mut r, 20, total);
        }
        Cmd::BusWrite { src, bus } => {
            r[0] = BUS_WRITE;
            put_u16(&mut r, 2, bus.0);
            put_input(&mut r, 4, src);
        }
        Cmd::BusWriteGains { src, bus, gl, gr } => {
            r[0] = BUS_WRITE_GAINS;
            put_u16(&mut r, 2, bus.0);
            put_input(&mut r, 4, src);
            put_f32(&mut r, 10, gl);
            put_f32(&mut r, 14, gr);
        }
        Cmd::SetRoot { bus } => {
            r[0] = SET_ROOT;
            put_u16(&mut r, 2, bus.0);
        }
        Cmd::SetUsbOut { channel, src } => {
            r[0] = SET_USB_OUT;
            r[1] = channel;
            put_output_src(&mut r, 4, src);
        }
        Cmd::BusGain { bus, gain } => {
            r[0] = BUS_GAIN;
            put_u16(&mut r, 2, bus.0);
            put_f32(&mut r, 4, gain);
        }
        Cmd::BusSend { from, to, gain } => {
            r[0] = BUS_SEND;
            put_u16(&mut r, 2, from.0);
            put_u16(&mut r, 4, to.0);
            put_f32(&mut r, 8, gain);
        }
        Cmd::SetMasterLimit { ceiling, release } => {
            r[0] = SET_MASTER_LIMIT;
            put_f32(&mut r, 4, ceiling);
            put_f32(&mut r, 8, release);
        }
        Cmd::SetMasterDcBlock { cutoff_hz } => {
            r[0] = SET_MASTER_DC_BLOCK;
            put_f32(&mut r, 4, cutoff_hz);
        }
        Cmd::SetMasterEq {
            freq,
            gain_db,
            q,
            eq_type,
        } => {
            r[0] = SET_MASTER_EQ;
            r[1] = eq_type;
            put_f32(&mut r, 4, freq);
            put_f32(&mut r, 8, gain_db);
            put_f32(&mut r, 12, q);
        }
        Cmd::SetCtrl { bus, value } => {
            r[0] = SET_CTRL;
            put_u16(&mut r, 2, bus.0);
            put_f32(&mut r, 4, value);
        }
        Cmd::CtrlWrite { node, port, bus } => {
            r[0] = CTRL_WRITE;
            r[1] = port;
            put_u16(&mut r, 2, node.0);
            put_u16(&mut r, 4, bus.0);
        }
        Cmd::ClearCtrlWrite { bus } => {
            r[0] = CLEAR_CTRL_WRITE;
            put_u16(&mut r, 2, bus.0);
        }
        Cmd::MapParam { node, param, bus } => {
            r[0] = MAP_PARAM;
            r[1] = param;
            put_u16(&mut r, 2, node.0);
            put_u16(&mut r, 4, bus.0);
        }
        Cmd::UnmapParam { node, param } => {
            r[0] = UNMAP_PARAM;
            r[1] = param;
            put_u16(&mut r, 2, node.0);
        }
        Cmd::SetRate { node, rate } => {
            r[0] = SET_RATE;
            r[1] = matches!(rate, Rate::Control) as u8;
            put_u16(&mut r, 2, node.0);
        }
        Cmd::MoveBefore { node, target } => {
            r[0] = MOVE_BEFORE;
            put_u16(&mut r, 2, node.0);
            put_u16(&mut r, 4, target.0);
        }
        Cmd::MoveAfter { node, target } => {
            r[0] = MOVE_AFTER;
            put_u16(&mut r, 2, node.0);
            put_u16(&mut r, 4, target.0);
        }
        Cmd::Free { node } => {
            r[0] = FREE;
            put_u16(&mut r, 2, node.0);
        }
        Cmd::BeginUpdate => r[0] = BEGIN_UPDATE,
        Cmd::EndUpdate => r[0] = END_UPDATE,
        Cmd::ClearSchedule => r[0] = CLEAR_SCHEDULE,
        Cmd::Reset => r[0] = RESET,
    }
    r
}

// ── Decode ───────────────────────────────────────────────────────────────────

/// Decode one [`REC`]-byte record. An unknown tag yields [`Cmd::Nop`].
pub fn decode(r: &[u8]) -> Cmd {
    let node = NodeId(get_u16(r, 2));
    let bus = BusId(get_u16(r, 2));
    match r[0] {
        NEW_NODE => match Kind::from_u8(r[1]) {
            Some(kind) => Cmd::NewNode {
                node,
                kind,
                args: [get_input(r, 4), get_input(r, 10), get_input(r, 16)],
            },
            // An unknown kind byte cannot be guessed at: creating the wrong
            // node is worse than creating none.
            None => Cmd::Nop,
        },
        SET_INPUT => Cmd::SetInput {
            node,
            port: r[1],
            src: get_input(r, 4),
        },
        SET_PARAM => Cmd::SetParam {
            node,
            param: r[1],
            value: get_f32(r, 4),
        },
        BIND_TABLE => Cmd::BindTable {
            node,
            src: if r[1] == 1 {
                TableSrc::Pooled(PoolHandle::from_raw(get_u32(r, 4), get_u32(r, 8)))
            } else {
                TableSrc::Static(TableId(get_u16(r, 4)))
            },
        },
        GATE => Cmd::Gate {
            node,
            on: r[1] != 0,
        },
        TRIGGER => Cmd::Trigger { node },
        GATE_VOICE => Cmd::GateVoice {
            node,
            voice: r[1],
            on: r[4] != 0,
        },
        TRIGGER_VOICE => Cmd::TriggerVoice { node, voice: r[1] },
        STREAM_FILL => Cmd::StreamFill {
            node,
            voice: r[1],
            fill_lo: get_u64(r, 4),
            fill_hi: get_u64(r, 12),
            total: get_u64(r, 20),
        },
        BUS_WRITE => Cmd::BusWrite {
            src: get_input(r, 4),
            bus,
        },
        BUS_WRITE_GAINS => Cmd::BusWriteGains {
            src: get_input(r, 4),
            bus,
            gl: get_f32(r, 10),
            gr: get_f32(r, 14),
        },
        SET_ROOT => Cmd::SetRoot { bus },
        SET_USB_OUT => Cmd::SetUsbOut {
            channel: r[1],
            src: get_output_src(r, 4),
        },
        BUS_GAIN => Cmd::BusGain {
            bus,
            gain: get_f32(r, 4),
        },
        BUS_SEND => Cmd::BusSend {
            from: bus,
            to: BusId(get_u16(r, 4)),
            gain: get_f32(r, 8),
        },
        SET_MASTER_LIMIT => Cmd::SetMasterLimit {
            ceiling: get_f32(r, 4),
            release: get_f32(r, 8),
        },
        SET_MASTER_DC_BLOCK => Cmd::SetMasterDcBlock {
            cutoff_hz: get_f32(r, 4),
        },
        SET_MASTER_EQ => Cmd::SetMasterEq {
            freq: get_f32(r, 4),
            gain_db: get_f32(r, 8),
            q: get_f32(r, 12),
            eq_type: r[1],
        },
        SET_CTRL => Cmd::SetCtrl {
            bus: CtrlBusId(get_u16(r, 2)),
            value: get_f32(r, 4),
        },
        CTRL_WRITE => Cmd::CtrlWrite {
            node,
            port: r[1],
            bus: CtrlBusId(get_u16(r, 4)),
        },
        CLEAR_CTRL_WRITE => Cmd::ClearCtrlWrite {
            bus: CtrlBusId(get_u16(r, 2)),
        },
        MAP_PARAM => Cmd::MapParam {
            node,
            param: r[1],
            bus: CtrlBusId(get_u16(r, 4)),
        },
        UNMAP_PARAM => Cmd::UnmapParam { node, param: r[1] },
        SET_RATE => Cmd::SetRate {
            node,
            rate: if r[1] != 0 {
                Rate::Control
            } else {
                Rate::Audio
            },
        },
        MOVE_BEFORE => Cmd::MoveBefore {
            node,
            target: NodeId(get_u16(r, 4)),
        },
        MOVE_AFTER => Cmd::MoveAfter {
            node,
            target: NodeId(get_u16(r, 4)),
        },
        FREE => Cmd::Free { node },
        BEGIN_UPDATE => Cmd::BeginUpdate,
        END_UPDATE => Cmd::EndUpdate,
        CLEAR_SCHEDULE => Cmd::ClearSchedule,
        RESET => Cmd::Reset,
        _ => Cmd::Nop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(c: Cmd) -> Cmd {
        decode(&encode(c))
    }

    fn assert_round(c: Cmd) {
        assert_eq!(round(c), c, "did not survive the wire: {c:?}");
    }

    /// Every `Cmd` variant, with distinctive field values so a field written to
    /// the wrong offset shows up as a mismatch rather than coincidentally
    /// matching a zero.
    fn one_of_each() -> [Cmd; 31] {
        [
            Cmd::Nop,
            Cmd::NewNode {
                node: NodeId(513),
                kind: Kind::MoogLp4,
                args: [
                    Input::Const(440.5),
                    Input::Node {
                        node: NodeId(7),
                        port: 2,
                    },
                    Input::CtrlBus(CtrlBusId(9)),
                ],
            },
            Cmd::SetInput {
                node: NodeId(11),
                port: 2,
                src: Input::Bus(BusId(3)),
            },
            Cmd::SetParam {
                node: NodeId(12),
                param: 200,
                value: -0.75,
            },
            Cmd::BindTable {
                node: NodeId(13),
                src: TableSrc::Static(TableId(42)),
            },
            Cmd::BindTable {
                node: NodeId(14),
                src: TableSrc::Pooled(PoolHandle::from_raw(2048, 4096)),
            },
            Cmd::Gate {
                node: NodeId(15),
                on: true,
            },
            Cmd::Trigger { node: NodeId(16) },
            Cmd::GateVoice {
                node: NodeId(17),
                voice: 5,
                on: true,
            },
            Cmd::TriggerVoice {
                node: NodeId(18),
                voice: 6,
            },
            Cmd::StreamFill {
                node: NodeId(19),
                voice: 7,
                fill_lo: 1 << 40,
                fill_hi: (1 << 40) + 1234,
                total: u64::MAX,
            },
            Cmd::BusWrite {
                src: Input::Node {
                    node: NodeId(20),
                    port: 1,
                },
                bus: BusId(2),
            },
            Cmd::BusWriteGains {
                src: Input::Const(0.5),
                bus: BusId(3),
                gl: 0.25,
                gr: 0.75,
            },
            Cmd::SetRoot { bus: BusId(4) },
            Cmd::SetUsbOut {
                channel: 5,
                src: OutputSrc::Node {
                    node: NodeId(21),
                    port: 1,
                },
            },
            Cmd::SetUsbOut {
                channel: 6,
                src: OutputSrc::BusR(BusId(2)),
            },
            Cmd::BusGain {
                bus: BusId(5),
                gain: 0.6,
            },
            Cmd::BusSend {
                from: BusId(6),
                to: BusId(1),
                gain: 0.3,
            },
            Cmd::SetMasterLimit {
                ceiling: 0.98,
                release: 0.15,
            },
            Cmd::SetMasterDcBlock { cutoff_hz: 20.0 },
            Cmd::SetMasterEq {
                freq: 1000.0,
                gain_db: -3.0,
                q: 0.7,
                eq_type: 2,
            },
            Cmd::SetCtrl {
                bus: CtrlBusId(7),
                value: 0.42,
            },
            Cmd::CtrlWrite {
                node: NodeId(22),
                port: 1,
                bus: CtrlBusId(8),
            },
            Cmd::ClearCtrlWrite { bus: CtrlBusId(9) },
            Cmd::MapParam {
                node: NodeId(23),
                param: 3,
                bus: CtrlBusId(10),
            },
            Cmd::UnmapParam {
                node: NodeId(24),
                param: 4,
            },
            Cmd::SetRate {
                node: NodeId(25),
                rate: Rate::Control,
            },
            Cmd::MoveBefore {
                node: NodeId(26),
                target: NodeId(27),
            },
            Cmd::MoveAfter {
                node: NodeId(28),
                target: NodeId(29),
            },
            Cmd::Free { node: NodeId(30) },
            Cmd::Reset,
        ]
    }

    #[test]
    fn every_variant_survives_a_round_trip() {
        for c in one_of_each() {
            assert_round(c);
        }
        // The bracket commands carry no payload but must not collapse to Nop.
        assert_round(Cmd::BeginUpdate);
        assert_round(Cmd::EndUpdate);
        assert_round(Cmd::ClearSchedule);
    }

    #[test]
    fn each_variant_has_a_distinct_tag() {
        // A duplicated tag constant would make two commands indistinguishable
        // on the wire, and the round-trip test alone would not catch it (the
        // first matching decode arm would win and could still compare equal).
        //
        // Keyed by variant discriminant, not by list position: `one_of_each`
        // carries two `BindTable`s and two `SetUsbOut`s to cover their
        // sub-variants, and those *should* share a tag.
        use core::mem::{Discriminant, discriminant};
        let mut owner: [Option<Discriminant<Cmd>>; 256] = [None; 256];
        for c in one_of_each() {
            let tag = encode(c)[0] as usize;
            let d = discriminant(&c);
            match owner[tag] {
                Some(prev) => assert!(prev == d, "tag {tag} is used by two variants: {c:?}"),
                None => owner[tag] = Some(d),
            }
        }
    }

    #[test]
    fn every_input_variant_survives() {
        for src in [
            Input::Const(-12.5),
            Input::Node {
                node: NodeId(300),
                port: 2,
            },
            Input::Bus(BusId(5)),
            Input::CtrlBus(CtrlBusId(6)),
        ] {
            assert_round(Cmd::SetInput {
                node: NodeId(1),
                port: 0,
                src,
            });
        }
    }

    #[test]
    fn every_output_src_variant_survives() {
        for src in [
            OutputSrc::Silent,
            OutputSrc::BusL(BusId(1)),
            OutputSrc::BusR(BusId(2)),
            OutputSrc::Node {
                node: NodeId(3),
                port: 1,
            },
        ] {
            assert_round(Cmd::SetUsbOut { channel: 0, src });
        }
    }

    #[test]
    fn every_kind_survives_a_round_trip() {
        // `NewNode` is the only command carrying a `Kind`, and a wrong byte
        // would build a different node entirely.
        for k in deluge_audio_graph::node::ALL_KINDS {
            assert_round(Cmd::NewNode {
                node: NodeId(1),
                kind: k,
                args: [Input::Const(0.0); 3],
            });
        }
    }

    #[test]
    fn an_unknown_tag_decodes_to_nop() {
        // Forward compatibility: a newer sender's command is dropped, not
        // misread. Fixed-width records mean the stream stays aligned.
        let mut r = [0u8; REC];
        r[0] = 200;
        assert_eq!(decode(&r), Cmd::Nop);
    }

    #[test]
    fn an_unknown_kind_byte_does_not_create_a_node() {
        let mut r = encode(Cmd::NewNode {
            node: NodeId(1),
            kind: Kind::Saw,
            args: [Input::Const(0.0); 3],
        });
        r[1] = 254; // no such kind
        assert_eq!(decode(&r), Cmd::Nop, "guessing would build the wrong node");
    }

    #[test]
    fn the_record_holds_the_largest_payload() {
        // `StreamFill` is the widest: 4 + 8 + 8 + 8 = 28 bytes used.
        assert!(REC >= 28, "REC too small for StreamFill");
        // Every encode must stay inside the record; `encode` returns a fixed
        // array, so this is really a guard on the offsets in the helpers above.
        for c in one_of_each() {
            assert_eq!(encode(c).len(), REC);
        }
    }
}
