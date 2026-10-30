//! Block-oriented audio DSP plumbing for the SDK's `dlg.audio()`.
//!
//! **Ring-follow tap.** Reads codec input behind the SSI RX DMA write head and
//! writes processed output ahead of the SCUX TX DMA read head, reusing the
//! existing free-running rings. There is no DMA/SCUX reconfiguration: this
//! module only reaches the rings through [`crate::audio::with_tx`] / [`crate::audio::with_rx`].
//! The offset
//! math and anti-auto-mute dither match the firmware's `SpeakerSink`.
//!
//! Board-specific concerns (i32↔f32 format, uncached ring access, wrap, dither)
//! live here; the SDK's `deluge::audio` owns the cadence and the user callback.

#![cfg(target_os = "none")]

/// Codec sample rate.
pub const SAMPLE_RATE_HZ: u32 = 44_100;

/// Stereo frames processed per callback. 128 ≈ 2.9 ms at 44.1 kHz.
pub const BLOCK_FRAMES: usize = 128;

/// How far ahead of the TX DMA read head we keep the write head, in frames.
/// ~11.6 ms — absorbs cadence jitter / SCUX FFD burst DMA. (Round-trip latency
/// ≈ this + one block.)
#[cfg(not(feature = "audio-irq"))]
const TX_WRITE_AHEAD_FRAMES: usize = 512;
/// Tighter lead for the IRQ clock (codec-locked, so less jitter to absorb).
#[cfg(feature = "audio-irq")]
const TX_WRITE_AHEAD_FRAMES: usize = 256;

/// Overrun margin: if available input exceeds the ring's frames minus `GUARD_FRAMES`, the
/// read head has been lapped by the DMA, so re-anchor.
const GUARD_FRAMES: usize = BLOCK_FRAMES;

/// One stereo audio frame; samples in `[-1.0, 1.0]`. Re-exported by the SDK as
/// `deluge::StereoFrame`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Frame {
    pub l: f32,
    pub r: f32,
}

// Pure format conversion + dither live in `crate::sample_fmt` (host-tested).
#[cfg(feature = "audio-irq")]
use crate::audio::rx_block_signal;
use crate::audio::{with_rx, with_tx};
use crate::sample_fmt::{dither_sample, f32_to_i32, i32_to_f32};

/// Fill the whole transmit ring with dither, so the codec never sees a run of identical samples
/// (it would auto-mute). Nothing before `audio::init`.
pub fn prime_tx() {
    crate::audio::fill_tx_with_dither();
}

/// The block loop's ring positions, in words (two per stereo frame).
pub struct BlockState {
    rx_rd: usize,
    tx_wr: usize,
    lfsr: u32,
}

impl BlockState {
    /// Read from the receive DMA's position; write [`TX_WRITE_AHEAD_FRAMES`] ahead of the
    /// transmit DMA's.
    pub fn new() -> Self {
        let tx_wr = with_tx(|tx| ((tx.position() + TX_WRITE_AHEAD_FRAMES * 2) % tx.len()) & !1)
            .unwrap_or(0);
        let rx_rd = with_rx(|rx| rx.position() & !1).unwrap_or(0);
        Self {
            rx_rd,
            tx_wr,
            lfsr: 0x1,
        }
    }

    /// Fill `out` with the next received frames, if that many have arrived; `false` (and nothing
    /// read) otherwise. More than a ring's worth behind skips ahead to the newest block.
    pub fn try_read_block(&mut self, out: &mut [Frame]) -> bool {
        let rx_rd = &mut self.rx_rd;
        with_rx(|rx| {
            let len = rx.len();
            let head = rx.position();
            let avail = ((head + len - *rx_rd) % len) / 2;
            if avail < out.len() {
                return false; // underrun: the loop ran ahead of the codec crystal
            }
            if avail > len / 2 - GUARD_FRAMES {
                *rx_rd = ((head + len - BLOCK_FRAMES * 2) % len) & !1;
            }
            let ring = rx.ring();
            let mut p = *rx_rd;
            for fr in out.iter_mut() {
                fr.l = i32_to_f32(ring[p] as i32);
                p = (p + 1) % len;
                fr.r = i32_to_f32(ring[p] as i32);
                p = (p + 1) % len;
            }
            *rx_rd = p;
            true
        })
        .unwrap_or(false)
    }

    /// Write `inp` (with dither) [`TX_WRITE_AHEAD_FRAMES`] ahead of the transmit DMA,
    /// re-anchoring if the DMA has caught up to within half that.
    pub fn write_output_block(&mut self, inp: &[Frame]) {
        let (tx_wr, lfsr) = (&mut self.tx_wr, &mut self.lfsr);
        with_tx(|tx| {
            let len = tx.len();
            let dma = tx.position();
            let ahead_frames = ((*tx_wr + len - dma) % len) / 2;
            if ahead_frames < TX_WRITE_AHEAD_FRAMES / 2 {
                *tx_wr = ((dma + TX_WRITE_AHEAD_FRAMES * 2) % len) & !1;
            }
            let ring = tx.ring_mut();
            let mut p = *tx_wr;
            for fr in inp.iter() {
                ring[p] = f32_to_i32(fr.l).saturating_add(dither_sample(lfsr)) as u32;
                p = (p + 1) % len;
                ring[p] = f32_to_i32(fr.r).saturating_add(dither_sample(lfsr)) as u32;
                p = (p + 1) % len;
            }
            *tx_wr = p;
        });
    }
}

impl Default for BlockState {
    fn default() -> Self {
        Self::new()
    }
}

/// Wait for the receive ring's next 128-frame block.
///
/// # Panics
/// Before `audio::init`.
#[cfg(feature = "audio-irq")]
pub async fn wait_block() {
    rx_block_signal()
        .expect("audio::init hasn't run")
        .wait()
        .await;
}
