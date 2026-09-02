//! Public identifiers and the connection type. `NodeId` is the only id an author
//! or the wire ever sees; output-slot indices are engine-internal.

/// A compute node in the arena.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NodeId(pub u16);

/// A stereo bus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BusId(pub u16);

/// A mono control bus: one persistent value, not a block of samples (G6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CtrlBusId(pub u16);

/// The number of mono control buses (G6). A plain const rather than a seventh
/// `Engine` const-generic parameter; at 4 bytes each the whole space costs 128
/// bytes, which is not worth another type parameter on an already-wide
/// signature. Out-of-range ids are inert (reads give 0.0, writes are dropped).
pub const CTRL_BUSES: usize = 32;

/// A node input: a constant, a source node's output port, an audio bus, or a
/// control bus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Const(f32),
    Node {
        node: NodeId,
        port: u8,
    },
    Bus(BusId),
    /// A control bus, read as a single value for the whole block
    /// ([`crate::In::K`]), so the consuming kernel takes its `as_const` fast
    /// path. Like [`Input::Bus`], the value read is the one standing at the
    /// *start* of the block — see `docs/audio-graph-vs-scsynth.md` §G6.
    CtrlBus(CtrlBusId),
}

/// The number of routable USB output channels (mono).
pub const USB_CHANNELS: usize = 8;

/// A mono source feeding one USB output channel (IO-4). `Silent` = off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutputSrc {
    Silent,
    /// A bus's left row (e.g. `drums.left`).
    BusL(BusId),
    /// A bus's right row.
    BusR(BusId),
    /// A node's output port (already mono).
    Node {
        node: NodeId,
        port: u8,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_variants_are_copy_and_constructible() {
        let a = Input::Const(0.5);
        let b = Input::Node {
            node: NodeId(3),
            port: 2,
        };
        let c = Input::Bus(BusId(1));
        // Copy check
        let _copies = (a, b, c);
        match b {
            Input::Node { node, port } => {
                assert_eq!(node, NodeId(3));
                assert_eq!(port, 2);
            }
            _ => panic!("wrong variant"),
        }
    }
}
