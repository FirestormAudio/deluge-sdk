//! Audio DSP — a per-block callback over the codec.

use core::sync::atomic::{AtomicBool, Ordering};

use deluge_bsp::audio_block::{self, BlockState};
use embassy_time::{Duration, Ticker};

/// One stereo audio frame; samples in `[-1.0, 1.0]`. `l` = left, `r` = right.
pub use deluge_bsp::audio_block::Frame as StereoFrame;

fn ensure_init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: runs once. Brings up the whole codec path (SSI RX DMA + SCUX → SSIF0
    // TX + codec power); blocks ~5 ms internally. Acquire `audio()` before the
    // main loop. Owns the codec — incompatible with the USB UAC2 device tasks.
    unsafe { deluge_bsp::audio::init() };
}

/// The codec audio path, taken once from [`Deluge::audio`](crate::Deluge::audio).
///
/// Run a DSP callback over every block with [`process`](Audio::process). The
/// block arrives pre-loaded with codec line-in; overwrite it with the output
/// sent to line-out — so the same API serves insert-effects and synths.
///
/// **Owns the codec path.** Do not also run a USB audio (UAC2) device stack; both
/// drive the same SSI/SCUX rings.
pub struct Audio {
    _private: (),
}

impl Audio {
    pub(crate) fn new() -> Self {
        ensure_init();
        Self { _private: () }
    }

    /// Run `f` over every audio block, forever.
    ///
    /// `f` receives a `BLOCK`-length slice pre-loaded with codec input; whatever
    /// it leaves in the slice is sent to the codec. Never returns.
    ///
    /// ```ignore
    /// dlg.audio().process(|block| {
    ///     for f in block { f.l *= 0.5; f.r *= 0.5; }
    /// }).await
    /// ```
    pub async fn process<F: FnMut(&mut [StereoFrame])>(self, mut f: F) -> ! {
        // Prime the TX ring with dither, then anchor read/write heads.
        audio_block::prime_tx();
        let mut state = BlockState::new();
        let mut block = [StereoFrame::default(); audio_block::BLOCK_FRAMES];

        // Pace at the block period; the codec crystal is the effective master
        // (try_read_block skips on underrun / re-anchors on overrun), so OSTM vs
        // codec drift costs at most an occasional one-block glitch.
        let period_us =
            (audio_block::BLOCK_FRAMES as u64 * 1_000_000) / audio_block::SAMPLE_RATE_HZ as u64;
        let mut tick = Ticker::every(Duration::from_micros(period_us));

        loop {
            tick.next().await;
            if !state.try_read_block(&mut block) {
                continue; // not enough input yet — try next tick
            }
            f(&mut block);
            state.write_output_block(&block);
        }
    }
}
