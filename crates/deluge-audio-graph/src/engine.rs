//! The block-rendering engine. Owns the arena and the per-slot output arena and
//! evaluates nodes in eval order — **topologically sorted** whenever the graph's
//! shape changes (see [`crate::arena::Arena::sort`]) — a block at a time.
//!
//! ## Borrow model (spec §3.5)
//! The output arena is one `UnsafeCell<[[f32; BLOCK]; OUTS]>`. Each `render_block`
//! iteration first *resolves* the node's inputs — copying every source (a
//! constant, another slot's block, or a bus) into local `scratch` — and only then
//! writes the node's own slot-run. Memory-safety comes from this copy-out
//! discipline: every read is copied into `scratch` before the mutable-write
//! `unsafe` deref is created, so the write borrow never overlaps a read. This
//! holds regardless of eval order — the topological sort is what makes the
//! *values* correct (so a node sees its inputs' current-block outputs), not what
//! makes the borrow sound.

use core::cell::UnsafeCell;

use deluge_dsp_kernels::In;
use deluge_dsp_kernels::eq::MasterEq;
use deluge_dsp_kernels::filter::MasterDcBlock;
use deluge_dsp_kernels::limiter::MasterLimiter;
use deluge_dsp_kernels::poly::VOICES;

use crate::arena::Arena;
use crate::event::CmdError;
use crate::node::{Kind, MAX_BLOCK, MAX_INPUTS, OutView};
use crate::{BusId, Input, Node, NodeId, OutputSrc, StereoFrame, USB_CHANNELS};

pub struct Engine<
    const BLOCK: usize,
    const NODES: usize,
    const OUTS: usize,
    const BUSES: usize,
    const PCAP: usize,
    const PCHUNK: usize,
> {
    arena: Arena<NODES, OUTS>,
    outs: UnsafeCell<[[f32; BLOCK]; OUTS]>,
    dt: f32,
    pub(crate) bus_l: [[f32; BLOCK]; BUSES],
    pub(crate) bus_r: [[f32; BLOCK]; BUSES],
    // Per-bus mono gain (channel fader), default 1.0. Applied to each bus's rows
    // after the write loop, before the master chain.
    pub(crate) bus_gain: [f32; BUSES],
    // Bus→bus sends: (from, to, gain), applied after the node-write loop,
    // descending `from` id (rule: from > to). Stereo-preserving, pre-fader.
    bus_sends: [Option<(BusId, BusId, f32)>; NODES],
    bus_sends_len: usize,
    // Stereo line-in for the current block, filled by `render` from its `input`
    // arg and read by `render_block` for `Kind::Input` nodes.
    pub(crate) in_l: [f32; BLOCK],
    pub(crate) in_r: [f32; BLOCK],
    // Bus rendered to the audio output; set via `set_root`, read in `render`.
    pub(crate) root: Option<BusId>,
    // Pending bus writes, re-applied every `render` (P0: no persistent routing
    // table yet — see spec §3.5 / bus.rs).
    writes: [Option<(Input, BusId, f32, f32)>; NODES],
    writes_len: usize,
    pool: crate::pool::Pool<PCAP, PCHUNK>,
    // Per-`StreamPlayer`-node fill cursors (produced by the prefetch task via
    // `Cmd::StreamFill`, consumed at render). Keyed by node index.
    stream_state: [Option<crate::stream::StreamCursors>; NODES],
    // USB output routing (IO-4): each of USB_CHANNELS mono slots routes from a
    // bus side or a node port; read by `fill_usb`. Default `Silent`.
    usb_out: [OutputSrc; USB_CHANNELS],
    // Opt-in master limiter on the root bus, applied at the render seam before
    // the output clamp. `None` = disabled (render path byte-unchanged).
    master_limiter: Option<MasterLimiter>,
    // Opt-in master DC-blocker on the root bus, applied at the render seam BEFORE
    // the limiter. `None` = disabled (render path byte-unchanged).
    master_dcblock: Option<MasterDcBlock>,
    // Opt-in master EQ on the root bus, applied at the render seam between the
    // DC-block and the limiter. `None` = disabled (render path byte-unchanged).
    master_eq: Option<MasterEq>,
    // Envelope-completion tracking (G1). Previous block's `Node::idle_mask` per
    // node slot; diffed at the end of `render_block` so completion is reported
    // as a rising edge, never a level. Seeded on create (see `seed_prev_idle`)
    // so a freshly created envelope never announces a release it never played.
    prev_idle: [u32; NODES],
    // Engine → host events drained by the host after render. See `event.rs`.
    events: crate::event::EventQueue,
    // Samples elapsed since construction (or the last `Cmd::Reset`), advanced
    // by `BLOCK` per `render_block`. The engine's only time source: scheduled
    // commands timestamp against it, and a host converts musical time (bars,
    // ms) to sample positions on its side of the `Cmd` seam.
    sample_clock: u64,
    // Set whenever the graph's shape changes (a node created or freed, an input
    // rewired). `render_block` re-sorts eval order when it is set, so a burst of
    // edits inside one block costs one sort. Explicit `move_before` /
    // `move_after` deliberately do NOT set it: a move is an author's decision
    // and holds until the graph's shape changes again.
    graph_dirty: bool,
    // Opt-in: skip nodes that reach no output root (G11). Off by default — the
    // engine cannot see what the host reads (`node_output`, `fill_usb`, a
    // prefetch cursor), so deciding on its own that a node is pointless would
    // silently freeze a node someone is legitimately reading.
    cull: bool,
    // Which nodes reach an output root, recomputed with the sort. Meaningless
    // (and unread) while `cull` is false.
    reachable: [bool; NODES],
    // Incremental patch update (GL2). `epoch` advances on `BeginUpdate`;
    // `node_epoch[i]` records the epoch in which node `i` was last (re-)emitted.
    // `EndUpdate` frees every live node still carrying an older epoch.
    in_update: bool,
    epoch: u8,
    node_epoch: [u8; NODES],
}

impl<
    const BLOCK: usize,
    const NODES: usize,
    const OUTS: usize,
    const BUSES: usize,
    const PCAP: usize,
    const PCHUNK: usize,
> Engine<BLOCK, NODES, OUTS, BUSES, PCAP, PCHUNK>
{
    pub fn new(sample_rate: f32) -> Self {
        assert!(BLOCK <= MAX_BLOCK, "BLOCK exceeds MAX_BLOCK");
        Engine {
            arena: Arena::new(),
            outs: UnsafeCell::new([[0.0; BLOCK]; OUTS]),
            dt: 1.0 / sample_rate,
            bus_l: [[0.0; BLOCK]; BUSES],
            bus_r: [[0.0; BLOCK]; BUSES],
            bus_gain: [1.0; BUSES],
            bus_sends: [None; NODES],
            bus_sends_len: 0,
            in_l: [0.0; BLOCK],
            in_r: [0.0; BLOCK],
            root: None,
            writes: [None; NODES],
            writes_len: 0,
            pool: crate::pool::Pool::new(),
            stream_state: [None; NODES],
            usb_out: [OutputSrc::Silent; USB_CHANNELS],
            master_limiter: None,
            master_dcblock: None,
            master_eq: None,
            prev_idle: [0; NODES],
            events: crate::event::EventQueue::new(),
            sample_clock: 0,
            // A fresh engine has no nodes, so there is nothing to sort.
            graph_dirty: false,
            cull: false,
            reachable: [false; NODES],
            in_update: false,
            epoch: 0,
            node_epoch: [0; NODES],
        }
    }

    /// Samples elapsed since construction or the last [`Cmd::Reset`].
    ///
    /// Advances by `BLOCK` per [`Self::render_block`], so it names the first
    /// sample of the *next* block. Free-running: it is not wall-clock and does
    /// not follow a host transport — a host that needs bar or millisecond
    /// positions converts against this on its own side.
    pub fn sample_time(&self) -> u64 {
        self.sample_clock
    }

    /// Enqueue a build-time command failure for the host (see
    /// [`crate::event::Event::CmdFailed`]).
    fn fail(&mut self, node: NodeId, reason: CmdError) {
        self.events
            .push(crate::event::Event::CmdFailed { node, reason });
    }

    /// Seed a node's previous-idle mask from its current state, so a freshly
    /// created envelope (which reads as idle before its first gate) does not
    /// register a spurious rising edge on its first render.
    fn seed_prev_idle(&mut self, id: NodeId) {
        let idx = id.0 as usize;
        if idx < NODES {
            self.prev_idle[idx] = self.arena.node(id).and_then(|n| n.idle_mask()).unwrap_or(0);
        }
    }

    /// Dequeue one engine event, oldest first. Drain until `None` after each
    /// `render` to keep the queue from overflowing.
    pub fn pop_event(&mut self) -> Option<crate::event::Event> {
        self.events.pop()
    }

    /// Drain every pending event through `f`, oldest first.
    pub fn drain_events(&mut self, mut f: impl FnMut(crate::event::Event)) {
        while let Some(ev) = self.events.pop() {
            f(ev);
        }
    }

    /// Events lost to queue overflow since construction. Non-zero means the
    /// host is not draining every block, or `EVENT_QUEUE` is undersized.
    pub fn events_dropped(&self) -> u16 {
        self.events.dropped()
    }

    pub fn pool_alloc(&mut self, len: usize) -> Option<crate::pool::PoolHandle> {
        self.pool.alloc(len)
    }
    pub fn pool_free(&mut self, h: crate::pool::PoolHandle) {
        self.pool.free(h)
    }
    pub fn pool_slice(&self, h: crate::pool::PoolHandle) -> &[f32] {
        self.pool.slice(h)
    }
    pub fn pool_slice_mut(&mut self, h: crate::pool::PoolHandle) -> &mut [f32] {
        self.pool.slice_mut(h)
    }

    pub fn create(&mut self, id: NodeId, kind: crate::node::Kind) -> bool {
        let ok = self.arena.create(id, kind);
        if ok {
            self.seed_prev_idle(id);
            self.graph_dirty = true;
        }
        ok
    }

    /// Skip nodes that cannot reach an output when rendering (G11).
    ///
    /// Off by default. An "output root" is a node feeding a bus write or a USB
    /// output channel; reachability is transitive through input edges. With
    /// culling on, an unreachable node is not evaluated at all: its output row
    /// holds whatever it last wrote and its DSP state stops advancing, so
    /// `node_output` on such a node is stale by design. Turn it on when the
    /// host's only outputs are the buses and the USB map.
    pub fn set_cull_unreachable(&mut self, on: bool) {
        self.cull = on;
        // The reachable set is computed alongside the sort, so a fresh opt-in
        // must force that pass — otherwise everything reads as unreachable.
        self.graph_dirty = true;
    }

    /// Recompute which nodes reach an output root. Roots are the nodes feeding
    /// bus writes and USB output channels; reachability then walks backwards
    /// along input edges to a fixpoint (the set only grows, so it terminates).
    fn compute_reachable(&mut self) {
        let mut r = [false; NODES];
        for w in 0..self.writes_len {
            if let Some((Input::Node { node, .. }, ..)) = self.writes[w] {
                if (node.0 as usize) < NODES {
                    r[node.0 as usize] = true;
                }
            }
        }
        for o in self.usb_out {
            if let OutputSrc::Node { node, .. } = o {
                if (node.0 as usize) < NODES {
                    r[node.0 as usize] = true;
                }
            }
        }
        loop {
            let mut changed = false;
            for k in 0..self.arena.eval_order().len() {
                let id = self.arena.eval_order()[k];
                if !r[id as usize] {
                    continue;
                }
                let Some(n) = self.arena.node(NodeId(id)) else {
                    continue;
                };
                for inp in n.inputs_snapshot() {
                    if let Input::Node { node: src, .. } = inp {
                        let idx = src.0 as usize;
                        if idx < NODES && !r[idx] && self.arena.node(src).is_some() {
                            r[idx] = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        self.reachable = r;
    }

    /// Free one node and everything keyed to it: its pooled table region, its
    /// stream cursors, its idle history, its arena slot and its bus writes.
    ///
    /// The single free path — `Cmd::Free` and the `EndUpdate` sweep both land
    /// here, so a swept node can never be cleaned up less thoroughly than an
    /// explicitly freed one.
    fn free_node(&mut self, node: NodeId) {
        // Free a pooled table region (if bound) BEFORE reclaiming the node's
        // arena slot: `table_src()` reads through the node, which must still
        // be live.
        if let Some(n) = self.arena.node_mut(node) {
            if let Some(crate::node::TableSrc::Pooled(h)) = n.table_src() {
                self.pool.free(h);
            }
        }
        let idx = node.0 as usize;
        if idx < NODES {
            self.stream_state[idx] = None;
            // A recreated id re-seeds this on create; clearing here keeps a
            // dead slot from spuriously edging in between.
            self.prev_idle[idx] = 0;
        }
        self.arena.free(node);
        self.graph_dirty = true;
        // Invalidate this node's bus writes so a reused id inherits no stale
        // routing (IO-2a). Const/Bus-sourced writes are untouched.
        for w in self.writes.iter_mut() {
            if let Some((Input::Node { node: n, .. }, ..)) = w {
                if *n == node {
                    *w = None;
                }
            }
        }
    }

    /// Close an incremental update: sweep every node the update did not
    /// re-emit, announcing each one so the host can drop its id.
    fn end_update(&mut self) {
        self.in_update = false;
        for idx in 0..NODES {
            let id = NodeId(idx as u16);
            if self.arena.node(id).is_some() && self.node_epoch[idx] != self.epoch {
                self.free_node(id);
                self.events.push(crate::event::Event::Freed { node: id });
            }
        }
    }

    /// Record that `node` belongs to the patch as of the current epoch.
    fn stamp(&mut self, node: NodeId) {
        let idx = node.0 as usize;
        if idx < NODES {
            self.node_epoch[idx] = self.epoch;
        }
    }

    /// A node's `Kind`, or `None` if it is not live.
    pub fn kind_of(&self, node: NodeId) -> Option<crate::node::Kind> {
        self.arena.kind_of(node)
    }

    /// Mutable access to one input edge. Rewiring changes the graph's shape, so
    /// this marks eval order for re-sorting whether or not the caller writes
    /// through the reference — a spurious sort costs one pass, a missed one
    /// costs a block-stale read.
    pub fn node_input_mut(&mut self, id: NodeId, port: u8) -> Option<&mut Input> {
        self.graph_dirty = true;
        self.arena.node_mut(id)?.input_mut(port)
    }

    /// Set a node's evaluation rate. `false` (and no change) if the node is
    /// not live, or if [`Rate::Control`] is asked of a node wider than one
    /// output port.
    ///
    /// Poly (`VOICES`-wide) and stereo (2-wide) kinds are refused: their output
    /// is a multi-row tile, and broadcasting one control value across it is a
    /// separate design. Control rate is for modulation sources — LFOs,
    /// envelopes used as modulators, `Ctrl`, `SampleHold`, `Slew`, `Steps`,
    /// `Mtof`, the quantisers — and those are all width 1.
    ///
    /// Nothing stops you putting an oscillator or a filter at control rate;
    /// scsynth allows the same, and the result is the same kind of nonsense.
    /// See [`Rate`] for the `BLOCK * dt` and input sample-and-hold semantics.
    pub fn set_rate(&mut self, node: NodeId, rate: crate::node::Rate) -> bool {
        use crate::node::Rate;
        if rate == Rate::Control {
            match self.arena.kind_of(node) {
                Some(k) if Node::out_width(k) == 1 => {}
                _ => return false,
            }
        }
        match self.arena.node_mut(node) {
            Some(n) => {
                n.set_rate(rate);
                true
            }
            None => false,
        }
    }

    /// A node's current evaluation rate, or `None` if it is not live.
    pub fn rate_of(&self, node: NodeId) -> Option<crate::node::Rate> {
        self.arena.node(node).map(|n| n.rate())
    }

    /// Move `node` so it evaluates immediately before `target`. `false` if
    /// either id is not live, or if they are equal. Takes effect next render.
    pub fn move_before(&mut self, node: NodeId, target: NodeId) -> bool {
        self.arena.move_before(node, target)
    }

    /// Move `node` so it evaluates immediately after `target`. `false` if
    /// either id is not live, or if they are equal. Takes effect next render.
    pub fn move_after(&mut self, node: NodeId, target: NodeId) -> bool {
        self.arena.move_after(node, target)
    }

    /// Current eval order, as `NodeId` raw values.
    ///
    /// A pending sort is applied at the next [`Self::render_block`], so between
    /// a structural edit and that render this still shows the pre-sort order.
    /// A node reading a source that appears later here sees that source's
    /// *previous* block — after a render that can only happen inside a feedback
    /// cycle or under an explicit `move_before` / `move_after`.
    pub fn eval_order(&self) -> &[u16] {
        self.arena.eval_order()
    }

    /// Apply one control-rate `Cmd`, mutating the arena/engine state it names.
    pub fn apply(&mut self, cmd: crate::cmd::Cmd) {
        use crate::cmd::Cmd;
        match cmd {
            Cmd::Nop => {}
            Cmd::NewNode { node, kind, args } => {
                // Inside an update, re-emitting an unchanged node is not an
                // error — it is the script saying "this stage is still here".
                // Same kind keeps the running node and its DSP state; a
                // different kind cannot (there is no way to carry a saw's phase
                // into a square's), so it is rebuilt.
                if self.in_update && self.arena.node(node).is_some() {
                    if self.arena.kind_of(node) == Some(kind) {
                        if let Some(n) = self.arena.node_mut(node) {
                            for p in 0..crate::cmd::MAX_ARGS {
                                if let Some(slot) = n.input_mut(p as u8) {
                                    *slot = args[p];
                                }
                            }
                        }
                        self.stamp(node);
                        self.graph_dirty = true;
                        return;
                    }
                    // Replaced, not swept: the id stays live, so this emits no
                    // `Freed` — a host's gate wired to this id still works.
                    self.free_node(node);
                }
                if self.arena.create(node, kind) {
                    if let Some(n) = self.arena.node_mut(node) {
                        for p in 0..crate::cmd::MAX_ARGS {
                            if let Some(slot) = n.input_mut(p as u8) {
                                *slot = args[p];
                            }
                        }
                    }
                    self.seed_prev_idle(node);
                    self.stamp(node);
                    self.graph_dirty = true;
                } else {
                    self.fail(node, CmdError::CreateFailed);
                }
            }
            Cmd::SetInput { node, port, src } => {
                if let Some(n) = self.arena.node_mut(node) {
                    if let Some(slot) = n.input_mut(port) {
                        *slot = src;
                    }
                    self.graph_dirty = true;
                } else {
                    self.fail(node, CmdError::DeadNode);
                }
            }
            Cmd::SetParam { node, param, value } => match self.arena.node_mut(node) {
                Some(n) => n.set_param(param, value),
                None => self.fail(node, CmdError::DeadNode),
            },
            Cmd::BindTable { node, src } => {
                if let Some(n) = self.arena.node_mut(node) {
                    n.bind_table(src);
                } else {
                    self.fail(node, CmdError::DeadNode);
                }
            }
            Cmd::Gate { node, on } => {
                if let Some(n) = self.arena.node_mut(node) {
                    n.gate(on);
                }
            }
            Cmd::Trigger { node } => {
                if let Some(n) = self.arena.node_mut(node) {
                    n.trigger();
                }
            }
            Cmd::GateVoice { node, voice, on } => {
                if let Some(n) = self.arena.node_mut(node) {
                    n.gate_voice(voice as usize, on);
                }
            }
            Cmd::TriggerVoice { node, voice } => {
                if let Some(n) = self.arena.node_mut(node) {
                    n.trigger_voice(voice as usize);
                }
            }
            Cmd::StreamFill {
                node,
                voice,
                fill_lo,
                fill_hi,
                total,
            } => {
                let idx = node.0 as usize;
                if idx < NODES && (voice as usize) < VOICES {
                    let sc = self.stream_state[idx]
                        .get_or_insert_with(crate::stream::StreamCursors::new);
                    sc.total = total;
                    sc.fill[voice as usize] = (fill_lo, fill_hi);
                }
            }
            Cmd::BusWrite { src, bus } => self.bus_write(src, bus),
            Cmd::BusWriteGains { src, bus, gl, gr } => self.bus_write_gains(src, bus, gl, gr),
            Cmd::SetRoot { bus } => self.set_root(bus),
            Cmd::SetUsbOut { channel, src } => {
                let c = channel as usize;
                if c < USB_CHANNELS {
                    self.usb_out[c] = src;
                    self.graph_dirty = true;
                }
            }
            Cmd::BusGain { bus, gain } => self.set_bus_gain(bus, gain),
            Cmd::BusSend { from, to, gain } => self.bus_send(from, to, gain),
            Cmd::SetMasterLimit { ceiling, release } => match &mut self.master_limiter {
                Some(lim) => {
                    lim.set_ceiling(ceiling);
                    lim.set_release(release);
                }
                None => self.master_limiter = Some(MasterLimiter::new(ceiling, release)),
            },
            Cmd::SetMasterDcBlock { cutoff_hz } => match &mut self.master_dcblock {
                Some(dc) => dc.set_cutoff(cutoff_hz, self.dt),
                None => self.master_dcblock = Some(MasterDcBlock::new(cutoff_hz, self.dt)),
            },
            Cmd::SetMasterEq {
                freq,
                gain_db,
                q,
                eq_type,
            } => match &mut self.master_eq {
                Some(eq) => eq.set_params(freq, gain_db, q, eq_type),
                None => self.master_eq = Some(MasterEq::new(freq, gain_db, q, eq_type)),
            },
            Cmd::SetRate { node, rate } => {
                if !self.set_rate(node, rate) {
                    // Distinguish "no such node" from "this node cannot run at
                    // control rate" — they send the host looking in different
                    // places.
                    let reason = if self.arena.node(node).is_some() {
                        CmdError::UnsupportedRate
                    } else {
                        CmdError::DeadNode
                    };
                    self.fail(node, reason);
                }
            }
            Cmd::MoveBefore { node, target } => {
                if !self.arena.move_before(node, target) {
                    self.fail(node, CmdError::DeadNode);
                }
            }
            Cmd::MoveAfter { node, target } => {
                if !self.arena.move_after(node, target) {
                    self.fail(node, CmdError::DeadNode);
                }
            }
            Cmd::Free { node } => self.free_node(node),
            Cmd::BeginUpdate => {
                self.in_update = true;
                self.epoch = self.epoch.wrapping_add(1);
            }
            Cmd::EndUpdate => self.end_update(),
            Cmd::Reset => {
                self.arena.reset();
                self.sample_clock = 0;
                self.graph_dirty = false;
                self.in_update = false;
                self.node_epoch = [0; NODES];
                self.writes = [None; NODES];
                self.writes_len = 0;
                self.root = None;
                self.stream_state = [None; NODES];
                self.usb_out = [OutputSrc::Silent; USB_CHANNELS];
                self.master_limiter = None;
                self.master_dcblock = None;
                self.master_eq = None;
                self.bus_gain = [1.0; BUSES];
                self.bus_sends = [None; NODES];
                self.bus_sends_len = 0;
                self.bus_l = [[0.0; BLOCK]; BUSES];
                self.bus_r = [[0.0; BLOCK]; BUSES];
                self.prev_idle = [0; NODES];
                self.events.clear();
            }
        }
    }

    /// Evaluate every live node in eval order into the output arena.
    pub fn render_block(&mut self) {
        self.sample_clock = self.sample_clock.wrapping_add(BLOCK as u64);
        // Eval order is topological by construction (G11): every node runs after
        // the nodes it reads. Sorting here rather than in `apply` means a whole
        // patch build costs one sort, not one per command.
        if self.graph_dirty {
            self.arena.sort();
            if self.cull {
                self.compute_reachable();
            }
            self.graph_dirty = false;
        }
        // Snapshot eval order so we don't borrow the arena across the loop.
        let mut order = [0u16; NODES];
        let live = {
            let eo = self.arena.eval_order();
            order[..eo.len()].copy_from_slice(eo);
            eo.len()
        };

        for k in 0..live {
            let id = NodeId(order[k]);
            if self.cull && !self.reachable[order[k] as usize] {
                continue; // reaches no output root — see `set_cull_unreachable`
            }
            let (base, kind, width, inputs, table_src, rate) = {
                let n = self.arena.node(id).expect("eval-order node exists");
                (
                    n.out_base as usize,
                    n.kind,
                    Node::out_width(n.kind),
                    n.inputs_snapshot(),
                    n.table_src(),
                    n.rate(),
                )
            };
            // Which input ports are fed by a control-rate node. Such a source's
            // row is constant across the block, so it is handed down as
            // `In::K` and the consuming kernel takes its `as_const` fast path
            // rather than indexing a row of identical values.
            let mut src_kr = [false; MAX_INPUTS];
            for (p, flag) in src_kr.iter_mut().enumerate() {
                if let Input::Node { node, .. } = inputs[p] {
                    *flag =
                        self.arena.node(node).map(|n| n.rate()) == Some(crate::node::Rate::Control);
                }
            }

            // ── Resolve inputs into scratch (all reads copied out first) ──
            let mut scratch = [[0.0f32; BLOCK]; MAX_INPUTS];
            let mut poly_scratch = [[[0.0f32; BLOCK]; VOICES]; 3];
            {
                // SAFETY: read-only view of the output arena; no writer is live.
                let arr = unsafe { &*self.outs.get() };
                for (p, row) in scratch.iter_mut().enumerate() {
                    match inputs[p] {
                        Input::Const(v) => row.fill(v),
                        Input::Node { node, port } => match self.arena.out_base(node) {
                            Some(sbase) if sbase + (port as usize) < OUTS => {
                                *row = arr[sbase + port as usize]
                            }
                            _ => *row = [0.0; BLOCK], // dangling ref or out-of-range port → silence (never panic, never slot-0 crosstalk)
                        },
                        Input::Bus(bus) => {
                            let b = bus.0 as usize;
                            for i in 0..BLOCK {
                                row[i] = self.bus_l[b][i] + self.bus_r[b][i];
                            }
                        }
                    }
                }

                for j in 0..Node::poly_in_count(kind) {
                    // Ports 0..k are poly edges. A poly (VOICES-wide) source's
                    // rows are copied verbatim (row-copy preserves the
                    // interleaved layout). A mono (width-1) source instead has
                    // its single output row splatted to every voice lane —
                    // copying VOICES rows from a width-1 producer would read
                    // adjacent-node garbage/silence past its one live slot.
                    match inputs[j] {
                        Input::Const(v) => {
                            for lane in &mut poly_scratch[j] {
                                lane.fill(v);
                            }
                        }
                        Input::Node { node, .. } => match self.arena.out_base_and_kind(node) {
                            Some((sbase, src_kind))
                                if Node::out_width(src_kind) == 1 && sbase < OUTS =>
                            {
                                for v in 0..VOICES {
                                    poly_scratch[j][v] = arr[sbase];
                                }
                            }
                            Some((sbase, _)) if sbase + VOICES <= OUTS => {
                                for v in 0..VOICES {
                                    poly_scratch[j][v] = arr[sbase + v];
                                }
                            }
                            _ => {
                                for v in 0..VOICES {
                                    poly_scratch[j][v] = [0.0; BLOCK];
                                }
                            }
                        },
                        _ => {
                            for v in 0..VOICES {
                                poly_scratch[j][v] = [0.0; BLOCK];
                            }
                        }
                    }
                }
            }
            // A control-rate node is evaluated once per block, so every one of
            // its inputs collapses to the block's first sample (scsynth's
            // `A2K` sample-and-hold). An audio-rate node keeps full rows,
            // except on ports fed by a control-rate source, which are already
            // constant and so cost nothing to pass as `In::K`.
            let kr = rate == crate::node::Rate::Control;
            let resolve = |p: usize| -> In<'_> {
                if kr || src_kr[p] {
                    In::K(scratch[p][0])
                } else {
                    In::A(&scratch[p][..])
                }
            };
            let ins = [resolve(0), resolve(1), resolve(2)];

            // ── Write this node's ports ──
            // SAFETY (deref soundness): every read for this iteration was already
            // copied into `scratch` above, before this mutable deref exists, so
            // the write region `[base, base+width)` cannot alias a live read —
            // regardless of eval order (wrong order would be a correctness bug,
            // not UB).
            // SAFETY (disjoint borrow): the raw deref of `self.outs` is not
            // tracked against `self`, so the borrow checker still allows the
            // following `self.arena.node_mut(id)` call (a borrow of the
            // disjoint `arena` field) to coexist with `arr`.
            let arr = unsafe { &mut *self.outs.get() };
            if Node::is_poly(kind) {
                // Poly path: voice-interleaved tile in/out, isolated dispatch.
                let count = Node::poly_in_count(kind);
                let poly_in: [Option<&[f32]>; 3] = [
                    if count > 0 {
                        Some(poly_scratch[0].as_flattened())
                    } else {
                        None
                    },
                    if count > 1 {
                        Some(poly_scratch[1].as_flattened())
                    } else {
                        None
                    },
                    if count > 2 {
                        Some(poly_scratch[2].as_flattened())
                    } else {
                        None
                    },
                ];
                let out = arr[base..base + width].as_flattened_mut(); // width*BLOCK
                // Resolve a pooled node's region MUTABLY before the node's
                // `&mut` borrow below, exactly as the mono path does at its
                // call site below — only `PolyWt`/`PolyWtMorph` read this;
                // every other poly kind ignores it.
                let pool_region: Option<&mut [f32]> = match table_src {
                    Some(crate::node::TableSrc::Pooled(h)) => Some(self.pool.slice_mut(h)),
                    _ => None,
                };
                // Stream fill cursors (a disjoint `Engine` field from `pool`/`arena`).
                let stream = {
                    let sidx = id.0 as usize;
                    if sidx < NODES {
                        self.stream_state[sidx].as_ref()
                    } else {
                        None
                    }
                };
                if let Some(n) = self.arena.node_mut(id) {
                    n.poly_process(&ins, poly_in, self.dt, out, pool_region, stream);
                }
            } else if kind == Kind::Input {
                // Stereo line-in: copy engine input rows into port0 (L) / port1 (R).
                // `self.in_l`/`in_r` are disjoint fields from `self.outs` (raw-ptr
                // borrow), same disjoint-field discipline as bus access.
                for i in 0..BLOCK {
                    arr[base][i] = self.in_l[i];
                    arr[base + 1][i] = self.in_r[i];
                }
            } else if kr && width == 1 {
                // ── Control rate: evaluate ONE sample, then broadcast it ──
                // The kernel is handed `BLOCK * dt` so time-based state (LFO
                // phase, envelope stages, slew) advances the same wall-clock
                // amount per block as it would at audio rate; passing `self.dt`
                // here would run every such node BLOCK times too slow.
                //
                // The row is still filled, because readers other than kernels
                // — bus writes, `node_output`, `fill_usb`, the poly-lane splat
                // — index it directly. Filling BLOCK floats is far cheaper
                // than BLOCK kernel evaluations, which is the whole point.
                let pool_region: Option<&mut [f32]> = match table_src {
                    Some(crate::node::TableSrc::Pooled(h)) => Some(self.pool.slice_mut(h)),
                    _ => None,
                };
                {
                    let row = &mut arr[base];
                    let mut view = OutView::single(&mut row[..1]);
                    if let Some(n) = self.arena.node_mut(id) {
                        n.process_resolved(&ins, BLOCK as f32 * self.dt, &mut view, pool_region);
                    }
                }
                let v = arr[base][0];
                arr[base][1..].fill(v);
            } else {
                let mut view = OutView::from_arena::<OUTS, BLOCK>(arr, base, width);
                // Resolve a pooled node's region MUTABLY (delay lines write it;
                // wavetables reborrow it immutably in `process_resolved`) BEFORE
                // the node's `&mut` borrow below — `self.pool` and `self.arena` are
                // separate fields of `Engine`, so the borrow checker tracks them
                // independently as long as each is accessed as a direct field
                // projection (not through a whole-`&mut self` helper method).
                let pool_region: Option<&mut [f32]> = match table_src {
                    Some(crate::node::TableSrc::Pooled(h)) => Some(self.pool.slice_mut(h)),
                    _ => None,
                };
                if let Some(n) = self.arena.node_mut(id) {
                    n.process_resolved(&ins, self.dt, &mut view, pool_region);
                }
            }
        }

        self.collect_events(&order[..live]);
    }

    /// Diff every live node's envelope-completion mask against last block's and
    /// enqueue an [`crate::Event`] per rising (not-idle → idle) lane.
    ///
    /// Rising-edge only: an envelope reads as idle both before its first gate
    /// and after its release decays, so a level would announce completions that
    /// never happened. `seed_prev_idle` covers the create-time case.
    fn collect_events(&mut self, order: &[u16]) {
        for &raw in order {
            let id = NodeId(raw);
            let idx = raw as usize;
            if idx >= NODES {
                continue;
            }
            // Disjoint field borrows: `self.arena` (shared) vs `self.prev_idle`
            // / `self.events` (mutable) — same discipline as the render loop.
            let (mask, poly) = match self.arena.node(id) {
                Some(n) => match n.idle_mask() {
                    Some(m) => (m, n.is_poly_env()),
                    None => continue, // not an envelope kind
                },
                None => continue,
            };
            let rising = mask & !self.prev_idle[idx];
            self.prev_idle[idx] = mask;
            if rising == 0 {
                continue;
            }
            if poly {
                for v in 0..VOICES {
                    if rising & (1 << v) != 0 {
                        self.events.push(crate::event::Event::VoiceDone {
                            node: id,
                            voice: v as u8,
                        });
                    }
                }
            } else if rising & 1 != 0 {
                self.events.push(crate::event::Event::Done { node: id });
            }
        }
    }

    /// A `StreamPlayer` voice's playback read-cursor (⌊pos⌋). `None` if `node`
    /// doesn't exist / isn't a `StreamPlayer` / `voice` is out of range. The
    /// prefetch task polls this to trail playback.
    pub fn stream_read_cursor(&self, node: NodeId, voice: usize) -> Option<u64> {
        self.arena.node(node)?.stream_read_cursor(voice)
    }

    /// Test/inspection accessor: a node's rendered output port.
    pub fn node_output(&self, id: NodeId, port: u8) -> &[f32] {
        let base = self.arena.out_base(id).expect("node exists");
        // SAFETY: shared read; no writer is live outside `render_block`.
        let arr = unsafe { &*self.outs.get() };
        &arr[base + port as usize][..]
    }

    /// Record a center (L = R) bus write — unchanged mono behavior.
    pub fn bus_write(&mut self, src: Input, bus: BusId) {
        self.bus_write_gains(src, bus, 1.0, 1.0);
    }

    /// Record a bus write with per-side gains (`gl` → L, `gr` → R). A stereo
    /// source routes as two of these: `(port0, 1, 0)` and `(port1, 0, 1)`.
    pub fn bus_write_gains(&mut self, src: Input, bus: BusId, gl: f32, gr: f32) {
        // Routing is what makes a node reachable, so this invalidates the
        // culling set just as an edge change invalidates eval order.
        self.graph_dirty = true;
        // A write is a routing statement, not an accumulator: re-stating the
        // same source→bus route updates its gains in place. Without this, a
        // patch re-run (GL2) would append a second entry and add 6 dB per edit.
        for w in 0..self.writes_len {
            if let Some((s, b, ..)) = self.writes[w] {
                if s == src && b.0 == bus.0 {
                    self.writes[w] = Some((src, bus, gl, gr));
                    return;
                }
            }
        }
        // Reuse a freed (`None`) slot first so free/patch cycles don't leak
        // slots; otherwise append. No holes exist without a prior `Free`, so a
        // fresh engine appends in the same order as before (byte-identical).
        for w in 0..self.writes_len {
            if self.writes[w].is_none() {
                self.writes[w] = Some((src, bus, gl, gr));
                return;
            }
        }
        if self.writes_len < self.writes.len() {
            self.writes[self.writes_len] = Some((src, bus, gl, gr));
            self.writes_len += 1;
        }
    }

    /// Select which bus is copied to the audio output by `render`.
    pub fn set_root(&mut self, bus: BusId) {
        self.root = Some(bus);
    }

    /// Set a bus's mono gain (bounds-guarded; out-of-range is a no-op).
    pub fn set_bus_gain(&mut self, bus: BusId, gain: f32) {
        let b = bus.0 as usize;
        if b < BUSES {
            self.bus_gain[b] = gain;
        }
    }

    /// Record a bus→bus send (reuses a freed slot first, else appends).
    pub fn bus_send(&mut self, from: BusId, to: BusId, gain: f32) {
        for s in 0..self.bus_sends_len {
            if self.bus_sends[s].is_none() {
                self.bus_sends[s] = Some((from, to, gain));
                return;
            }
        }
        if self.bus_sends_len < self.bus_sends.len() {
            self.bus_sends[self.bus_sends_len] = Some((from, to, gain));
            self.bus_sends_len += 1;
        }
    }

    /// Fill the USB output channels from their routed sources (IO-4). Call AFTER
    /// `render` — buses hold this block's final content and node outputs are still
    /// in the arena. Each channel is mono; a dangling/out-of-range source → silence.
    pub fn fill_usb(&self, usb: &mut [[f32; BLOCK]; USB_CHANNELS]) {
        // SAFETY: read-only view of the output arena; no writer is live here.
        let arr = unsafe { &*self.outs.get() };
        for ch in 0..USB_CHANNELS {
            match self.usb_out[ch] {
                OutputSrc::Silent => usb[ch] = [0.0; BLOCK],
                OutputSrc::BusL(b) => {
                    let bi = b.0 as usize;
                    usb[ch] = if bi < BUSES {
                        self.bus_l[bi]
                    } else {
                        [0.0; BLOCK]
                    };
                }
                OutputSrc::BusR(b) => {
                    let bi = b.0 as usize;
                    usb[ch] = if bi < BUSES {
                        self.bus_r[bi]
                    } else {
                        [0.0; BLOCK]
                    };
                }
                OutputSrc::Node { node, port } => match self.arena.out_base(node) {
                    Some(base) if base + (port as usize) < OUTS => {
                        usb[ch] = arr[base + port as usize]
                    }
                    _ => usb[ch] = [0.0; BLOCK],
                },
            }
        }
    }

    /// Render one block: clear buses, evaluate nodes, apply pending bus
    /// writes, then copy the root bus into `out`, clamped to `[-1, 1]`.
    pub fn render(&mut self, out: &mut [StereoFrame], input: &[StereoFrame]) {
        // Fill the stereo line-in rows from `input`; frames beyond `input.len()`
        // (including an empty slice) read as silence — no panic on any length.
        for i in 0..BLOCK {
            match input.get(i) {
                Some(f) => {
                    self.in_l[i] = f.l;
                    self.in_r[i] = f.r;
                }
                None => {
                    self.in_l[i] = 0.0;
                    self.in_r[i] = 0.0;
                }
            }
        }
        // Evaluate nodes FIRST, so an `Input::Bus` node-read sees the PREVIOUS
        // block's bus content (one-block-delayed) rather than a freshly-zeroed
        // bus — this is what lets a bus feed an effect node (aux returns).
        self.render_block();
        // NOW zero the buses and refill them from this block's node outputs.
        for b in 0..BUSES {
            self.bus_l[b] = [0.0; BLOCK];
            self.bus_r[b] = [0.0; BLOCK];
        }
        // Apply bus writes (per-side gains; mono center = (1,1)).
        let arr = unsafe { &*self.outs.get() };
        for w in 0..self.writes_len {
            if let Some((src, bus, gl, gr)) = self.writes[w] {
                let b = bus.0 as usize;
                for i in 0..BLOCK {
                    let v = match src {
                        Input::Const(c) => c,
                        Input::Node { node, port } => match self.arena.out_base(node) {
                            Some(base) if base + (port as usize) < OUTS => {
                                arr[base + port as usize][i]
                            }
                            _ => 0.0, // dangling ref or out-of-range port → contributes silence
                        },
                        Input::Bus(_) => 0.0, // bus→bus not in P0
                    };
                    self.bus_l[b][i] += v * gl;
                    self.bus_r[b][i] += v * gr;
                }
            }
        }
        // Per-bus gain (channel fader), applied BEFORE bus→bus sends so a send
        // carries the POST-fader signal (a channel's `gain` scales its master
        // contribution and its aux sends — console semantics). Default 1.0 =
        // no-op, byte-identical.
        for b in 0..BUSES {
            let g = self.bus_gain[b];
            if g != 1.0 {
                for i in 0..BLOCK {
                    self.bus_l[b][i] *= g;
                    self.bus_r[b][i] *= g;
                }
            }
        }
        // Bus→bus sends (stereo-preserving, POST-fader): fold source buses into
        // targets in descending `from` id (rule: from > to, master=0 = sink) so a
        // source is fully filled before it feeds a lower bus. No-op when none.
        for from in (0..BUSES).rev() {
            for s in 0..self.bus_sends_len {
                if let Some((f, t, g)) = self.bus_sends[s] {
                    let (fi, ti) = (f.0 as usize, t.0 as usize);
                    if fi == from && fi < BUSES && ti < BUSES && fi != ti {
                        for i in 0..BLOCK {
                            self.bus_l[ti][i] += self.bus_l[fi][i] * g;
                            self.bus_r[ti][i] += self.bus_r[fi][i] * g;
                        }
                    }
                }
            }
        }
        // Master chain (opt-in): DC-block → limiter, on the root bus before the clamp.
        if let Some(root) = self.root {
            let b = root.0 as usize;
            if let Some(dc) = &mut self.master_dcblock {
                dc.process(&mut self.bus_l[b], &mut self.bus_r[b]);
            }
            if let Some(eq) = &mut self.master_eq {
                eq.process(&mut self.bus_l[b], &mut self.bus_r[b], self.dt);
            }
            if let Some(lim) = &mut self.master_limiter {
                lim.process(&mut self.bus_l[b], &mut self.bus_r[b], self.dt);
            }
        }
        // Copy root bus to output, clamped.
        let n = out.len().min(BLOCK);
        if let Some(root) = self.root {
            let b = root.0 as usize;
            for i in 0..n {
                out[i].l = self.bus_l[b][i].clamp(-1.0, 1.0);
                out[i].r = self.bus_r[b][i].clamp(-1.0, 1.0);
            }
        } else {
            for i in 0..n {
                out[i] = StereoFrame::default();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Kind, TableSrc};
    use crate::{Cmd, Input, NodeId, OutputSrc, USB_CHANNELS};

    type E = Engine<16, 8, 8, 4, 45056, 2048>;

    #[test]
    fn stream_player_renders_and_cursors() {
        // Two VOICES-wide (8-row) nodes need OUTS room for 16 output rows.
        type SE = Engine<16, 8, 128, 4, 45056, 2048>;
        let mut e = SE::new(16.0);
        let cap = 64usize;
        let node = NodeId(0);
        let h = e.pool_alloc(VOICES * cap).expect("ring pool");
        e.create(node, Kind::StreamPlayer);
        e.apply(Cmd::BindTable {
            node,
            src: TableSrc::Pooled(h),
        });
        e.apply(Cmd::SetParam {
            node,
            param: 0,
            value: 60.0,
        }); // root note
        // Mock prefetch: fill voice 0's sub-ring [0..cap) so sample `a` == a.
        {
            let region = e.pool_slice_mut(h);
            for a in 0..cap {
                region[a] = a as f32;
            }
        }
        e.apply(Cmd::StreamFill {
            node,
            voice: 0,
            fill_lo: 0,
            fill_hi: cap as u64,
            total: cap as u64,
        });
        e.apply(Cmd::TriggerVoice { node, voice: 0 });
        // pitch == root Hz (mtof(60) ≈ 261.63) → rate ≈ 1.0.
        *e.node_input_mut(node, 0).unwrap() = Input::Const(261.625_58);
        e.render_block();
        assert!(
            e.stream_read_cursor(node, 0).unwrap() > 0,
            "read cursor advanced on a full window"
        );

        // Underrun: a short window (fill_hi = 4) reads a couple samples then holds.
        let node2 = NodeId(1);
        let h2 = e.pool_alloc(VOICES * cap).expect("ring pool 2");
        e.create(node2, Kind::StreamPlayer);
        e.apply(Cmd::BindTable {
            node: node2,
            src: TableSrc::Pooled(h2),
        });
        e.apply(Cmd::SetParam {
            node: node2,
            param: 0,
            value: 60.0,
        });
        {
            let r = e.pool_slice_mut(h2);
            for a in 0..cap {
                r[a] = a as f32;
            }
        }
        e.apply(Cmd::StreamFill {
            node: node2,
            voice: 0,
            fill_lo: 0,
            fill_hi: 4,
            total: 1000,
        });
        e.apply(Cmd::TriggerVoice {
            node: node2,
            voice: 0,
        });
        *e.node_input_mut(node2, 0).unwrap() = Input::Const(261.625_58);
        e.render_block();
        assert!(
            e.stream_read_cursor(node2, 0).unwrap() <= 2,
            "held under underrun"
        );

        // No panic on out-of-range node / voice.
        e.apply(Cmd::StreamFill {
            node: NodeId(999),
            voice: 99,
            fill_lo: 0,
            fill_hi: 0,
            total: 0,
        });
        e.apply(Cmd::StreamFill {
            node,
            voice: 99,
            fill_lo: 0,
            fill_hi: 0,
            total: 0,
        });
        assert!(e.stream_read_cursor(NodeId(999), 0).is_none());
        assert!(e.stream_read_cursor(node, 99).is_none());
    }

    #[test]
    fn single_saw_node_renders() {
        let mut e = E::new(16.0); // sr so 4 Hz → 0.25/sample
        e.create(NodeId(0), Kind::Saw);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(4.0);
        e.render_block();
        let out = e.node_output(NodeId(0), 0);
        // Kernel-agnostic on purpose: this test verifies graph wiring (a
        // Kind::Saw node renders to its output port), not the oscillator's
        // exact samples. The naive vs. band-limited kernel shape is covered
        // by deluge-dsp-kernels; here we only check the output is a finite,
        // saw-like signal that swings through both polarities within the
        // expected [-1, 1] range (band-limiting can reduce peak amplitude
        // relative to the naive ramp, so thresholds are intentionally loose).
        assert!(out.iter().all(|s| s.is_finite() && *s >= -1.1 && *s <= 1.1));
        assert!(out.iter().cloned().fold(f32::MAX, f32::min) < -0.3);
        assert!(out.iter().cloned().fold(f32::MIN, f32::max) > 0.3);
    }

    #[test]
    fn pink_brown_nodes_render_bounded() {
        for k in [Kind::PinkNoise, Kind::BrownNoise] {
            let mut e = E::new(48_000.0);
            e.create(NodeId(0), k);
            e.render_block();
            let out = e.node_output(NodeId(0), 0);
            assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            assert!(out.iter().any(|&s| s != 0.0));
        }
    }

    #[test]
    fn sync_saw_node_renders_bounded() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::SyncSaw);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(220.0); // master
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(660.0); // slave
        e.render_block();
        let out = e.node_output(NodeId(0), 0);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.2));
        assert!(out.iter().any(|&s| s != 0.0));
    }

    #[test]
    fn chain_saw_times_const_scales() {
        // node0 = saw(4Hz); node1 = mul(node0, 0.5)
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Saw);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(4.0);
        e.create(NodeId(1), Kind::Mul);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(0.5);
        e.render_block();
        let saw = e.node_output(NodeId(0), 0)[1]; // -0.5
        let scaled = e.node_output(NodeId(1), 0)[1];
        assert!((scaled - saw * 0.5).abs() < 1e-6);
    }

    #[test]
    fn two_nodes_sum_into_master_bus() {
        // node0 = const 0.3 (via Add of const+const), node1 = const 0.4; both → bus0.
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.3);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.create(NodeId(1), Kind::Add);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Const(0.4);
        *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.bus_write(
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));

        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.7).abs() < 1e-6);
        assert!((out[0].r - 0.7).abs() < 1e-6);
    }

    #[test]
    fn render_clamps_to_unit_range() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(5.0);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 1.0).abs() < 1e-6); // clamped
    }

    #[test]
    fn split2_feeds_two_consumers_from_two_ports_single_compute() {
        // src const 0.6 → split2 (ports 0,1); consumerA reads port0, consumerB port1.
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add); // produce a constant 0.6 source
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.6);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.create(NodeId(1), Kind::Split2);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        // consumerA = mul(port0, 2)
        e.create(NodeId(2), Kind::Mul);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(2.0);
        // consumerB = mul(port1, 3)
        e.create(NodeId(3), Kind::Mul);
        *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 1,
        };
        *e.node_input_mut(NodeId(3), 1).unwrap() = Input::Const(3.0);

        e.render_block();
        assert!((e.node_output(NodeId(2), 0)[0] - 1.2).abs() < 1e-6); // 0.6*2
        assert!((e.node_output(NodeId(3), 0)[0] - 1.8).abs() < 1e-6); // 0.6*3
    }

    #[test]
    fn dangling_and_oob_refs_render_silence_not_panic() {
        // node0 = a real 1-output saw (out_base 0, width 1).
        // node1 sums two bad references into bus0:
        //   - Input::Node { node: NodeId(50), port: 0 } — node never created (dangling).
        //   - Input::Node { node: NodeId(0), port: 7 }  — real node, out-of-range port.
        // Both must resolve to silence (0.0), never panic, never read an
        // unrelated slot.
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Saw);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(4.0);

        e.bus_write(
            Input::Node {
                node: NodeId(50),
                port: 0,
            },
            BusId(0),
        );
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 7,
            },
            BusId(0),
        );
        e.set_root(BusId(0));

        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil); // must not panic

        for f in out.iter() {
            assert!(f.l.is_finite() && f.l.abs() <= 1.0);
            assert!(f.r.is_finite() && f.r.abs() <= 1.0);
        }
        // Both bus-write sources are bad refs, so the bus sum is exactly 0.
        assert_eq!(out[0].l, 0.0);
        assert_eq!(out[0].r, 0.0);

        // Also exercise the render_block (Input::Node input-resolve) path
        // directly: a consumer node reading a dangling node ref must get a
        // zero row, not a panic or slot-0 crosstalk.
        let mut e2 = E::new(16.0);
        e2.create(NodeId(0), Kind::Saw);
        *e2.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(4.0);
        e2.create(NodeId(1), Kind::Add);
        *e2.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(50),
            port: 0,
        };
        *e2.node_input_mut(NodeId(1), 1).unwrap() = Input::Node {
            node: NodeId(0),
            port: 7,
        };
        e2.render_block(); // must not panic
        let consumer_out = e2.node_output(NodeId(1), 0);
        assert!(consumer_out.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn setparam_feedback_renders_bounded() {
        // Render the same sine oscillator config TWICE: once with feedback=0
        // (no SetParam call), once with feedback=0.8 (via Cmd::SetParam).
        // Assert the two outputs DIFFER, proving feedback modulation actually
        // changed the signal. Keep existing finite/bounded checks on feedback=0.8.

        // ── Render with feedback=0 (no SetParam) ──
        let mut e0 = E::new(48_000.0);
        e0.create(NodeId(0), Kind::Sine);
        *e0.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(2_000.0);
        // No SetParam call → default feedback=0
        e0.render_block();
        let out_no_feedback = e0.node_output(NodeId(0), 0).to_vec();

        // ── Render with feedback=0.8 (via Cmd::SetParam) ──
        let mut e1 = E::new(48_000.0);
        e1.create(NodeId(0), Kind::Sine);
        *e1.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(2_000.0);
        e1.apply(Cmd::SetParam {
            node: NodeId(0),
            param: 0,
            value: 0.8,
        });
        e1.render_block();
        let out_with_feedback = e1.node_output(NodeId(0), 0);

        // ── Assert feedback=0.8 output is finite and bounded ──
        assert!(
            out_with_feedback
                .iter()
                .all(|s| s.is_finite() && s.abs() <= 4.0)
        );
        assert!(out_with_feedback.iter().any(|&s| s != 0.0)); // feedback sine still oscillates

        // ── Assert the two outputs DIFFER (feedback changed the waveform) ──
        // A sine with feedback=0.8 must visibly differ from feedback=0.
        // We check that at least one sample differs beyond a small epsilon.
        let epsilon = 1e-5;
        assert!(
            out_no_feedback
                .iter()
                .zip(out_with_feedback.iter())
                .any(|(a, b)| (a - b).abs() > epsilon),
            "feedback=0 and feedback=0.8 outputs must differ"
        );
    }

    #[test]
    fn engine_pool_alloc_fill_read_free() {
        let mut e = E::new(48_000.0);
        let h = e.pool_alloc(16).expect("alloc");
        e.pool_slice_mut(h).fill(0.25);
        assert!(e.pool_slice(h).iter().all(|&x| x == 0.25));
        e.pool_free(h);
        // After free, a full-capacity alloc succeeds (region reclaimed).
        assert!(e.pool_alloc(16).is_some());
    }

    #[test]
    fn wavetable_node_renders_bounded_nonsilent() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Wavetable);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(220.0);
        e.apply(Cmd::BindTable {
            node: NodeId(0),
            src: TableSrc::Static(deluge_dsp_kernels::wavetable::TableId(0)),
        });
        e.render_block();
        let out = e.node_output(NodeId(0), 0);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.2));
        assert!(out.iter().any(|&s| s != 0.0));
    }

    #[test]
    fn pooled_wavetable_renders_and_frees() {
        let mut e = E::new(48_000.0);
        // Build a saw pyramid directly into the pool (mimics upload_table),
        // via the same flat-compact builder the runtime upload path uses.
        let n = mipgen::N;
        let compact_len = deluge_dsp_kernels::wavetable::COMPACT_LEN;
        let h = e.pool_alloc(compact_len).expect("pool room");
        let mut base = [0.0f32; mipgen::N];
        for (i, s) in base.iter_mut().enumerate() {
            *s = 2.0 * (i as f32 / n as f32) - 1.0;
        }
        mipgen::build_pyramid_flat_compact(&base, e.pool_slice_mut(h));
        e.create(NodeId(0), Kind::Wavetable);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(220.0);
        e.apply(Cmd::BindTable {
            node: NodeId(0),
            src: TableSrc::Pooled(h),
        });
        e.render_block();
        let out = e.node_output(NodeId(0), 0);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.2));
        assert!(out.iter().any(|&s| s != 0.0));
        // Free the node → pool region reclaimed.
        e.apply(Cmd::Free { node: NodeId(0) });
        // First-fit: a genuine free lets the next same-size alloc reclaim the exact
        // region → same handle. This fails if Cmd::Free didn't actually pool.free(h).
        assert_eq!(e.pool_alloc(compact_len), Some(h));
    }

    #[test]
    fn pool_exhaustion_returns_none_not_panic() {
        // `E`'s pool is PCAP=45056, PCHUNK=2048 → 22 chunks total. One
        // wavetable pyramid is the flat-compact `COMPACT_LEN`=6208 f32,
        // which rounds up to 4 chunks (2048*3=6144 < 6208 <= 2048*4=8192), so
        // exactly 5 pyramids fit (20 chunks) and a 6th (needing 4 more, only
        // 2 free) must degrade to `None`, never panic — the caller (Wren
        // `Wavetable.from`) is expected to leave the table unbound in that case.
        let mut e = E::new(48_000.0);
        let want = deluge_dsp_kernels::wavetable::COMPACT_LEN;
        let mut handles = [None; 5];
        for (i, slot) in handles.iter_mut().enumerate() {
            *slot = Some(
                e.pool_alloc(want)
                    .unwrap_or_else(|| panic!("pyramid {i} should fit")),
            );
        }
        assert!(
            e.pool_alloc(want).is_none(),
            "6th pyramid must not fit a 5-pyramid pool"
        );
        // Pool is not corrupted by the failed alloc: existing handles still work.
        let h1 = handles[0].unwrap();
        let h2 = handles[1].unwrap();
        e.pool_slice_mut(h1).fill(0.5);
        e.pool_slice_mut(h2).fill(0.75);
        assert!(e.pool_slice(h1).iter().all(|&x| x == 0.5));
        assert!(e.pool_slice(h2).iter().all(|&x| x == 0.75));
    }

    #[test]
    fn pooled_wavetable_wrong_sized_region_renders_silence_not_panic() {
        // A `TableSrc::Pooled` handle whose region isn't exactly `COMPACT_LEN`
        // long (e.g. the upload path allocated the wrong size, or a stale
        // handle from a different table) must never be sliced by the
        // compact-layout helpers — `process_resolved`'s exact-size guard
        // (`region.len() == COMPACT_LEN`) should just leave the output
        // untouched (silence in a freshly-zeroed arena slot). This is the
        // reachable degrade path for a "bad pool region": a
        // legitimately-obtained `PoolHandle` (via the public `pool_alloc`)
        // whose length happens to be wrong, since `PoolHandle`'s fields are
        // private to `pool.rs` and can't be hand-forged from `engine::tests`.
        let mut e = E::new(48_000.0);
        let bad = e.pool_alloc(64).expect("small alloc fits"); // 64 != COMPACT_LEN (6208)
        e.create(NodeId(0), Kind::Wavetable);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(220.0);
        e.apply(Cmd::BindTable {
            node: NodeId(0),
            src: TableSrc::Pooled(bad),
        });
        e.render_block(); // must not panic
        let out = e.node_output(NodeId(0), 0);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "wrong-sized pool region must render silence: {out:?}"
        );
    }

    #[test]
    fn pooled_morph_wavetable_renders() {
        let mut e = E::new(48_000.0);
        let cl = deluge_dsp_kernels::wavetable::COMPACT_LEN;
        let h = e.pool_alloc(2 * cl).expect("pool");
        // build 2 frames (saw, square) into the region
        let mut saw = [0.0f32; mipgen::N];
        let mut sq = [0.0f32; mipgen::N];
        for i in 0..mipgen::N {
            saw[i] = 2.0 * (i as f32 / mipgen::N as f32) - 1.0;
            sq[i] = if i < mipgen::N / 2 { 1.0 } else { -1.0 };
        }
        {
            let r = e.pool_slice_mut(h);
            mipgen::build_pyramid_flat_compact(&saw, &mut r[..cl]);
            mipgen::build_pyramid_flat_compact(&sq, &mut r[cl..]);
        }
        e.create(NodeId(0), Kind::Wavetable);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(220.0); // freq
        *e.node_input_mut(NodeId(0), 2).unwrap() = Input::Const(0.5); // position (port 2)
        e.apply(Cmd::BindTable {
            node: NodeId(0),
            src: TableSrc::Pooled(h),
        });
        e.render_block();
        let out = e.node_output(NodeId(0), 0);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.2));
        assert!(out.iter().any(|&s| s != 0.0));
    }

    #[test]
    fn per_side_write_gains_route_l_and_r_separately() {
        // node0 = const 0.5 (Add of const+0). Two gained writes: (1,0)→L only,
        // (0,1)→R only. Plus a plain center write from a second source to
        // prove (1,1) is unchanged.
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.apply(Cmd::BusWriteGains {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: BusId(0),
            gl: 1.0,
            gr: 0.0,
        });
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.5).abs() < 1e-6, "L got the (1,0) write");
        assert!(out[0].r.abs() < 1e-6, "R silent for a (1,0) write");

        // A (0,1) write lands only on R.
        let mut e2 = E::new(16.0);
        e2.create(NodeId(0), Kind::Add);
        *e2.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e2.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e2.apply(Cmd::BusWriteGains {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: BusId(0),
            gl: 0.0,
            gr: 1.0,
        });
        e2.set_root(BusId(0));
        let mut out2 = [StereoFrame::default(); 16];
        let sil2 = [StereoFrame::default(); 16];
        e2.render(&mut out2, &sil2);
        assert!(out2[0].l.abs() < 1e-6 && (out2[0].r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn input_node_routes_line_in_to_output() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Input);
        // route port0→L and port1→R of the input node into master, like Out.patch.
        e.bus_write_gains(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
            1.0,
            0.0,
        );
        e.bus_write_gains(
            Input::Node {
                node: NodeId(0),
                port: 1,
            },
            BusId(0),
            0.0,
            1.0,
        );
        e.set_root(BusId(0));

        let mut input = [StereoFrame::default(); 16];
        for i in 0..16 {
            input[i] = StereoFrame { l: 0.25, r: -0.5 };
        }
        let mut out = [StereoFrame::default(); 16];
        e.render(&mut out, &input);
        assert!((out[0].l - 0.25).abs() < 1e-6);
        assert!((out[0].r - (-0.5)).abs() < 1e-6);
        assert!((out[15].l - 0.25).abs() < 1e-6);
    }

    #[test]
    fn input_shorter_than_block_is_silent_tail_no_panic() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Input);
        e.bus_write_gains(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
            1.0,
            0.0,
        );
        e.set_root(BusId(0));
        let input = [StereoFrame { l: 1.0, r: 1.0 }; 4]; // shorter than BLOCK=16
        let mut out = [StereoFrame::default(); 16];
        e.render(&mut out, &input); // must not panic
        assert!((out[0].l - 1.0).abs() < 1e-6);
        assert_eq!(out[15].l, 0.0); // beyond input → silence
        // empty input also fine
        e.render(&mut out, &[]);
        assert_eq!(out[0].l, 0.0);
    }

    #[test]
    fn delay_node_renders_delayed_impulse_via_pool() {
        let mut e = E::new(48_000.0);
        let ring = e.pool_alloc(4096).expect("pool room");
        e.pool_slice_mut(ring).fill(0.0);
        e.create(NodeId(0), Kind::Delay);
        // impulse input via a const won't give a single spike; drive port 0
        // with a one-shot is awkward at graph level, so assert boundedness +
        // that a bound-buffer Delay runs (differs from dry) instead.
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5); // steady input
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.005); // 5 ms
        *e.node_input_mut(NodeId(0), 2).unwrap() = Input::Const(0.5); // feedback
        e.apply(Cmd::SetParam {
            node: NodeId(0),
            param: 0,
            value: 0.5,
        }); // mix
        e.apply(Cmd::BindTable {
            node: NodeId(0),
            src: TableSrc::Pooled(ring),
        });
        e.render_block();
        let out = e.node_output(NodeId(0), 0);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 8.0));
        assert!(out.iter().any(|&s| s != 0.0));
        // Free → pool region reclaimed (same handle reallocates).
        e.apply(Cmd::Free { node: NodeId(0) });
        assert_eq!(e.pool_alloc(4096), Some(ring));
    }

    #[test]
    fn poly_chain_sums_eight_voices_in_phase() {
        // PolyCtrl(all voices = same freq) → PolyOsc → VoiceSum.
        // All 8 lanes are identical (phase starts 0), so the sum peaks near 8×.
        type PE = Engine<64, 8, 32, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        e.create(NodeId(0), Kind::PolyCtrl);
        for v in 0..VOICES {
            e.apply(Cmd::SetParam {
                node: NodeId(0),
                param: v as u8,
                value: 440.0,
            });
        }
        e.create(NodeId(1), Kind::PolyOsc);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.create(NodeId(2), Kind::VoiceSum);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        e.render_block();
        let out = e.node_output(NodeId(2), 0);
        let peak = out.iter().cloned().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(out.iter().all(|s| s.is_finite()), "finite");
        assert!(
            peak > 7.0 && peak <= 8.001,
            "8 in-phase voices sum near 8×: peak {peak}"
        );
    }

    #[test]
    fn poly_distinct_voices_partially_cancel() {
        // Distinct per-voice frequencies → lanes drift out of phase → the sum's
        // peak stays well below 8× (proving voices are independent, not cloned).
        type PE = Engine<64, 8, 32, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        e.create(NodeId(0), Kind::PolyCtrl);
        for v in 0..VOICES {
            e.apply(Cmd::SetParam {
                node: NodeId(0),
                param: v as u8,
                value: (v as f32 + 1.0) * 300.0,
            });
        }
        e.create(NodeId(1), Kind::PolyOsc);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.create(NodeId(2), Kind::VoiceSum);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        e.render_block();
        let out = e.node_output(NodeId(2), 0);
        let peak = out.iter().cloned().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 8.001));
        assert!(peak < 7.0, "distinct voices don't all align: peak {peak}");
    }

    #[test]
    fn poly_dangling_input_is_silent() {
        // A VoiceSum whose poly input references a non-existent node → silence.
        type PE = Engine<64, 8, 32, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        e.create(NodeId(2), Kind::VoiceSum);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(5),
            port: 0,
        };
        e.render_block();
        let out = e.node_output(NodeId(2), 0);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "dangling poly edge → silence, no panic"
        );
    }

    #[test]
    fn poly_chain_matches_independent_reference_sum() {
        // Distinct per-voice frequencies: the VoiceSum output must EXACTLY equal
        // an independently-computed sum of 8 per-voice reference sines. This pins
        // the full arena↔poly_scratch interleave round-trip end-to-end — a
        // transpose would deliver the wrong frequency to each lane and fail here
        // (unlike the bulk-property tests above, which survive a permutation).
        // Runs in both feature configs, so it also null-tests the f32x8 PolyOsc.
        type PE = Engine<64, 8, 32, 4, 45056, 2048>;
        let sr = 48_000.0f32;
        let dt = 1.0 / sr;
        let freqs: [f32; VOICES] = core::array::from_fn(|v| (v as f32 + 1.0) * 137.0);
        let mut e = PE::new(sr);
        e.create(NodeId(0), Kind::PolyCtrl);
        for v in 0..VOICES {
            e.apply(Cmd::SetParam {
                node: NodeId(0),
                param: v as u8,
                value: freqs[v],
            });
        }
        e.create(NodeId(1), Kind::PolyOsc);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.create(NodeId(2), Kind::VoiceSum);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        e.render_block();
        let out = e.node_output(NodeId(2), 0);
        // Reference: per-voice phase accumulator (phase stays positive, so
        // `fract()` matches the kernel's floorf-based wrap). Advance-then-output,
        // matching the kernel's order.
        let mut ph = [0.0f32; VOICES];
        for (i, &got) in out.iter().enumerate() {
            let mut want = 0.0f32;
            for v in 0..VOICES {
                ph[v] += freqs[v] * dt;
                ph[v] -= ph[v].floor();
                want += deluge_dsp_kernels::fast_sin(ph[v]);
            }
            assert!(
                (got - want).abs() < 1e-3,
                "sample {i}: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn poly_mul_node_multiplies_two_poly_sources() {
        // Two PolyCtrl sources → PolyMul → VoiceSum. Sum == Σ_v (a_v * b_v).
        type PE = Engine<64, 8, 40, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        e.create(NodeId(0), Kind::PolyCtrl);
        e.create(NodeId(1), Kind::PolyCtrl);
        for v in 0..VOICES {
            e.apply(Cmd::SetParam {
                node: NodeId(0),
                param: v as u8,
                value: (v + 1) as f32,
            }); // 1..=8
            e.apply(Cmd::SetParam {
                node: NodeId(1),
                param: v as u8,
                value: 2.0,
            });
        }
        e.create(NodeId(2), Kind::PolyMul);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        e.create(NodeId(3), Kind::VoiceSum);
        *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node {
            node: NodeId(2),
            port: 0,
        };
        e.render_block();
        let out = e.node_output(NodeId(3), 0);
        let want: f32 = (1..=VOICES).map(|x| x as f32 * 2.0).sum(); // Σ 2·(1..8) = 72
        assert!(
            out.iter().all(|&s| (s - want).abs() < 1e-3),
            "sum of a·b == {want}"
        );
    }

    #[test]
    fn mono_source_broadcasts_to_all_poly_lanes() {
        // PolyOsc(pitch) as poly source A; a mono Ctrl=0.5 as source B;
        // PolyMul(A, B). Because B is width-1, the broadcast must splat 0.5 to
        // every lane, so PolyMul lane v == PolyOsc lane v * 0.5 for every v —
        // including v > 0, which the old VOICES-row-copy left as zero/garbage
        // (Ctrl only ever writes its own single output row).
        type PE = Engine<64, 8, 40, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        e.create(NodeId(0), Kind::PolyCtrl); // pitch per voice
        for v in 0..VOICES {
            e.apply(Cmd::SetParam {
                node: NodeId(0),
                param: v as u8,
                value: (v as f32 + 1.0) * 110.0,
            });
        }
        e.create(NodeId(1), Kind::PolyOsc); // poly source A
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.create(NodeId(2), Kind::Ctrl); // mono source B
        e.apply(Cmd::SetParam {
            node: NodeId(2),
            param: 0,
            value: 0.5,
        });
        e.create(NodeId(3), Kind::PolyMul);
        *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        *e.node_input_mut(NodeId(3), 1).unwrap() = Input::Node {
            node: NodeId(2),
            port: 0,
        };
        e.render_block();

        for v in 0..VOICES {
            let osc = e.node_output(NodeId(1), v as u8);
            let out = e.node_output(NodeId(3), v as u8);
            let osc: [f32; 64] = osc.try_into().unwrap();
            let out: [f32; 64] = out.try_into().unwrap();
            for i in 0..64 {
                let want = osc[i] * 0.5;
                assert!(
                    (out[i] - want).abs() < 1e-5,
                    "voice {v} sample {i}: got {}, want {} (broadcast of mono B)",
                    out[i],
                    want
                );
            }
            if v > 0 {
                assert!(
                    out.iter().any(|&s| s.abs() > 1e-6),
                    "voice {v} must carry the broadcast mono source, not zero/garbage"
                );
            }
        }
    }

    #[test]
    fn const_source_broadcasts_to_all_poly_lanes() {
        // I-1 regression guard: a poly input port driven directly by a
        // numeric literal (Input::Const(v), v != 0) — e.g. `Osc.syncSaw(60, p)`
        // or `o.width = 0.3` in Wren — must broadcast v to every VOICES lane,
        // exactly like a mono (width-1) Node source does. Before the fix,
        // Input::Const fell into the poly-resolution catch-all `_ => zero`
        // arm and silently produced an all-zero tile on every lane.
        type PE = Engine<64, 8, 40, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        e.create(NodeId(0), Kind::PolyCtrl); // pitch per voice
        for v in 0..VOICES {
            e.apply(Cmd::SetParam {
                node: NodeId(0),
                param: v as u8,
                value: (v as f32 + 1.0) * 110.0,
            });
        }
        e.create(NodeId(1), Kind::PolyOsc); // poly source A
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.create(NodeId(2), Kind::PolyMul);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(0.5);
        e.render_block();

        for v in 0..VOICES {
            let osc = e.node_output(NodeId(1), v as u8);
            let out = e.node_output(NodeId(2), v as u8);
            let osc: [f32; 64] = osc.try_into().unwrap();
            let out: [f32; 64] = out.try_into().unwrap();
            for i in 0..64 {
                let want = osc[i] * 0.5;
                assert!(
                    (out[i] - want).abs() < 1e-5,
                    "voice {v} sample {i}: got {}, want {} (broadcast of Input::Const(0.5))",
                    out[i],
                    want
                );
            }
            assert!(
                out.iter().any(|&s| s.abs() > 1e-6),
                "voice {v} must carry the broadcast Const source, not zero (I-1 regression)"
            );
        }
    }

    #[test]
    fn polyosc_width_port_default_and_mono_broadcast_pwm() {
        // PolyOsc's new width port (port 1): unconnected ⇒ Const(0.0) ⇒ the
        // engine's mono→poly broadcast (Task 1) delivers an all-zero tile ⇒
        // 0.5 duty (backward-compat default). Wiring a mono Ctrl source ⇒ that
        // value's duty, broadcast to every voice. Low freq (100 Hz, exactly 480
        // samples/cycle @ 48 kHz) over 10 cycles so the +1 fraction approximates
        // the duty cycle (mirrors `osc::tests::pwm_duty_cycle_tracks_width`).
        type PE = Engine<64, 8, 40, 4, 45056, 2048>;
        const BLOCK: usize = 64;
        const N_BLOCKS: usize = 75; // 4800 samples = 10 cycles @ 100 Hz
        let sr = 48_000.0f32;
        let freq = 100.0f32;

        let render = |width_const: Option<f32>| -> [f32; BLOCK * N_BLOCKS] {
            let mut e = PE::new(sr);
            e.create(NodeId(0), Kind::PolyCtrl); // pitch per voice
            for v in 0..VOICES {
                e.apply(Cmd::SetParam {
                    node: NodeId(0),
                    param: v as u8,
                    value: freq,
                });
            }
            e.create(NodeId(1), Kind::PolyOsc);
            e.apply(Cmd::SetParam {
                node: NodeId(1),
                param: 0,
                value: 2.0,
            }); // Square
            *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
                node: NodeId(0),
                port: 0,
            };
            if let Some(w) = width_const {
                e.create(NodeId(2), Kind::Ctrl); // mono width source
                e.apply(Cmd::SetParam {
                    node: NodeId(2),
                    param: 0,
                    value: w,
                });
                *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Node {
                    node: NodeId(2),
                    port: 0,
                };
            }
            // else: width port (1) stays unconnected, i.e. Input::Const(0.0).
            let mut out = [0.0f32; BLOCK * N_BLOCKS];
            for b in 0..N_BLOCKS {
                e.render_block();
                out[b * BLOCK..(b + 1) * BLOCK].copy_from_slice(e.node_output(NodeId(1), 0));
            }
            out
        };

        let default_out = render(None);
        let shifted_out = render(Some(0.2));

        for out in [&default_out, &shifted_out] {
            assert!(
                out.iter().all(|s| s.is_finite() && s.abs() <= 1.2),
                "finite/bounded"
            );
            assert!(out.iter().any(|&s| s != 0.0), "non-silent");
        }

        let duty = |out: &[f32]| out.iter().filter(|&&s| s > 0.0).count() as f32 / out.len() as f32;
        let d_default = duty(&default_out);
        let d_shifted = duty(&shifted_out);
        assert!(
            (d_default - 0.5).abs() < 0.05,
            "unconnected width port ⇒ ~0.5 duty, got {d_default}"
        );
        assert!(
            (d_shifted - 0.2).abs() < 0.05,
            "width=0.2 broadcast ⇒ ~0.2 duty, got {d_shifted}"
        );
    }

    // ── G2: control rate ──────────────────────────────────────────────────

    type KE = Engine<64, 8, 16, 4, 45056, 2048>;

    /// An LFO at `hz`, optionally at control rate.
    fn lfo(e: &mut KE, id: u16, hz: f32, kr: bool) {
        e.apply(Cmd::NewNode {
            node: NodeId(id),
            kind: Kind::Lfo,
            args: [Input::Const(hz), Input::Const(0.0), Input::Const(0.0)],
        });
        if kr {
            assert!(e.set_rate(NodeId(id), crate::node::Rate::Control));
        }
    }

    #[test]
    fn control_rate_output_is_constant_across_the_block() {
        let mut e = KE::new(48_000.0);
        lfo(&mut e, 0, 500.0, true); // fast enough to move within one block
        lfo(&mut e, 1, 500.0, false);
        e.render_block();

        let kr_row = e.node_output(NodeId(0), 0);
        assert!(
            kr_row.iter().all(|s| *s == kr_row[0]),
            "control rate broadcasts one value across the row"
        );
        let ar_row = e.node_output(NodeId(1), 0);
        assert!(
            ar_row.iter().any(|s| *s != ar_row[0]),
            "audio rate still varies within the block"
        );
    }

    #[test]
    fn control_rate_advances_at_the_same_wall_clock_rate() {
        // The `BLOCK * dt` scaling is the whole correctness question here: with
        // plain `dt` a control-rate node would advance BLOCK times too slowly.
        // A slow LFO keeps the inherent one-block quantisation well below the
        // tolerance, so this measures the scaling rather than the lag.
        let mut e = KE::new(48_000.0);
        lfo(&mut e, 0, 2.0, true);
        lfo(&mut e, 1, 2.0, false);
        for _ in 0..200 {
            e.render_block();
        }
        let kr = e.node_output(NodeId(0), 0)[0];
        let ar = e.node_output(NodeId(1), 0)[0];
        assert!(
            (kr - ar).abs() < 0.05,
            "kr {kr} should track ar {ar} (one block of lag, not BLOCK× slower)"
        );
        // ...and it has actually gone somewhere: a BLOCK-times-too-slow LFO
        // would still be within a rounding error of its starting value.
        assert!(kr.abs() > 0.1, "kr {kr} barely moved — dt scaling missing?");
    }

    #[test]
    fn control_rate_without_dt_scaling_would_be_visibly_slower() {
        // Guards the scaling from being "simplified" away: over enough blocks a
        // BLOCK-times-too-slow LFO cannot have completed a full cycle, so its
        // output would still be pinned near the start of the ramp.
        let mut e = KE::new(48_000.0);
        lfo(&mut e, 0, 50.0, true); // 50 Hz → a cycle every 960 samples = 15 blocks
        let mut seen_low = false;
        let mut seen_high = false;
        for _ in 0..60 {
            e.render_block();
            let v = e.node_output(NodeId(0), 0)[0];
            seen_low |= v < -0.5;
            seen_high |= v > 0.5;
        }
        assert!(
            seen_low && seen_high,
            "a correctly-clocked kr LFO sweeps its full range"
        );
    }

    #[test]
    fn a_control_rate_source_reaches_its_consumer() {
        // The consumer is handed `In::K`, not a row; the value must still be
        // the one the source produced.
        let mut e = KE::new(48_000.0);
        // `Ctrl` takes its value from `SetParam`, not from an input arg.
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Ctrl,
            args: [Input::Const(0.0); 3],
        });
        e.apply(Cmd::SetParam {
            node: NodeId(0),
            param: 0,
            value: 0.25,
        });
        assert!(e.set_rate(NodeId(0), crate::node::Rate::Control));
        e.apply(Cmd::NewNode {
            node: NodeId(1),
            kind: Kind::Add,
            args: [
                Input::Node {
                    node: NodeId(0),
                    port: 0,
                },
                Input::Const(0.5),
                Input::Const(0.0),
            ],
        });
        e.render_block();
        let out = e.node_output(NodeId(1), 0);
        assert!(
            out.iter().all(|s| (*s - 0.75).abs() < 1e-6),
            "consumer saw {:?}",
            &out[..4]
        );
    }

    #[test]
    fn set_rate_refuses_multi_port_kinds() {
        let mut e = KE::new(48_000.0);
        use crate::node::Rate;
        e.create(NodeId(0), Kind::PolyOsc); // VOICES wide
        e.create(NodeId(1), Kind::Pan); // 2 wide
        e.create(NodeId(2), Kind::Lfo); // 1 wide
        assert!(!e.set_rate(NodeId(0), Rate::Control), "poly refused");
        assert!(!e.set_rate(NodeId(1), Rate::Control), "stereo refused");
        assert!(e.set_rate(NodeId(2), Rate::Control), "mono accepted");
        assert_eq!(e.rate_of(NodeId(0)), Some(Rate::Audio));
        assert_eq!(e.rate_of(NodeId(1)), Some(Rate::Audio));
        assert_eq!(e.rate_of(NodeId(2)), Some(Rate::Control));
    }

    #[test]
    fn set_rate_on_a_dead_node_is_refused() {
        let mut e = KE::new(48_000.0);
        use crate::node::Rate;
        assert!(!e.set_rate(NodeId(5), Rate::Control));
        assert!(!e.set_rate(NodeId(5), Rate::Audio));
        assert_eq!(e.rate_of(NodeId(5)), None);
    }

    #[test]
    fn rate_resets_to_audio_on_recreate() {
        let mut e = KE::new(48_000.0);
        use crate::node::Rate;
        lfo(&mut e, 0, 20.0, true);
        assert_eq!(e.rate_of(NodeId(0)), Some(Rate::Control));
        e.apply(Cmd::Free { node: NodeId(0) });
        lfo(&mut e, 0, 20.0, false); // same id, fresh node
        assert_eq!(
            e.rate_of(NodeId(0)),
            Some(Rate::Audio),
            "a recreated id must not inherit the old node's rate"
        );
    }

    #[test]
    fn switching_back_to_audio_rate_restores_per_sample_detail() {
        let mut e = KE::new(48_000.0);
        use crate::node::Rate;
        lfo(&mut e, 0, 500.0, true);
        e.render_block();
        let row = e.node_output(NodeId(0), 0);
        assert!(row.iter().all(|s| *s == row[0]));

        e.apply(Cmd::SetRate {
            node: NodeId(0),
            rate: Rate::Audio,
        });
        e.render_block();
        let row = e.node_output(NodeId(0), 0);
        assert!(
            row.iter().any(|s| *s != row[0]),
            "back at audio rate the row varies again"
        );
    }

    #[test]
    fn control_rate_node_still_routes_to_a_bus() {
        // Bus writes read the output row directly rather than going through
        // `In::K`, so the broadcast fill is what keeps them correct.
        let mut e = KE::new(48_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Ctrl,
            args: [Input::Const(0.0); 3],
        });
        e.apply(Cmd::SetParam {
            node: NodeId(0),
            param: 0,
            value: 0.5,
        });
        assert!(e.set_rate(NodeId(0), crate::node::Rate::Control));
        e.apply(Cmd::BusWrite {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: BusId(0),
        });
        e.apply(Cmd::SetRoot { bus: BusId(0) });
        let mut out = [StereoFrame::default(); 64];
        let sil = [StereoFrame::default(); 64];
        e.render(&mut out, &sil);
        assert!(
            out.iter().all(|f| (f.l - 0.5).abs() < 1e-6),
            "every frame carries the broadcast value"
        );
    }

    // ── G4b: reordering ───────────────────────────────────────────────────

    #[test]
    fn a_move_that_contradicts_a_dependency_is_undone_by_the_next_sort() {
        // `MoveBefore` / `MoveAfter` are an override, not a pin. A move takes
        // effect immediately — including one that puts a consumer *before* its
        // source, which is the deliberate one-block-delay case — and it holds
        // for as long as the graph's shape is unchanged. The next structural
        // edit re-sorts, and the dependency wins.
        //
        // Node 0 is a `Saw`, so its value differs every block: that is what
        // makes "this block" and "the previous block" distinguishable without
        // touching the graph (and touching it would itself force a re-sort).
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Saw,
            args: [Input::Const(2_000.0), Input::Const(0.0), Input::Const(0.0)],
        });
        let follow = |e: &mut ME, id: u16, src: u16| {
            e.apply(Cmd::NewNode {
                node: NodeId(id),
                kind: Kind::Add,
                args: [
                    Input::Node {
                        node: NodeId(src),
                        port: 0,
                    },
                    Input::Const(0.0),
                    Input::Const(0.0),
                ],
            });
        };
        follow(&mut e, 1, 0);
        follow(&mut e, 2, 1);

        e.render_block();
        let one_a = e.node_output(NodeId(1), 0)[0];
        assert_eq!(
            e.node_output(NodeId(2), 0)[0],
            one_a,
            "sorted: node 2 sees node 1's current block"
        );

        // Override: put the consumer ahead of its source on purpose.
        assert!(e.move_before(NodeId(2), NodeId(1)));
        assert_eq!(e.eval_order(), &[0, 2, 1]);
        e.render_block();
        let one_b = e.node_output(NodeId(1), 0)[0];
        assert_ne!(one_b, one_a, "the saw moved on, so the two blocks differ");
        assert_eq!(
            e.node_output(NodeId(2), 0)[0],
            one_a,
            "reads node 1's PREVIOUS block: the override is honoured"
        );

        // A structural edit — here an unrelated new node — re-sorts, and the
        // dependency reasserts itself.
        assert!(e.create(NodeId(3), Kind::Saw));
        e.render_block();
        assert_eq!(e.eval_order(), &[0, 1, 2, 3]);
        assert_eq!(
            e.node_output(NodeId(2), 0)[0],
            e.node_output(NodeId(1), 0)[0],
            "current block again"
        );
    }

    #[test]
    fn reordering_preserves_bus_routing_and_output() {
        // Bus writes are keyed by source `Input`, not eval position, so moving
        // a routed node must not drop or duplicate its contribution.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        for (id, v) in [(0u16, 0.25f32), (1, 0.5)] {
            e.apply(Cmd::NewNode {
                node: NodeId(id),
                kind: Kind::Add,
                args: [Input::Const(v), Input::Const(0.0), Input::Const(0.0)],
            });
            e.apply(Cmd::BusWrite {
                src: Input::Node {
                    node: NodeId(id),
                    port: 0,
                },
                bus: BusId(0),
            });
        }
        e.apply(Cmd::SetRoot { bus: BusId(0) });
        let mut out = [StereoFrame::default(); 8];
        let sil = [StereoFrame::default(); 8];
        e.render(&mut out, &sil);
        let before = out[0].l;
        assert!((before - 0.75).abs() < 1e-6);

        e.apply(Cmd::MoveBefore {
            node: NodeId(1),
            target: NodeId(0),
        });
        e.render(&mut out, &sil);
        assert!(
            (out[0].l - before).abs() < 1e-6,
            "routing survives the move: {} vs {before}",
            out[0].l
        );
    }

    #[test]
    fn move_of_a_freed_node_is_a_noop() {
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        for id in 0..3u16 {
            e.create(NodeId(id), Kind::Saw);
        }
        e.apply(Cmd::Free { node: NodeId(1) });
        assert!(!e.move_after(NodeId(1), NodeId(0)));
        assert!(!e.move_after(NodeId(0), NodeId(1)));
        assert_eq!(e.eval_order(), &[0, 2]);
        e.render_block(); // must not panic on a stale order entry
    }

    // ── G1: engine → host events ──────────────────────────────────────────

    /// Render blocks until `f` reports done or `max` blocks elapse; returns the
    /// blocks consumed. Envelope releases take many blocks to decay.
    fn render_until(
        e: &mut Engine<64, 8, 40, 4, 45056, 2048>,
        max: usize,
        mut f: impl FnMut(&mut Engine<64, 8, 40, 4, 45056, 2048>) -> bool,
    ) -> usize {
        for n in 0..max {
            e.render_block();
            if f(e) {
                return n + 1;
            }
        }
        max
    }

    type EvE = Engine<64, 8, 40, 4, 45056, 2048>;

    fn poly_env(e: &mut EvE) {
        e.create(NodeId(0), Kind::PolyAr);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.0005); // fast attack
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.001); // fast release
    }

    #[test]
    fn fresh_envelope_emits_no_event() {
        // A newly created envelope reads as Idle. If completion were reported as
        // a level rather than a rising edge, this would announce a release that
        // never happened. `seed_prev_idle` is what prevents it.
        let mut e = EvE::new(48_000.0);
        poly_env(&mut e);
        for _ in 0..8 {
            e.render_block();
        }
        assert_eq!(e.pop_event(), None, "fresh envelope must stay silent");
        assert_eq!(e.events_dropped(), 0);
    }

    #[test]
    fn poly_release_completion_emits_voice_done_once() {
        let mut e = EvE::new(48_000.0);
        poly_env(&mut e);
        e.apply(Cmd::GateVoice {
            node: NodeId(0),
            voice: 3,
            on: true,
        });
        // Held: attack then sustain, never idle → no event.
        for _ in 0..16 {
            e.render_block();
        }
        assert_eq!(e.pop_event(), None, "a held voice has not completed");

        e.apply(Cmd::GateVoice {
            node: NodeId(0),
            voice: 3,
            on: false,
        });
        render_until(&mut e, 200, |e| !e.events.is_empty());

        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::VoiceDone {
                node: NodeId(0),
                voice: 3
            })
        );
        // Rising edge only: the lane stays idle but must not re-announce.
        for _ in 0..32 {
            e.render_block();
        }
        assert_eq!(
            e.pop_event(),
            None,
            "idle is a level, completion is an edge"
        );
    }

    #[test]
    fn only_the_released_lane_reports() {
        let mut e = EvE::new(48_000.0);
        poly_env(&mut e);
        for v in [1u8, 5] {
            e.apply(Cmd::GateVoice {
                node: NodeId(0),
                voice: v,
                on: true,
            });
        }
        for _ in 0..16 {
            e.render_block();
        }
        e.apply(Cmd::GateVoice {
            node: NodeId(0),
            voice: 5,
            on: false,
        });
        render_until(&mut e, 200, |e| !e.events.is_empty());

        let mut got = [0u8; 8];
        let mut n = 0;
        e.drain_events(|ev| {
            if let crate::event::Event::VoiceDone { voice, .. } = ev {
                got[n] = voice;
                n += 1;
            }
        });
        assert_eq!(&got[..n], &[5], "lane 1 is still held");
    }

    #[test]
    fn mono_envelope_reports_done_not_voice_done() {
        let mut e = EvE::new(48_000.0);
        e.create(NodeId(0), Kind::Env);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.0005);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.001);
        e.apply(Cmd::Gate {
            node: NodeId(0),
            on: true,
        });
        for _ in 0..16 {
            e.render_block();
        }
        e.apply(Cmd::Gate {
            node: NodeId(0),
            on: false,
        });
        render_until(&mut e, 200, |e| !e.events.is_empty());
        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::Done { node: NodeId(0) })
        );
    }

    #[test]
    fn non_envelope_kinds_never_emit() {
        let mut e = EvE::new(48_000.0);
        e.create(NodeId(0), Kind::Saw);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(220.0);
        e.create(NodeId(1), Kind::Lpf);
        *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(800.0);
        for _ in 0..32 {
            e.render_block();
        }
        assert_eq!(e.pop_event(), None);
    }

    #[test]
    fn freed_node_does_not_edge_on_a_reused_slot() {
        // Free a released envelope, then recreate the id as a fresh envelope.
        // The new node must not inherit the old slot's idle history.
        let mut e = EvE::new(48_000.0);
        poly_env(&mut e);
        e.apply(Cmd::GateVoice {
            node: NodeId(0),
            voice: 0,
            on: true,
        });
        for _ in 0..8 {
            e.render_block();
        }
        e.apply(Cmd::Free { node: NodeId(0) });
        e.drain_events(|_| {});
        poly_env(&mut e); // same id, fresh PolyAr
        for _ in 0..16 {
            e.render_block();
        }
        assert_eq!(e.pop_event(), None);
    }

    #[test]
    fn reset_clears_pending_events_and_history() {
        let mut e = EvE::new(48_000.0);
        poly_env(&mut e);
        e.apply(Cmd::GateVoice {
            node: NodeId(0),
            voice: 2,
            on: true,
        });
        for _ in 0..16 {
            e.render_block();
        }
        e.apply(Cmd::GateVoice {
            node: NodeId(0),
            voice: 2,
            on: false,
        });
        render_until(&mut e, 200, |e| !e.events.is_empty());
        assert!(!e.events.is_empty());
        e.apply(Cmd::Reset);
        assert_eq!(e.pop_event(), None, "Reset drops queued events");
    }

    #[test]
    fn gate_voice_reaches_only_addressed_lane() {
        // A PolyAr gated on voice 3 only → after some samples, VoiceSum > 0 comes
        // solely from lane 3 (all others idle at 0).
        type PE = Engine<64, 8, 40, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        e.create(NodeId(0), Kind::PolyAr);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.0005); // fast attack
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.05);
        e.create(NodeId(1), Kind::VoiceSum);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.apply(Cmd::GateVoice {
            node: NodeId(0),
            voice: 3,
            on: true,
        });
        e.render_block();
        let out = e.node_output(NodeId(1), 0);
        // Exactly one voice ramping ⇒ sum rises toward ~1 (not 0, not ~8).
        let peak = out.iter().cloned().fold(0.0f32, |m, s| m.max(s));
        assert!(peak > 0.1 && peak < 1.5, "one gated voice: peak {peak}");
    }

    #[test]
    fn full_voice_gated_per_voice() {
        // A complete voice: gate voices 0 & 1 on, 2..7 off. The mix is non-silent
        // and comes only from the gated voices; with no gates it is silent; the
        // filter keeps the output bounded. Runs in both feature configs.
        type PE = Engine<64, 8, 48, 4, 45056, 2048>;
        let build = |gates: &[usize]| -> [f32; 64] {
            let mut e = PE::new(48_000.0);
            // pitch source
            e.create(NodeId(0), Kind::PolyCtrl);
            for v in 0..VOICES {
                e.apply(Cmd::SetParam {
                    node: NodeId(0),
                    param: v as u8,
                    value: (v as f32 + 1.0) * 110.0,
                });
            }
            // osc → filter
            e.create(NodeId(1), Kind::PolyOsc);
            *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
                node: NodeId(0),
                port: 0,
            };
            e.create(NodeId(2), Kind::PolySvf);
            *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
                node: NodeId(1),
                port: 0,
            };
            *e.node_input_mut(NodeId(2), 1).unwrap() = Input::Const(1200.0); // cutoff
            *e.node_input_mut(NodeId(2), 2).unwrap() = Input::Const(0.2); // res
            // envelope + VCA
            e.create(NodeId(3), Kind::PolyAr);
            *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Const(0.0005); // attack
            *e.node_input_mut(NodeId(3), 1).unwrap() = Input::Const(0.05); // release
            e.create(NodeId(4), Kind::PolyMul);
            *e.node_input_mut(NodeId(4), 0).unwrap() = Input::Node {
                node: NodeId(2),
                port: 0,
            };
            *e.node_input_mut(NodeId(4), 1).unwrap() = Input::Node {
                node: NodeId(3),
                port: 0,
            };
            // sum → out
            e.create(NodeId(5), Kind::VoiceSum);
            *e.node_input_mut(NodeId(5), 0).unwrap() = Input::Node {
                node: NodeId(4),
                port: 0,
            };
            for &g in gates {
                e.apply(Cmd::GateVoice {
                    node: NodeId(3),
                    voice: g as u8,
                    on: true,
                });
            }
            e.render_block();
            let mono = e.node_output(NodeId(5), 0);
            let mut out = [0.0f32; 64];
            out.copy_from_slice(&mono[..64]);
            out
        };

        // No gates → silence.
        let silent = build(&[]);
        assert!(silent.iter().all(|&s| s.abs() < 1e-6), "no gates → silent");

        // Gate voices 0 & 1 → non-silent, bounded/finite.
        let voiced = build(&[0, 1]);
        assert!(
            voiced.iter().all(|&s| s.is_finite() && s.abs() <= 8.0),
            "bounded"
        );
        assert!(voiced.iter().any(|&s| s.abs() > 1e-4), "gated voices sound");
    }

    fn build_voice(e: &mut Engine<64, 8, 56, 4, 45056, 2048>) {
        // PolyCtrl(0) → PolyMtof(1) → PolyOsc(2) → PolySvf(3) →
        //     PolyMul(4, PolyAr(5)) → VoiceSum(6)
        e.create(NodeId(0), Kind::PolyCtrl);
        e.create(NodeId(1), Kind::PolyMtof);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.create(NodeId(2), Kind::PolyOsc);
        *e.node_input_mut(NodeId(2), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        e.create(NodeId(3), Kind::PolySvf);
        *e.node_input_mut(NodeId(3), 0).unwrap() = Input::Node {
            node: NodeId(2),
            port: 0,
        };
        *e.node_input_mut(NodeId(3), 1).unwrap() = Input::Const(2000.0);
        *e.node_input_mut(NodeId(3), 2).unwrap() = Input::Const(0.2);
        e.create(NodeId(5), Kind::PolyAr);
        *e.node_input_mut(NodeId(5), 0).unwrap() = Input::Const(0.001); // attack
        *e.node_input_mut(NodeId(5), 1).unwrap() = Input::Const(0.005); // release
        e.create(NodeId(4), Kind::PolyMul);
        *e.node_input_mut(NodeId(4), 0).unwrap() = Input::Node {
            node: NodeId(3),
            port: 0,
        };
        *e.node_input_mut(NodeId(4), 1).unwrap() = Input::Node {
            node: NodeId(5),
            port: 0,
        };
        e.create(NodeId(6), Kind::VoiceSum);
        *e.node_input_mut(NodeId(6), 0).unwrap() = Input::Node {
            node: NodeId(4),
            port: 0,
        };
    }

    #[test]
    fn note_on_sounds_then_note_off_silences() {
        type PE = Engine<64, 8, 56, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        build_voice(&mut e);
        let mut gates = [NodeId(0); crate::voice::MAX_GATES];
        gates[0] = NodeId(5);
        let mut alloc = crate::voice::VoiceAllocator::new(
            NodeId(0),
            gates,
            1,
            None,
            NodeId(99),
            [NodeId(0); crate::voice::MAX_TRIGGERS],
            0,
        );

        {
            let mut emit = |c: Cmd| e.apply(c);
            alloc.note_on(69, 100, &mut emit);
        }
        for _ in 0..8 {
            e.render_block();
        } // let the fast envelope attack
        let out = e.node_output(NodeId(6), 0);
        assert!(
            out.iter().all(|s| s.is_finite() && s.abs() <= 8.5),
            "bounded"
        );
        assert!(out.iter().any(|&s| s.abs() > 1e-3), "note-on sounds");

        {
            let mut emit = |c: Cmd| e.apply(c);
            alloc.note_off(69, &mut emit);
        }
        for _ in 0..300 {
            e.render_block();
        } // past the 5 ms release
        let out2 = e.node_output(NodeId(6), 0);
        assert!(out2.iter().all(|&s| s.abs() < 1e-4), "note-off silences");
    }

    #[test]
    fn ninth_note_steals_and_still_sounds() {
        type PE = Engine<64, 8, 56, 4, 45056, 2048>;
        let mut e = PE::new(48_000.0);
        build_voice(&mut e);
        let mut gates = [NodeId(0); crate::voice::MAX_GATES];
        gates[0] = NodeId(5);
        let mut alloc = crate::voice::VoiceAllocator::new(
            NodeId(0),
            gates,
            1,
            None,
            NodeId(99),
            [NodeId(0); crate::voice::MAX_TRIGGERS],
            0,
        );
        {
            let mut emit = |c: Cmd| e.apply(c);
            for k in 0..9u8 {
                alloc.note_on(60 + k, 100, &mut emit); // 9 notes → one steal
            }
        }
        for _ in 0..8 {
            e.render_block();
        }
        let out = e.node_output(NodeId(6), 0);
        assert!(
            out.iter().all(|s| s.is_finite() && s.abs() <= 8.5),
            "bounded after steal"
        );
        assert!(
            out.iter().any(|&s| s.abs() > 1e-3),
            "still sounds after steal"
        );
    }

    #[test]
    fn master_limiter_bounds_root_bus_when_enabled() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::SetMasterLimit {
            ceiling: 0.5,
            release: 0.05,
        });
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        for i in 0..16 {
            assert!(out[i].l.abs() <= 0.5 + 1e-6, "out[{}].l={}", i, out[i].l);
        }
    }

    #[test]
    fn master_limiter_disabled_is_raw_then_clamp() {
        // Without SetMasterLimit, a 0.98 root bus passes at 0.98 (below the clamp).
        // 0.98 is deliberately ABOVE the limiter's 0.95 default ceiling, so this
        // also catches an accidental `Some(default-limiter)` init: a default
        // limiter would attenuate 0.98 toward 0.95 and fail this assertion.
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.98);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.98).abs() < 1e-6); // unlimited, only the [-1,1] clamp would act
    }

    #[test]
    fn master_limiter_reset_clears() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::SetMasterLimit {
            ceiling: 0.5,
            release: 0.05,
        });
        e.apply(Cmd::Reset);
        // After Reset the graph is torn down; rebuild the same patch, no limiter.
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.8).abs() < 1e-6); // limiter cleared → unlimited
    }

    #[test]
    fn master_dcblock_removes_dc_when_enabled() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5); // DC
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::SetMasterDcBlock { cutoff_hz: 20.0 });
        // Render several blocks so the HP settles.
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        for _ in 0..512 {
            e.render(&mut out, &sil);
        }
        assert!(out[15].l.abs() < 1e-2, "DC not removed: {}", out[15].l);
    }

    #[test]
    fn master_dcblock_disabled_is_byte_identical() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.5).abs() < 1e-6); // no DC-block → 0.5 passes
    }

    #[test]
    fn master_dcblock_reset_clears() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::SetMasterDcBlock { cutoff_hz: 20.0 });
        e.apply(Cmd::Reset);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.5).abs() < 1e-6); // DC-block cleared → 0.5 passes
    }

    #[test]
    fn master_eq_changes_render_when_enabled() {
        let build = |eq: bool| -> f32 {
            let mut e = E::new(48_000.0);
            e.create(NodeId(0), Kind::Add);
            *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.2);
            *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
            e.bus_write(
                Input::Node {
                    node: NodeId(0),
                    port: 0,
                },
                BusId(0),
            );
            e.set_root(BusId(0));
            if eq {
                // Low-shelf (type 1) boost: a shelf below `freq` raises DC content,
                // so a constant source measurably changes (a peak EQ would not
                // touch DC). +12 dB → the 0.2 DC level climbs toward ~0.8.
                e.apply(Cmd::SetMasterEq {
                    freq: 1000.0,
                    gain_db: 12.0,
                    q: 0.707,
                    eq_type: 1,
                });
            }
            let mut out = [StereoFrame::default(); 16];
            let sil = [StereoFrame::default(); 16];
            for _ in 0..64 {
                e.render(&mut out, &sil);
            }
            out[15].l
        };
        let off = build(false);
        let on = build(true);
        assert!(
            on > off + 0.1,
            "low-shelf boost should raise the DC level (off={}, on={})",
            off,
            on
        );
    }

    #[test]
    fn master_eq_disabled_is_byte_identical() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.5).abs() < 1e-6); // no EQ → 0.5 passes
    }

    #[test]
    fn master_eq_reset_clears() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::SetMasterEq {
            freq: 2000.0,
            gain_db: 12.0,
            q: 2.0,
            eq_type: 0,
        });
        e.apply(Cmd::Reset);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.5);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.5).abs() < 1e-6); // EQ cleared → 0.5 passes
    }

    #[test]
    fn bus_write_reuses_freed_slot() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.25);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        ); // writes_len -> 1
        e.apply(Cmd::Free { node: NodeId(0) }); // nulls that write (hole at 0)
        // A new write should reuse the hole, not grow writes_len.
        e.create(NodeId(1), Kind::Add);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Const(0.4);
        *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!(
            (out[0].l - 0.4).abs() < 1e-6,
            "only node1's write is live: {}",
            out[0].l
        );
    }

    #[test]
    fn bus_gain_scales_output() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::BusGain {
            bus: BusId(0),
            gain: 0.5,
        });
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!(
            (out[0].l - 0.4).abs() < 1e-6,
            "0.8 * 0.5 = 0.4, got {}",
            out[0].l
        );
    }

    #[test]
    fn bus_gain_default_is_byte_identical() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.7);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.7).abs() < 1e-6); // no BusGain → unity, unchanged
    }

    #[test]
    fn bus_gain_reset_restores_unity() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::BusGain {
            bus: BusId(0),
            gain: 0.5,
        });
        e.apply(Cmd::Reset);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!(
            (out[0].l - 0.8).abs() < 1e-6,
            "Reset → unity, got {}",
            out[0].l
        );
    }

    #[test]
    fn bus_gain_out_of_range_is_noop() {
        let mut e = E::new(16.0);
        e.apply(Cmd::BusGain {
            bus: BusId(99),
            gain: 0.5,
        }); // BUSES=4 → no-op, no panic
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.6);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.6).abs() < 1e-6); // unaffected
    }

    #[test]
    fn bus_send_folds_stereo_into_target() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        // node0 → busA(1) with L=0.8, R=0.2 (asymmetric, proves L→L/R→R).
        e.bus_write_gains(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(1),
            1.0,
            0.25,
        );
        e.apply(Cmd::BusSend {
            from: BusId(1),
            to: BusId(0),
            gain: 0.5,
        });
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        // busA L=0.8, R=0.2 → sent ×0.5 into master → L=0.4, R=0.1.
        assert!((out[0].l - 0.4).abs() < 1e-6, "L: {}", out[0].l);
        assert!((out[0].r - 0.1).abs() < 1e-6, "R: {}", out[0].r);
    }

    #[test]
    fn bus_send_chain_routes_same_block() {
        // bus2 → bus1 → bus0 (master), each from > to, descending-from ordering.
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.4);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(2),
        ); // node → bus2
        e.apply(Cmd::BusSend {
            from: BusId(2),
            to: BusId(1),
            gain: 1.0,
        });
        e.apply(Cmd::BusSend {
            from: BusId(1),
            to: BusId(0),
            gain: 1.0,
        });
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!(
            (out[0].l - 0.4).abs() < 1e-6,
            "chain to master: {}",
            out[0].l
        );
    }

    #[test]
    fn bus_send_none_is_byte_identical() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.7);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.7).abs() < 1e-6); // no sends → unchanged
    }

    #[test]
    fn bus_send_self_and_oob_are_noop() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.6);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.apply(Cmd::BusSend {
            from: BusId(0),
            to: BusId(0),
            gain: 2.0,
        }); // self → no-op
        e.apply(Cmd::BusSend {
            from: BusId(9),
            to: BusId(0),
            gain: 2.0,
        }); // oob → no-op
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil); // must not panic
        assert!(
            (out[0].l - 0.6).abs() < 1e-6,
            "self/oob send no-op: {}",
            out[0].l
        );
    }

    #[test]
    fn bus_send_is_post_fader() {
        // A gained bus that sends to master carries the POST-fader signal:
        // node0=0.8 → busA(1); busA.gain=0.5 → busA=0.4; busA.send(master,1.0)
        // → master=0.4 (NOT 0.8 — which is what a pre-fader send would give).
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.8);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(1),
        );
        e.apply(Cmd::BusGain {
            bus: BusId(1),
            gain: 0.5,
        });
        e.apply(Cmd::BusSend {
            from: BusId(1),
            to: BusId(0),
            gain: 1.0,
        });
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!(
            (out[0].l - 0.4).abs() < 1e-6,
            "post-fader send should be 0.4, got {}",
            out[0].l
        );
    }

    #[test]
    fn bus_fed_node_reads_previous_block() {
        let mut e = E::new(16.0);
        // node0 = const 0.25, written (center) into bus1 → bus_l[1]=bus_r[1]=0.25.
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.25);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(1),
        );
        // node1 = reads bus1 (Add(bus1, 0)), routed to master bus0. Note
        // `Input::Bus` sums L+R, so a 0.25 center write reads back as 0.5.
        e.create(NodeId(1), Kind::Add);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Bus(BusId(1));
        *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));

        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        // Block 1: node1 reads bus1 = 0 (initial), so master ~0.
        e.render(&mut out, &sil);
        assert!(
            out[0].l.abs() < 1e-6,
            "block1 bus read should be 0: {}",
            out[0].l
        );
        // Block 2: node1 reads bus1 = block1's write (0.25 center → L+R sum = 0.5)
        // → node1 = 0.5 → master = 0.5. Proves the one-block-late bus→node read.
        e.render(&mut out, &sil);
        assert!(
            (out[0].l - 0.5).abs() < 1e-6,
            "block2 one-block-late read: {}",
            out[0].l
        );
    }

    #[test]
    fn existing_routing_byte_identical_after_reorder() {
        // Mirrors two_nodes_sum_into_master_bus: no Input::Bus reads → unchanged.
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.3);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.create(NodeId(1), Kind::Add);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Const(0.4);
        *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(0),
        );
        e.bus_write(
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        assert!((out[0].l - 0.7).abs() < 1e-6); // 0.3 + 0.4, same as before the reorder
    }

    #[test]
    fn reset_zeros_buses() {
        let mut e = E::new(16.0);
        // Drive bus1 with content over a block.
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.9);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(1),
        );
        e.set_root(BusId(1));
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil); // bus1 now holds 0.9
        e.apply(Cmd::Reset);
        // Rebuild a bus1-reading graph; first render must read a zeroed bus1.
        e.create(NodeId(1), Kind::Add);
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Bus(BusId(1));
        *e.node_input_mut(NodeId(1), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            BusId(0),
        );
        e.set_root(BusId(0));
        e.render(&mut out, &sil);
        assert!(
            out[0].l.abs() < 1e-6,
            "Reset must zero buses; got stale {}",
            out[0].l
        );
    }

    #[test]
    fn fill_usb_routes_bus_side_and_node() {
        let mut e = E::new(16.0);
        // node0 = const 0.3 → bus1 (center write → bus_l[1] = 0.3).
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.3);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(1),
        );
        e.set_root(BusId(0));
        // Route USB ch0 = bus1 left, ch1 = node0 output port 0.
        e.apply(Cmd::SetUsbOut {
            channel: 0,
            src: OutputSrc::BusL(BusId(1)),
        });
        e.apply(Cmd::SetUsbOut {
            channel: 1,
            src: OutputSrc::Node {
                node: NodeId(0),
                port: 0,
            },
        });
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        let mut usb = [[0.0f32; 16]; USB_CHANNELS];
        e.fill_usb(&mut usb);
        assert!(
            (usb[0][0] - 0.3).abs() < 1e-6,
            "ch0 = bus1 left: {}",
            usb[0][0]
        );
        assert!(
            (usb[1][0] - 0.3).abs() < 1e-6,
            "ch1 = node0 out: {}",
            usb[1][0]
        );
        assert!(usb[2][0].abs() < 1e-6, "ch2 silent"); // default Silent
        assert!(usb[7][0].abs() < 1e-6, "ch7 silent");
    }

    #[test]
    fn set_usb_out_of_range_is_noop() {
        let mut e = E::new(16.0);
        e.apply(Cmd::SetUsbOut {
            channel: 99,
            src: OutputSrc::BusL(BusId(0)),
        }); // no panic
        let mut usb = [[0.0f32; 16]; USB_CHANNELS];
        e.fill_usb(&mut usb); // no panic
        assert!(usb[0][0].abs() < 1e-6);
    }

    #[test]
    fn usb_dangling_node_is_silent() {
        let mut e = E::new(16.0);
        e.apply(Cmd::SetUsbOut {
            channel: 0,
            src: OutputSrc::Node {
                node: NodeId(50),
                port: 0,
            },
        });
        let mut usb = [[0.0f32; 16]; USB_CHANNELS];
        e.fill_usb(&mut usb); // dangling node → silence, no panic
        assert!(usb[0][0].abs() < 1e-6);
    }

    #[test]
    fn reset_clears_usb_map() {
        let mut e = E::new(16.0);
        e.create(NodeId(0), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Const(0.3);
        *e.node_input_mut(NodeId(0), 1).unwrap() = Input::Const(0.0);
        e.bus_write(
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            BusId(1),
        );
        e.apply(Cmd::SetUsbOut {
            channel: 0,
            src: OutputSrc::BusL(BusId(1)),
        });
        e.apply(Cmd::Reset);
        let mut out = [StereoFrame::default(); 16];
        let sil = [StereoFrame::default(); 16];
        e.render(&mut out, &sil);
        let mut usb = [[0.0f32; 16]; USB_CHANNELS];
        e.fill_usb(&mut usb);
        assert!(
            usb[0][0].abs() < 1e-6,
            "Reset should clear the usb map, got {}",
            usb[0][0]
        );
    }

    // ── Sample clock (transport seam for scheduled commands) ──────────────

    #[test]
    fn sample_time_advances_by_one_block_per_render() {
        let mut e = E::new(48_000.0);
        assert_eq!(e.sample_time(), 0, "a fresh engine starts at sample 0");
        e.render_block();
        assert_eq!(e.sample_time(), 16, "one block of BLOCK=16 samples elapsed");
        e.render_block();
        assert_eq!(e.sample_time(), 32);
    }

    #[test]
    fn sample_time_advances_through_the_full_render_path() {
        // `render` calls `render_block` exactly once, so the clock must not
        // double-count when the host uses the outer entry point.
        let mut e = E::new(48_000.0);
        let mut out = [StereoFrame { l: 0.0, r: 0.0 }; 16];
        e.render(&mut out, &[]);
        assert_eq!(e.sample_time(), 16);
    }

    #[test]
    fn reset_rewinds_the_sample_clock() {
        let mut e = E::new(48_000.0);
        e.render_block();
        e.apply(Cmd::Reset);
        assert_eq!(e.sample_time(), 0, "Reset returns the engine to time zero");
    }

    // ── Command failure reporting (GL7) ──────────────────────────────────

    #[test]
    fn a_new_node_that_cannot_be_created_reports_cmd_failed() {
        // NODES = 8, so id 8 is out of range: the node is never created and the
        // host would otherwise wire a whole patch onto silence without knowing.
        let mut e = E::new(48_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(8),
            kind: Kind::Saw,
            args: [Input::Const(0.0); 3],
        });
        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::CmdFailed {
                node: NodeId(8),
                reason: crate::event::CmdError::CreateFailed,
            })
        );
    }

    #[test]
    fn creating_a_node_twice_reports_cmd_failed() {
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::Saw);
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Saw,
            args: [Input::Const(0.0); 3],
        });
        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::CmdFailed {
                node: NodeId(0),
                reason: crate::event::CmdError::CreateFailed,
            })
        );
    }

    #[test]
    fn wiring_an_input_on_a_dead_node_reports_cmd_failed() {
        let mut e = E::new(48_000.0);
        e.apply(Cmd::SetInput {
            node: NodeId(3),
            port: 0,
            src: Input::Const(1.0),
        });
        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::CmdFailed {
                node: NodeId(3),
                reason: crate::event::CmdError::DeadNode,
            })
        );
    }

    #[test]
    fn a_patch_that_builds_cleanly_reports_no_failure() {
        let mut e = E::new(48_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Saw,
            args: [Input::Const(220.0); 3],
        });
        e.apply(Cmd::SetInput {
            node: NodeId(0),
            port: 0,
            src: Input::Const(110.0),
        });
        e.apply(Cmd::SetParam {
            node: NodeId(0),
            param: 0,
            value: 0.5,
        });
        assert_eq!(e.pop_event(), None, "a clean build emits no events");
    }

    #[test]
    fn performance_commands_on_a_dead_node_stay_silent() {
        // Gate/Trigger run at note rate; a stuck note must not flood the queue
        // and evict envelope-completion events. Only build-time commands report.
        let mut e = E::new(48_000.0);
        e.apply(Cmd::Gate {
            node: NodeId(4),
            on: true,
        });
        e.apply(Cmd::Trigger { node: NodeId(4) });
        assert_eq!(e.pop_event(), None);
    }

    #[test]
    fn control_rate_on_a_poly_node_reports_unsupported_rate_not_a_dead_node() {
        // `set_rate` refuses width > 1, but the node is alive and well — telling
        // the host "dead node" would send it hunting for a lifecycle bug that
        // isn't there.
        use crate::node::Rate;
        let mut e = E::new(48_000.0);
        e.create(NodeId(0), Kind::PolyOsc);
        e.apply(Cmd::SetRate {
            node: NodeId(0),
            rate: Rate::Control,
        });
        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::CmdFailed {
                node: NodeId(0),
                reason: crate::event::CmdError::UnsupportedRate,
            })
        );
    }

    // ── G11: automatic topological eval order ────────────────────────────

    #[test]
    fn a_consumer_created_before_its_source_reads_the_current_block() {
        // The hazard G4b existed to repair by hand: build 0 → 1 → 2 in the
        // wrong creation order (0, 2, 1). The engine now sorts before it
        // renders, so node 2 sees node 1's CURRENT block on the very first
        // render — no stale zero, no manual `MoveAfter`.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        let add = |e: &mut ME, id: u16, src: Input, k: f32| {
            e.apply(Cmd::NewNode {
                node: NodeId(id),
                kind: Kind::Add,
                args: [src, Input::Const(k), Input::Const(0.0)],
            });
        };
        add(&mut e, 0, Input::Const(1.0), 0.0); // → 1.0
        add(
            &mut e,
            2,
            Input::Node {
                node: NodeId(1),
                port: 0,
            },
            0.0,
        );
        add(
            &mut e,
            1,
            Input::Node {
                node: NodeId(0),
                port: 0,
            },
            10.0,
        ); // → 11.0

        assert_eq!(e.eval_order(), &[0, 2, 1], "creation order, before render");
        e.render_block();
        assert_eq!(e.eval_order(), &[0, 1, 2], "sorted at render time");
        assert_eq!(
            e.node_output(NodeId(2), 0)[0],
            11.0,
            "current block, not the previous one"
        );
    }

    #[test]
    fn sorting_runs_once_per_block_not_once_per_command() {
        // The dirty flag must clear: a second render with no edits in between
        // must not re-sort (and so must not disturb an order the author set
        // with `MoveBefore` / `MoveAfter`).
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.create(NodeId(0), Kind::Saw);
        e.create(NodeId(1), Kind::Saw);
        e.render_block();
        e.apply(Cmd::MoveBefore {
            node: NodeId(1),
            target: NodeId(0),
        });
        assert_eq!(e.eval_order(), &[1, 0]);
        e.render_block();
        assert_eq!(
            e.eval_order(),
            &[1, 0],
            "no structural change, so no re-sort to undo the move"
        );
    }

    #[test]
    fn an_explicit_move_survives_sorting_when_it_respects_dependencies() {
        // Independent nodes: a move expresses author intent the sort has no
        // reason to overrule, so it must survive the next structural change.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.create(NodeId(0), Kind::Saw);
        e.create(NodeId(1), Kind::Saw);
        e.create(NodeId(2), Kind::Saw);
        e.apply(Cmd::MoveBefore {
            node: NodeId(2),
            target: NodeId(0),
        });
        assert_eq!(e.eval_order(), &[2, 0, 1]);
        e.create(NodeId(3), Kind::Saw); // dirties the graph → re-sort
        e.render_block();
        assert_eq!(e.eval_order(), &[2, 0, 1, 3], "stable: the move is kept");
    }

    #[test]
    fn a_feedback_cycle_still_renders_and_keeps_its_authors_order() {
        // Two nodes reading each other have no topological order. The engine
        // must render them, not hang, and leave the author's choice of which
        // one reads a block late alone.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.create(NodeId(0), Kind::Add);
        e.create(NodeId(1), Kind::Add);
        *e.node_input_mut(NodeId(0), 0).unwrap() = Input::Node {
            node: NodeId(1),
            port: 0,
        };
        *e.node_input_mut(NodeId(1), 0).unwrap() = Input::Node {
            node: NodeId(0),
            port: 0,
        };
        e.render_block();
        assert_eq!(e.eval_order(), &[0, 1]);
    }

    // ── G11: reachability culling (opt-in) ───────────────────────────────

    /// Two saws: node 0 written to the root bus, node 1 wired to nothing.
    fn one_live_one_orphan(e: &mut Engine<8, 8, 8, 4, 45056, 2048>) {
        for id in [0u16, 1] {
            e.apply(Cmd::NewNode {
                node: NodeId(id),
                kind: Kind::Saw,
                args: [Input::Const(2_000.0), Input::Const(0.0), Input::Const(0.0)],
            });
        }
        e.apply(Cmd::BusWrite {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: BusId(0),
        });
        e.apply(Cmd::SetRoot { bus: BusId(0) });
    }

    #[test]
    fn culling_is_off_by_default_so_an_orphan_node_still_runs() {
        // The engine cannot know what the host reads — `node_output`,
        // `fill_usb`, a prefetch cursor — so it must not decide on its own that
        // a node is pointless.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        one_live_one_orphan(&mut e);
        e.render_block();
        assert_ne!(
            e.node_output(NodeId(1), 0)[1],
            0.0,
            "the orphan rendered anyway"
        );
    }

    #[test]
    fn culling_skips_a_node_that_reaches_no_output() {
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.set_cull_unreachable(true);
        one_live_one_orphan(&mut e);
        e.render_block();
        assert_ne!(e.node_output(NodeId(0), 0)[1], 0.0, "node 0 feeds the root");
        assert_eq!(
            e.node_output(NodeId(1), 0),
            &[0.0; 8],
            "the orphan was never evaluated"
        );
    }

    #[test]
    fn culling_keeps_every_node_that_feeds_a_written_node() {
        // Reachability is transitive: node 1 feeds node 0, which is written to
        // the root bus, so node 1 must still run.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.set_cull_unreachable(true);
        e.apply(Cmd::NewNode {
            node: NodeId(1),
            kind: Kind::Saw,
            args: [Input::Const(2_000.0), Input::Const(0.0), Input::Const(0.0)],
        });
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Add,
            args: [
                Input::Node {
                    node: NodeId(1),
                    port: 0,
                },
                Input::Const(0.0),
                Input::Const(0.0),
            ],
        });
        e.apply(Cmd::BusWrite {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: BusId(0),
        });
        e.apply(Cmd::SetRoot { bus: BusId(0) });
        e.render_block();
        assert_ne!(e.node_output(NodeId(1), 0)[1], 0.0, "source still runs");
        assert_eq!(
            e.node_output(NodeId(0), 0)[1],
            e.node_output(NodeId(1), 0)[1]
        );
    }

    #[test]
    fn culling_treats_a_usb_routed_node_as_an_output_root() {
        // A node routed to USB reaches an output without touching any bus.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.set_cull_unreachable(true);
        e.apply(Cmd::NewNode {
            node: NodeId(1),
            kind: Kind::Saw,
            args: [Input::Const(2_000.0), Input::Const(0.0), Input::Const(0.0)],
        });
        e.apply(Cmd::SetUsbOut {
            channel: 0,
            src: OutputSrc::Node {
                node: NodeId(1),
                port: 0,
            },
        });
        e.render_block();
        assert_ne!(e.node_output(NodeId(1), 0)[1], 0.0);
    }

    #[test]
    fn wiring_a_culled_node_up_brings_it_back() {
        // Reachability must be recomputed when routing changes, not frozen at
        // the first render.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.set_cull_unreachable(true);
        one_live_one_orphan(&mut e);
        e.render_block();
        assert_eq!(e.node_output(NodeId(1), 0), &[0.0; 8]);
        e.apply(Cmd::BusWrite {
            src: Input::Node {
                node: NodeId(1),
                port: 0,
            },
            bus: BusId(0),
        });
        e.render_block();
        assert_ne!(
            e.node_output(NodeId(1), 0)[1],
            0.0,
            "now it reaches the bus"
        );
    }

    // ── GL2: epoch mark-and-sweep patch update ───────────────────────────

    fn saw(e: &mut Engine<8, 8, 8, 4, 45056, 2048>, id: u16, hz: f32) {
        e.apply(Cmd::NewNode {
            node: NodeId(id),
            kind: Kind::Saw,
            args: [Input::Const(hz), Input::Const(0.0), Input::Const(0.0)],
        });
    }

    #[test]
    fn re_emitting_an_unchanged_node_inside_an_update_preserves_its_state() {
        // The whole point of GL2: re-running the patch script must not restart
        // the oscillator that the script did not change.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut control = ME::new(48_000.0);
        saw(&mut control, 0, 2_000.0);
        control.render_block();
        control.render_block();
        let expected = control.node_output(NodeId(0), 0)[0];

        let mut e = ME::new(48_000.0);
        saw(&mut e, 0, 2_000.0);
        e.render_block();
        e.apply(Cmd::BeginUpdate);
        saw(&mut e, 0, 2_000.0); // same id, same kind: keep the running node
        e.apply(Cmd::EndUpdate);
        e.render_block();
        assert_eq!(
            e.node_output(NodeId(0), 0)[0],
            expected,
            "phase continued across the update"
        );
        assert_eq!(e.pop_event(), None, "not a failure, and nothing was freed");
    }

    #[test]
    fn changing_a_nodes_kind_inside_an_update_replaces_it() {
        // State cannot survive a kind change — there is no meaningful way to
        // carry a saw's phase into a square's — so the node is rebuilt.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        saw(&mut e, 0, 2_000.0);
        e.render_block();
        e.apply(Cmd::BeginUpdate);
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Square,
            args: [Input::Const(2_000.0), Input::Const(0.0), Input::Const(0.0)],
        });
        e.apply(Cmd::EndUpdate);
        assert_eq!(e.kind_of(NodeId(0)), Some(Kind::Square));
    }

    #[test]
    fn a_node_the_new_patch_omits_is_swept_and_announced() {
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        saw(&mut e, 0, 2_000.0);
        saw(&mut e, 1, 3_000.0);
        e.render_block();
        e.apply(Cmd::BeginUpdate);
        saw(&mut e, 0, 2_000.0); // the new patch mentions only node 0
        e.apply(Cmd::EndUpdate);
        assert_eq!(e.kind_of(NodeId(0)), Some(Kind::Saw), "kept");
        assert_eq!(e.kind_of(NodeId(1)), None, "swept");
        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::Freed { node: NodeId(1) })
        );
    }

    #[test]
    fn an_update_that_re_emits_everything_frees_nothing() {
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        saw(&mut e, 0, 2_000.0);
        saw(&mut e, 1, 3_000.0);
        e.apply(Cmd::BeginUpdate);
        saw(&mut e, 0, 2_000.0);
        saw(&mut e, 1, 3_000.0);
        e.apply(Cmd::EndUpdate);
        assert_eq!(e.kind_of(NodeId(0)), Some(Kind::Saw));
        assert_eq!(e.kind_of(NodeId(1)), Some(Kind::Saw));
        assert_eq!(e.pop_event(), None);
    }

    #[test]
    fn creating_a_live_id_outside_an_update_still_fails() {
        // Outside an update, a duplicate `NewNode` is a host bug, not an edit.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        saw(&mut e, 0, 2_000.0);
        saw(&mut e, 0, 2_000.0);
        assert_eq!(
            e.pop_event(),
            Some(crate::event::Event::CmdFailed {
                node: NodeId(0),
                reason: crate::event::CmdError::CreateFailed,
            })
        );
    }

    #[test]
    fn an_update_rewires_a_surviving_node_without_rebuilding_it() {
        // The insert-a-stage case: node 1 keeps running, but now reads node 2,
        // which the update introduced.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        saw(&mut e, 0, 2_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(1),
            kind: Kind::Add,
            args: [
                Input::Node {
                    node: NodeId(0),
                    port: 0,
                },
                Input::Const(0.0),
                Input::Const(0.0),
            ],
        });
        e.render_block();

        e.apply(Cmd::BeginUpdate);
        saw(&mut e, 0, 2_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(2),
            kind: Kind::Add,
            args: [
                Input::Node {
                    node: NodeId(0),
                    port: 0,
                },
                Input::Const(1.0),
                Input::Const(0.0),
            ],
        });
        e.apply(Cmd::NewNode {
            node: NodeId(1),
            kind: Kind::Add,
            args: [
                Input::Node {
                    node: NodeId(2),
                    port: 0,
                },
                Input::Const(0.0),
                Input::Const(0.0),
            ],
        });
        e.apply(Cmd::EndUpdate);
        e.render_block();
        assert_eq!(
            e.eval_order(),
            &[0, 2, 1],
            "the new stage sorted into place"
        );
        assert_eq!(
            e.node_output(NodeId(1), 0)[0],
            e.node_output(NodeId(0), 0)[0] + 1.0
        );
    }

    #[test]
    fn re_emitting_a_bus_write_does_not_double_it() {
        // A bus write is a routing statement, not an accumulator. Re-running
        // the patch script re-emits every write; appending a second entry would
        // add 6 dB per edit.
        type ME = Engine<8, 8, 8, 4, 45056, 2048>;
        let mut e = ME::new(48_000.0);
        e.apply(Cmd::NewNode {
            node: NodeId(0),
            kind: Kind::Add,
            args: [Input::Const(0.25), Input::Const(0.0), Input::Const(0.0)],
        });
        let write = Cmd::BusWrite {
            src: Input::Node {
                node: NodeId(0),
                port: 0,
            },
            bus: BusId(0),
        };
        e.apply(write);
        e.apply(Cmd::SetRoot { bus: BusId(0) });
        let mut out = [StereoFrame::default(); 8];
        e.render(&mut out, &[]);
        let once = out[0].l;
        e.apply(write);
        e.render(&mut out, &[]);
        assert_eq!(out[0].l, once, "the second write replaced, not stacked");
    }
}
