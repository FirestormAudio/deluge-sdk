//! Block-oriented audio DSP plumbing for the SDK's `dlg.audio()`.
//!
//! **Ring-follow tap (v1).** Reads codec input behind the SSI RX DMA write head
//! and writes processed output ahead of the SCUX TX DMA read head, reusing the
//! existing free-running rings — no DMA/SCUX reconfiguration, so the
//! hardware-validated audio path ([`crate::audio`], [`crate::scux_dvu_path`],
//! [`rza1l_hal::ssi`]) is untouched; this module only *reads* their public
//! accessors. Mirrors the offset math + anti-auto-mute dither proven in the
//! firmware's `iso_out_to_ssi`.
//!
//! Board-specific concerns (i32↔f32 format, uncached ring access, wrap, dither)
//! live here; the SDK's `deluge::audio` owns the cadence and the user callback.

#![cfg(target_os = "none")]

use rza1l_hal::ssi;

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

/// Overrun margin: if available input exceeds `RX_FRAMES - GUARD_FRAMES` the
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

// Pure format conversion + dither live in `crate::sample_fmt` (host-tested);
// import them so the call sites below are unchanged.
use crate::sample_fmt::{dither_sample, f32_to_i32, i32_to_f32};

// ── DMA head offsets (slot = one i32; 2 slots per stereo frame) ────────────────

fn rx_head_off() -> usize {
    let start = ssi::rx_buf_start();
    let cur = ssi::rx_current_ptr();
    // SAFETY: both point into the same RX ring (uncached alias).
    (unsafe { cur.offset_from(start) } as usize) % ssi::RX_BUF_LEN
}

fn tx_head_off() -> usize {
    let start = ssi::tx_buf_start();
    let cur = ssi::tx_current_ptr();
    // CRSA can briefly read one-past-the-end at the DMA link-descriptor reload;
    // wrap into range so the lead calculation never sees a spurious value.
    (unsafe { cur.offset_from(start) } as usize) % ssi::TX_BUF_LEN
}

/// Prime the whole TX ring with dither so the codec hears no startup burst and
/// does not auto-mute before the first processed block lands.
pub fn prime_tx() {
    let start = ssi::tx_buf_start();
    let end = ssi::tx_buf_end();
    let mut lfsr: u32 = 0xACE1;
    let mut p = start;
    while p < end {
        // SAFETY: in-bounds walk of the uncached TX ring.
        unsafe {
            p.write_volatile(dither_sample(&mut lfsr));
            p = p.add(1);
        }
    }
}

/// Tracks the SDK's read/write positions within the two rings.
pub struct BlockState {
    /// Slot offset of our read head within the RX ring.
    rx_rd: usize,
    /// Slot offset of our write head within the TX ring.
    tx_wr: usize,
    lfsr: u32,
}

impl BlockState {
    /// Anchor at the current DMA heads: start reading where RX is now, and write
    /// `TX_WRITE_AHEAD_FRAMES` ahead of the TX read head (frame-aligned).
    pub fn new() -> Self {
        let tx_wr = ((tx_head_off() + TX_WRITE_AHEAD_FRAMES * 2) % ssi::TX_BUF_LEN) & !1;
        Self {
            rx_rd: rx_head_off() & !1,
            tx_wr,
            lfsr: 0x1,
        }
    }

    /// Frames of input available since our read head.
    fn rx_available_frames(&self) -> usize {
        ((rx_head_off() + ssi::RX_BUF_LEN - self.rx_rd) % ssi::RX_BUF_LEN) / 2
    }

    /// Fill `out` with the next input block, advancing the read head — unless not
    /// enough input has arrived yet (returns `false`; caller should skip this
    /// tick). Re-anchors if the RX DMA has lapped the read head.
    pub fn try_read_block(&mut self, out: &mut [Frame]) -> bool {
        let avail = self.rx_available_frames();
        if avail < out.len() {
            return false; // underrun: loop ran ahead of the codec crystal
        }
        if avail > ssi::RX_FRAMES - GUARD_FRAMES {
            // overrun: DMA lapped us — drop stale input, re-anchor one block back.
            self.rx_rd = ((rx_head_off() + ssi::RX_BUF_LEN - BLOCK_FRAMES * 2) % ssi::RX_BUF_LEN)
                & !1;
        }

        let base = ssi::rx_buf_start();
        let len = ssi::RX_BUF_LEN;
        let mut p = self.rx_rd;
        for fr in out.iter_mut() {
            // SAFETY: `p` is always in `[0, len)`; uncached alias is DMA-coherent.
            let l = unsafe { base.add(p).read_volatile() };
            p = (p + 1) % len;
            let r = unsafe { base.add(p).read_volatile() };
            p = (p + 1) % len;
            fr.l = i32_to_f32(l);
            fr.r = i32_to_f32(r);
        }
        self.rx_rd = p;
        true
    }

    /// Write `inp` to the TX ring ahead of the read head (f32→i32 saturating +
    /// dither), advancing the write head; re-anchors if the TX DMA caught up.
    pub fn write_output_block(&mut self, inp: &[Frame]) {
        let len = ssi::TX_BUF_LEN;
        let dma = tx_head_off();
        // Lead, in frames (2 i32 slots per frame).
        let ahead_frames = ((self.tx_wr + len - dma) % len) / 2;
        if ahead_frames < TX_WRITE_AHEAD_FRAMES / 2 {
            // TX DMA caught up — re-anchor the lead, frame-aligned.
            self.tx_wr = ((dma + TX_WRITE_AHEAD_FRAMES * 2) % len) & !1;
        }

        let base = ssi::tx_buf_start();
        let mut p = self.tx_wr;
        for fr in inp.iter() {
            let l = f32_to_i32(fr.l).saturating_add(dither_sample(&mut self.lfsr));
            let r = f32_to_i32(fr.r).saturating_add(dither_sample(&mut self.lfsr));
            // SAFETY: `p` is always in `[0, len)`; uncached alias is DMA-coherent.
            unsafe { base.add(p).write_volatile(l) };
            p = (p + 1) % len;
            unsafe { base.add(p).write_volatile(r) };
            p = (p + 1) % len;
        }
        self.tx_wr = p;
    }
}

impl Default for BlockState {
    fn default() -> Self {
        Self::new()
    }
}

/// Await the next RX block completion interrupt (v2 IRQ clock). Recurring; call
/// in a loop. Requires `init_block_irq` + `register_block_irq` (done by
/// `audio::init` under the `audio-irq` feature).
#[cfg(feature = "audio-irq")]
pub async fn wait_block() {
    rza1l_hal::dmac::wait_block(ssi::rx_dma_ch()).await;
}
