//! Portable `no_std` block-rendering audio graph engine.
//!
//! Nodes (a uniform enum wrapping `deluge-dsp-kernels` structs) evaluate in
//! **topological order** into a pooled output arena; ports are addressed
//! `(node, port)`; buses carry stereo bundles + fan-in; a `Host` transports
//! control-rate `Cmd`s to the engine, and the engine reports `Event`s back.
//! Sizes are const-generic; sample rate is runtime.
//!
//! Eval order is sorted when the graph's shape changes, so a node always sees
//! its sources' current block no matter what order they were created in (G11,
//! see [`arena::Arena::sort`]). Exceptions, both deliberate: a feedback cycle
//! keeps its authored order, and an `Input::Bus` read is one block delayed by
//! design.
//!
//! A patch can be updated incrementally rather than rebuilt: between
//! `Cmd::BeginUpdate` and `Cmd::EndUpdate`, re-emitting a node keeps it and its
//! DSP state, and anything the update did not re-emit is swept (GL2).
#![no_std]

pub mod arena;
pub mod bus;
pub mod cmd;
pub mod ctrl;
pub mod engine;
pub mod event;
pub mod frame;
pub mod ids;
pub mod node;
pub mod pool;
pub mod sched;
pub mod stream;
pub mod voice;

pub use arena::Arena;
pub use cmd::{Cmd, Host};
pub use deluge_dsp_kernels::poly::VOICES;
pub use deluge_dsp_kernels::wavetable::TableId;
pub use engine::Engine;
pub use event::{EVENT_QUEUE, Event, EventQueue};
pub use frame::StereoFrame;
pub use ids::{BusId, Input, NodeId, OutputSrc, USB_CHANNELS};
pub use node::{In, Kind, Node, Rate};
pub use pool::{Pool, PoolHandle};
pub use voice::{MAX_GATES, MAX_TRIGGERS, MonoAllocator, VoiceAllocator};
