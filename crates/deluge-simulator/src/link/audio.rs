//! The real-time audio bridge between the app's DSP loop and the GUI's audio
//! device, over two lock-free SPSC rings.
//!
//! Direction is named from the app's point of view:
//! - **out** — app → GUI: processed frames the app sends to the speakers.
//! - **in** — GUI → app: captured frames (mic / loopback) the app reads.
//!
//! The GUI's audio callback is the clock: it drains `out` to fill the output
//! device and fills `in` from the input device at the hardware rate, so the app's
//! block loop is paced by real time without a timer.

use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

/// Codec sample rate the simulator presents (matches the device).
pub const SAMPLE_RATE_HZ: u32 = 44_100;
/// Stereo frames per DSP block (matches the device).
pub const BLOCK_FRAMES: usize = 128;
/// Ring depth in blocks — headroom to absorb callback vs. block-loop jitter.
const RING_BLOCKS: usize = 8;

/// One stereo sample frame, `[left, right]` in `[-1.0, 1.0]`.
pub type Sample = [f32; 2];

/// The writing end of one audio ring.
pub struct AudioProducer(pub(crate) HeapProd<Sample>);

impl AudioProducer {
    /// Free space, in frames.
    pub fn vacant_len(&self) -> usize {
        self.0.vacant_len()
    }

    /// Push one frame, handing it back if the ring is full.
    pub fn try_push(&mut self, frame: Sample) -> Result<(), Sample> {
        self.0.try_push(frame)
    }

    /// Push as many of `frames` as fit and return how many that was.
    pub fn push_slice(&mut self, frames: &[Sample]) -> usize {
        self.0.push_slice(frames)
    }
}

/// The reading end of one audio ring.
pub struct AudioConsumer(pub(crate) HeapCons<Sample>);

impl AudioConsumer {
    /// Buffered frames.
    pub fn occupied_len(&self) -> usize {
        self.0.occupied_len()
    }

    /// Pop one frame, if any is buffered.
    pub fn try_pop(&mut self) -> Option<Sample> {
        self.0.try_pop()
    }

    /// Pop frames into `out` and return how many were copied.
    pub fn pop_slice(&mut self, out: &mut [Sample]) -> usize {
        self.0.pop_slice(out)
    }

    /// Pop every buffered frame, in order.
    pub fn pop_iter(&mut self) -> impl Iterator<Item = Sample> + '_ {
        self.0.pop_iter()
    }
}

/// The app-side ring endpoints (live in the SDK's simulator backend).
pub struct BrainEnds {
    /// App → GUI: push processed output frames here.
    pub out: AudioProducer,
    /// GUI → app: pop captured input frames here.
    pub in_: AudioConsumer,
}

/// The GUI-side ring endpoints (live in the simulator's audio callback).
pub struct GuiEnds {
    /// App → GUI: pop output frames to feed the speakers.
    pub out: AudioConsumer,
    /// GUI → app: push captured frames from the input device.
    pub in_: AudioProducer,
}

/// Create the paired audio rings, returning the app-side and GUI-side endpoints.
pub fn new_bridge() -> (BrainEnds, GuiEnds) {
    let cap = BLOCK_FRAMES * RING_BLOCKS;
    let (out_prod, out_cons) = HeapRb::<Sample>::new(cap).split();
    let (in_prod, in_cons) = HeapRb::<Sample>::new(cap).split();
    (
        BrainEnds {
            out: AudioProducer(out_prod),
            in_: AudioConsumer(in_cons),
        },
        GuiEnds {
            out: AudioConsumer(out_cons),
            in_: AudioProducer(in_prod),
        },
    )
}
