//! Portable `no_std` block-rendering audio graph engine.
//!
//! Nodes (a uniform enum wrapping `deluge-dsp-kernels` structs) evaluate in
//! **creation order** into a pooled output arena; ports are addressed `(node,
//! port)`; buses carry stereo bundles + fan-in; a `Host` transports control-rate
//! `Cmd`s to the engine, and the engine reports `Event`s back. Sizes are
//! const-generic; sample rate is runtime.
//!
//! The engine does **not** sort the graph. Eval order is the order nodes were
//! created (see [`arena`]); building a patch in dependency order is the
//! author's obligation. A node that reads a source created after it sees that
//! source's *previous* block.
#![no_std]

pub mod arena;
pub mod bus;
pub mod cmd;
pub mod engine;
pub mod event;
pub mod frame;
pub mod ids;
pub mod node;
pub mod pool;
pub mod stream;
pub mod voice;

pub use arena::Arena;
pub use cmd::{Cmd, Host};
pub use deluge_dsp_kernels::poly::VOICES;
pub use engine::Engine;
pub use event::{EVENT_QUEUE, Event, EventQueue};
pub use frame::StereoFrame;
pub use ids::{BusId, Input, NodeId, OutputSrc, USB_CHANNELS};
pub use node::{In, Kind, Node};
pub use pool::{Pool, PoolHandle};
pub use voice::{MAX_GATES, MAX_TRIGGERS, MonoAllocator, VoiceAllocator};
