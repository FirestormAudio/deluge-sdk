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

// Re-export the ring traits + types so consumers (the BSP host backend, the
// simulator) can build and drive rings without a direct `ringbuf` dependency.
// `HeapRb` + `Split` let the simulator build its own mono scope-monitor ring.
pub use ringbuf::traits::{Consumer, Observer, Producer, Split};
pub use ringbuf::{HeapCons, HeapProd, HeapRb};

/// Codec sample rate the simulator presents (matches the device).
pub const SAMPLE_RATE_HZ: u32 = 44_100;
/// Stereo frames per DSP block (matches the device).
pub const BLOCK_FRAMES: usize = 128;
/// Ring depth in blocks — headroom to absorb callback vs. block-loop jitter.
const RING_BLOCKS: usize = 8;

/// One stereo sample frame, `[left, right]` in `[-1.0, 1.0]`.
pub type Sample = [f32; 2];

/// The app-side ring endpoints (live in the BSP host backend).
pub struct BrainEnds {
    /// App → GUI: push processed output frames here.
    pub out: HeapProd<Sample>,
    /// GUI → app: pop captured input frames here.
    pub in_: HeapCons<Sample>,
}

/// The GUI-side ring endpoints (live in the simulator's audio callback).
pub struct GuiEnds {
    /// App → GUI: pop output frames to feed the speakers.
    pub out: HeapCons<Sample>,
    /// GUI → app: push captured frames from the input device.
    pub in_: HeapProd<Sample>,
}

/// Create the paired audio rings, returning the app-side and GUI-side endpoints.
pub fn new_bridge() -> (BrainEnds, GuiEnds) {
    let cap = BLOCK_FRAMES * RING_BLOCKS;
    let (out_prod, out_cons) = HeapRb::<Sample>::new(cap).split();
    let (in_prod, in_cons) = HeapRb::<Sample>::new(cap).split();
    (
        BrainEnds {
            out: out_prod,
            in_: in_cons,
        },
        GuiEnds {
            out: out_cons,
            in_: in_prod,
        },
    )
}
