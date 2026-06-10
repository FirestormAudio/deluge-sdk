//! Native DSP audio engine (M5).
//!
//! A small fixed node graph rendered per-sample at 44.1 kHz by [`audio_task`],
//! which streams into the SSI0 codec TX buffer (`rza1l_hal::ssi`, SCUX bypassed
//! — see `deluge_bsp::audio::init_direct`). Wren scripts build/patch/modulate the
//! graph at control rate; they never touch engine memory directly.
//!
//! ## Concurrency
//! The [`Engine`] is owned solely by `audio_task`. `vm_task` (via the `Node`
//! foreign bindings) only enqueues [`Cmd`]s onto [`CMD_RING`]; `audio_task` drains
//! them between render blocks. Node ids are handed out by [`NEXT_ID`] so a foreign
//! ctor can return an id immediately without touching the graph.
//!
//! ## Eval order
//! Nodes are evaluated in creation order. Wren builds graphs bottom-up (a node's
//! inputs are always created first → smaller ids), so creation order is a valid
//! topological order and no sorting is needed.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU16, Ordering};

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Duration, Timer};
use rza1l_hal::ssi;

const MAX_NODES: usize = 64;
const SAMPLE_RATE: f32 = 44_100.0;
const DT: f32 = 1.0 / SAMPLE_RATE;
const PI: f32 = core::f32::consts::PI;

// Node kind codes (shared with the Wren bindings).
pub const K_SINE: u8 = 0;
pub const K_SAW: u8 = 1;
pub const K_SQUARE: u8 = 2;
pub const K_TRI: u8 = 3;
pub const K_NOISE: u8 = 4;
pub const K_ENV: u8 = 5;
pub const K_LPF: u8 = 6;
pub const K_MUL: u8 = 7;
pub const K_ADD: u8 = 8;
pub const K_SUB: u8 = 9;

// Envelope stages.
const ST_IDLE: u8 = 0;
const ST_ATTACK: u8 = 1;
const ST_SUSTAIN: u8 = 2;
const ST_RELEASE: u8 = 3;

/// A node input: a constant, or another node's output (by id).
#[derive(Clone, Copy)]
pub enum Input {
    Const(f32),
    Node(u16),
}

impl Input {
    #[inline]
    fn eval(self, outs: &[f32; MAX_NODES]) -> f32 {
        match self {
            Input::Const(c) => c,
            Input::Node(i) => outs[i as usize],
        }
    }
}

#[derive(Clone, Copy)]
struct Node {
    kind: u8,
    a: Input, // freq / input / operand0
    b: Input, // release / cutoff / operand1
    phase: f32,
    z: f32, // filter state / env level
    stage: u8,
    oneshot: bool,
    rng: u32,
}

impl Node {
    const EMPTY: Node = Node {
        kind: 255,
        a: Input::Const(0.0),
        b: Input::Const(0.0),
        phase: 0.0,
        z: 0.0,
        stage: ST_IDLE,
        oneshot: false,
        rng: 0x2545_F491,
    };

    fn with(kind: u8, a: Input, b: Input) -> Node {
        Node { kind, a, b, ..Node::EMPTY }
    }

    #[inline]
    fn eval(&mut self, outs: &[f32; MAX_NODES]) -> f32 {
        match self.kind {
            K_SINE | K_SAW | K_SQUARE | K_TRI => {
                let f = self.a.eval(outs);
                self.phase += f * DT;
                self.phase -= libm_floorf(self.phase);
                match self.kind {
                    K_SINE => fast_sin(self.phase),
                    K_SAW => 2.0 * self.phase - 1.0,
                    K_SQUARE => {
                        if self.phase < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    _ => 1.0 - 4.0 * (self.phase - 0.5).abs(), // tri
                }
            }
            K_NOISE => {
                // xorshift32 → [-1, 1)
                let mut r = self.rng;
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                self.rng = r;
                (r as i32 as f32) / (i32::MAX as f32)
            }
            K_ENV => {
                let atk = self.a.eval(outs).max(0.0001);
                let rel = self.b.eval(outs).max(0.0001);
                match self.stage {
                    ST_ATTACK => {
                        self.z += DT / atk;
                        if self.z >= 1.0 {
                            self.z = 1.0;
                            self.stage = if self.oneshot { ST_RELEASE } else { ST_SUSTAIN };
                        }
                    }
                    ST_SUSTAIN => self.z = 1.0,
                    ST_RELEASE => {
                        self.z -= DT / rel;
                        if self.z <= 0.0 {
                            self.z = 0.0;
                            self.stage = ST_IDLE;
                        }
                    }
                    _ => self.z = 0.0,
                }
                self.z
            }
            K_LPF => {
                let x = self.a.eval(outs);
                let fc = self.b.eval(outs).max(1.0);
                let c = (2.0 * PI * fc * DT).min(1.0);
                self.z += c * (x - self.z);
                self.z
            }
            K_MUL => self.a.eval(outs) * self.b.eval(outs),
            K_ADD => self.a.eval(outs) + self.b.eval(outs),
            K_SUB => self.a.eval(outs) - self.b.eval(outs),
            _ => 0.0,
        }
    }
}

/// Fast sine of a normalised phase `p` in `[0,1)` (≈ `sin(2π p)`), via the
/// classic parabola + correction — ~0.1% error, no `libm`/table.
#[inline]
fn fast_sin(p: f32) -> f32 {
    let mut x = 2.0 * PI * p;
    if x > PI {
        x -= 2.0 * PI;
    }
    const B: f32 = 4.0 / PI;
    const C: f32 = -4.0 / (PI * PI);
    let y = B * x + C * x * x.abs();
    0.225 * (y * y.abs() - y) + y
}

/// `floorf` without a libm dependency (phase is small + finite here).
#[inline]
fn libm_floorf(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

// ── Engine ───────────────────────────────────────────────────────────────────

struct Engine {
    nodes: [Node; MAX_NODES],
    outs: [f32; MAX_NODES],
    live: usize,
    root: Option<u16>,
}

impl Engine {
    const fn new() -> Self {
        Engine {
            nodes: [Node::EMPTY; MAX_NODES],
            outs: [0.0; MAX_NODES],
            live: 0,
            root: None,
        }
    }

    fn node_mut(&mut self, id: u16) -> Option<&mut Node> {
        let i = id as usize;
        if i < self.live { Some(&mut self.nodes[i]) } else { None }
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::NewNode { id, kind, a, b } => {
                let i = id as usize;
                if i < MAX_NODES {
                    self.nodes[i] = Node::with(kind, a, b);
                    if i + 1 > self.live {
                        self.live = i + 1;
                    }
                }
            }
            Cmd::SetInput { id, port, src } => {
                if let Some(n) = self.node_mut(id) {
                    if port == 0 {
                        n.a = src;
                    } else {
                        n.b = src;
                    }
                }
            }
            Cmd::Gate { id, on } => {
                if let Some(n) = self.node_mut(id) {
                    n.oneshot = false;
                    n.stage = if on { ST_ATTACK } else { ST_RELEASE };
                }
            }
            Cmd::Trigger { id } => {
                if let Some(n) = self.node_mut(id) {
                    n.oneshot = true;
                    n.stage = ST_ATTACK;
                }
            }
            Cmd::SetRoot { id } => self.root = Some(id),
            Cmd::Reset => {
                self.live = 0;
                self.root = None;
            }
            Cmd::Nop => {}
        }
    }

    #[inline]
    fn render_frame(&mut self) -> f32 {
        for id in 0..self.live {
            // `nodes` and `outs` are disjoint fields → simultaneous borrows OK.
            let v = self.nodes[id].eval(&self.outs);
            self.outs[id] = v;
        }
        match self.root {
            Some(r) => self.outs[r as usize],
            None => 0.0,
        }
    }
}

// SAFETY: ENGINE is touched only by `audio_task` (single accessor).
static mut ENGINE: Engine = Engine::new();

// ── Control → audio command queue ────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Cmd {
    Nop,
    NewNode { id: u16, kind: u8, a: Input, b: Input },
    SetInput { id: u16, port: u8, src: Input },
    Gate { id: u16, on: bool },
    Trigger { id: u16 },
    SetRoot { id: u16 },
    Reset,
}

const CMD_CAP: usize = 256;

struct CmdRing {
    buf: [Cmd; CMD_CAP],
    head: usize,
    tail: usize,
    full: bool,
}

impl CmdRing {
    const fn new() -> Self {
        CmdRing { buf: [Cmd::Nop; CMD_CAP], head: 0, tail: 0, full: false }
    }
    fn push(&mut self, c: Cmd) {
        if self.full {
            return; // drop on overflow (control-rate; shouldn't happen)
        }
        self.buf[self.tail] = c;
        self.tail = (self.tail + 1) % CMD_CAP;
        if self.tail == self.head {
            self.full = true;
        }
    }
    fn pop(&mut self) -> Option<Cmd> {
        if self.head == self.tail && !self.full {
            return None;
        }
        let c = self.buf[self.head];
        self.head = (self.head + 1) % CMD_CAP;
        self.full = false;
        Some(c)
    }
}

static CMD_RING: Mutex<CriticalSectionRawMutex, RefCell<CmdRing>> =
    Mutex::new(RefCell::new(CmdRing::new()));
/// Monotonic node-id allocator (reset by [`reset`]).
static NEXT_ID: AtomicU16 = AtomicU16::new(0);

fn push(c: Cmd) {
    CMD_RING.lock(|r| r.borrow_mut().push(c));
}

// ── Engine hooks (called by the `Node` foreign bindings, in vm_task) ──────────

/// Allocate a node of `kind` with inputs `a`/`b`; returns its id. Returns an
/// out-of-range id (>= MAX_NODES) if the pool is full (foreign methods no-op).
pub fn alloc_node(kind: u8, a: Input, b: Input) -> u16 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if (id as usize) < MAX_NODES {
        push(Cmd::NewNode { id, kind, a, b });
    }
    id
}
pub fn set_input(id: u16, port: u8, src: Input) {
    push(Cmd::SetInput { id, port, src });
}
pub fn gate(id: u16, on: bool) {
    push(Cmd::Gate { id, on });
}
pub fn trigger(id: u16) {
    push(Cmd::Trigger { id });
}
pub fn set_root(id: u16) {
    push(Cmd::SetRoot { id });
}
pub fn reset() {
    NEXT_ID.store(0, Ordering::Relaxed);
    push(Cmd::Reset);
}

// ── Render task ──────────────────────────────────────────────────────────────

/// Current DMA read position as a frame index into the TX buffer.
fn cur_frame(frames: usize) -> usize {
    let base = ssi::tx_buf_start() as usize;
    let cur = ssi::tx_current_ptr() as usize;
    ((cur.wrapping_sub(base)) / (2 * core::mem::size_of::<i32>())) % frames
}

/// Renders the DSP graph into the SSI0 TX buffer, staying a half-buffer ahead of
/// the DMA read pointer. Mono output is duplicated to L+R.
#[embassy_executor::task]
pub async fn audio_task() {
    // SAFETY: this task is the sole accessor of ENGINE.
    let eng = unsafe { &mut *core::ptr::addr_of_mut!(ENGINE) };
    let tx = ssi::tx_buf_start(); // uncached, interleaved [L,R,L,R,…]
    let frames = ssi::TX_FRAMES;

    let mut last = cur_frame(frames);
    let mut write = (last + frames / 2) % frames; // half-buffer lead

    loop {
        // Apply all pending control-rate commands.
        loop {
            let cmd = CMD_RING.lock(|r| r.borrow_mut().pop());
            match cmd {
                Some(c) => eng.apply(c),
                None => break,
            }
        }

        // Render exactly the frames the DMA has consumed since last tick.
        let cur = cur_frame(frames);
        let freed = (cur + frames - last) % frames;
        last = cur;
        for _ in 0..freed {
            let s = eng.render_frame();
            let v = ((s.clamp(-1.0, 1.0) * 8_388_607.0) as i32) << 8;
            // SAFETY: write < frames; tx covers `frames` stereo slots.
            unsafe {
                *tx.add(write * 2) = v;
                *tx.add(write * 2 + 1) = v;
            }
            write = (write + 1) % frames;
        }

        Timer::after(Duration::from_micros(1500)).await;
    }
}
