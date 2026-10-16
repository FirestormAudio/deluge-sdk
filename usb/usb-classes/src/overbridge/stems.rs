//! The app-facing side of an Overbridge stream: a pair of interleaved full-scale `i32` rings
//! behind one lock, the device's identity, and the stream's statistics.
//!
//! The stream task pushes what it receives ([`OverbridgeHandle::push_rx`]) and pulls what it
//! sends ([`OverbridgeHandle::pull_tx`]); the app [`read`](OverbridgeHandle::read)s and
//! [`write`](OverbridgeHandle::write)s. Neither side blocks: an overrun drops the oldest frames
//! and an underrun sends silence, each counted in [`StreamStats`].

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use super::proto::Format;

/// The connected device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverbridgeInfo {
    pub pid: u16,
    pub name: &'static str,
    /// Channels the Overbridge device sends.
    pub ch_from: u8,
    /// Channels the Overbridge device receives.
    pub ch_to: u8,
    pub format: Format,
}

/// Stream counters. "In" is what arrives at this side (for a host, from the device).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Frames waiting for the app to read.
    pub fill_in: u32,
    /// Frames the app has written and the stream hasn't sent yet.
    pub fill_out: u32,
    pub blocks_in: u32,
    pub blocks_out: u32,
    /// Blocks missing by the +7 frame counter.
    pub counter_gaps: u32,
    /// Blocks dropped for a wrong header or length.
    pub framing_errors: u32,
    /// Received frames dropped because the app didn't read them in time.
    pub overruns: u32,
    /// Sent frames padded with silence because the app hadn't written them.
    pub underruns: u32,
    /// The device's frame rate against local time, in parts per million off nominal.
    pub rate_ratio_ppm: i32,
}

/// The device has more channels than this handle's rings hold [`MIN_RING_FRAMES`] of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyChannels;

/// The fewest frames a ring must hold for a device to be accepted (8 blocks).
pub const MIN_RING_FRAMES: usize = 56;

/// Interleaved ring of `N` samples, holding `N / channels` frames.
struct Ring<const N: usize> {
    buf: [i32; N],
    head: usize,
    frames: usize,
    channels: usize,
}

impl<const N: usize> Ring<N> {
    const fn new() -> Self {
        Self {
            buf: [0; N],
            head: 0,
            frames: 0,
            channels: 0,
        }
    }

    fn capacity(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            N / self.channels
        }
    }

    fn reset(&mut self, channels: usize) {
        self.head = 0;
        self.frames = 0;
        self.channels = channels;
    }

    /// Append whole frames; returns how many of the oldest were dropped to make room.
    fn push(&mut self, samples: &[i32]) -> usize {
        let (ch, cap) = (self.channels, self.capacity());
        if cap == 0 {
            return 0;
        }
        let mut dropped = 0;
        for frame in samples.chunks_exact(ch) {
            if self.frames == cap {
                self.head = (self.head + 1) % cap;
                self.frames -= 1;
                dropped += 1;
            }
            let at = (self.head + self.frames) % cap * ch;
            self.buf[at..at + ch].copy_from_slice(frame);
            self.frames += 1;
        }
        dropped
    }

    /// Append whole frames while there is room; returns how many were taken.
    fn push_some(&mut self, samples: &[i32]) -> usize {
        let room = self.capacity() - self.frames;
        let take = (samples.len() / self.channels.max(1)).min(room);
        self.push(&samples[..take * self.channels]);
        take
    }

    /// Move up to `out.len() / channels` frames out; returns how many.
    fn pop(&mut self, out: &mut [i32]) -> usize {
        let (ch, cap) = (self.channels, self.capacity());
        if cap == 0 {
            return 0;
        }
        let n = (out.len() / ch).min(self.frames);
        for f in 0..n {
            let at = (self.head + f) % cap * ch;
            out[f * ch..(f + 1) * ch].copy_from_slice(&self.buf[at..at + ch]);
        }
        self.head = (self.head + n) % cap;
        self.frames -= n;
        n
    }
}

struct Inner<const RX: usize, const TX: usize> {
    info: Option<OverbridgeInfo>,
    rx: Ring<RX>,
    tx: Ring<TX>,
    stats: StreamStats,
    connection: u32,
}

/// One Overbridge stream, shared by its stream task and the app. `RX` and `TX` are the ring
/// sizes in samples; a ring holds `RX / channels` frames.
pub struct OverbridgeHandle<const RX: usize, const TX: usize> {
    inner: Mutex<CriticalSectionRawMutex, RefCell<Inner<RX, TX>>>,
}

impl<const RX: usize, const TX: usize> OverbridgeHandle<RX, TX> {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(Inner {
                info: None,
                rx: Ring::new(),
                tx: Ring::new(),
                stats: StreamStats {
                    fill_in: 0,
                    fill_out: 0,
                    blocks_in: 0,
                    blocks_out: 0,
                    counter_gaps: 0,
                    framing_errors: 0,
                    overruns: 0,
                    underruns: 0,
                    rate_ratio_ppm: 0,
                },
                connection: 0,
            })),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner<RX, TX>) -> R) -> R {
        self.inner.lock(|i| f(&mut i.borrow_mut()))
    }

    /// The connected device, or `None`.
    pub fn info(&self) -> Option<OverbridgeInfo> {
        self.with(|i| i.info)
    }

    /// Copy up to `out.len() / ch_from` received frames, interleaved. Returns frames copied.
    pub fn read(&self, out: &mut [i32]) -> usize {
        self.with(|i| i.rx.pop(out))
    }

    /// Queue frames to send (interleaved, `ch_to` wide). Returns frames taken; the rest didn't
    /// fit, or nothing is connected.
    pub fn write(&self, inp: &[i32]) -> usize {
        self.with(|i| {
            if i.info.is_some() {
                i.tx.push_some(inp)
            } else {
                0
            }
        })
    }

    /// The stream's counters, with current fill levels.
    pub fn stats(&self) -> StreamStats {
        self.with(|i| StreamStats {
            fill_in: i.rx.frames as u32,
            fill_out: i.tx.frames as u32,
            ..i.stats
        })
    }

    /// Counts connections, so an app can tell the device (or driver) changed between reads.
    pub fn connection(&self) -> u32 {
        self.with(|i| i.connection)
    }

    /// Driver side: a device is connected, with `rx_ch` channels arriving and `tx_ch` sent.
    pub fn begin(
        &self,
        info: OverbridgeInfo,
        rx_ch: usize,
        tx_ch: usize,
    ) -> Result<(), TooManyChannels> {
        if rx_ch == 0 || tx_ch == 0 || RX / rx_ch < MIN_RING_FRAMES || TX / tx_ch < MIN_RING_FRAMES
        {
            return Err(TooManyChannels);
        }
        self.with(|i| {
            i.rx.reset(rx_ch);
            i.tx.reset(tx_ch);
            i.stats = StreamStats::default();
            i.info = Some(info);
            i.connection = i.connection.wrapping_add(1);
        });
        Ok(())
    }

    /// Driver side: the device is gone.
    pub fn end(&self) {
        self.with(|i| {
            i.info = None;
            i.rx.reset(0);
            i.tx.reset(0);
        });
    }

    /// Driver side: frames received (interleaved, `rx_ch` wide).
    pub fn push_rx(&self, frames: &[i32]) {
        self.with(|i| {
            let dropped = i.rx.push(frames) as u32;
            i.stats.overruns = i.stats.overruns.saturating_add(dropped);
        });
    }

    /// Driver side: fill `frames` (interleaved, `tx_ch` wide) with what to send, padding with
    /// silence when the app hasn't written enough.
    pub fn pull_tx(&self, frames: &mut [i32]) {
        self.with(|i| {
            let ch = i.tx.channels.max(1);
            let got = i.tx.pop(frames);
            frames[got * ch..].fill(0);
            let short = (frames.len() / ch - got) as u32;
            i.stats.underruns = i.stats.underruns.saturating_add(short);
        });
    }

    /// Driver side: update the counters.
    pub fn update_stats(&self, f: impl FnOnce(&mut StreamStats)) {
        self.with(|i| f(&mut i.stats));
    }
}

impl<const RX: usize, const TX: usize> Default for OverbridgeHandle<RX, TX> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::overbridge::proto::Format;

    fn info(ch_from: u8, ch_to: u8) -> OverbridgeInfo {
        OverbridgeInfo {
            pid: 0x0B2C,
            name: "Digitakt",
            ch_from,
            ch_to,
            format: Format::V2,
        }
    }

    #[test]
    fn nothing_is_connected_until_begin() {
        let h: OverbridgeHandle<1024, 256> = OverbridgeHandle::new();
        assert_eq!(h.info(), None);
        assert_eq!(h.read(&mut [0; 24]), 0);
        assert_eq!(h.write(&[1, 2]), 0, "nothing to write to");
        h.begin(info(12, 2), 12, 2).unwrap();
        assert_eq!(h.info(), Some(info(12, 2)));
        assert_eq!(h.connection(), 1);
        h.end();
        assert_eq!(h.info(), None);
    }

    #[test]
    fn received_frames_are_read_back_in_order() {
        let h: OverbridgeHandle<1200, 256> = OverbridgeHandle::new();
        h.begin(info(12, 2), 12, 2).unwrap();
        let block: std::vec::Vec<i32> = (0..7 * 12).collect();
        h.push_rx(&block);
        let mut out = [0i32; 5 * 12];
        assert_eq!(h.read(&mut out), 5, "frames, not samples");
        assert_eq!(&out[..], &block[..5 * 12]);
        assert_eq!(h.stats().fill_in, 2);
        let mut rest = [0i32; 7 * 12];
        assert_eq!(h.read(&mut rest), 2);
        assert_eq!(&rest[..2 * 12], &block[5 * 12..]);
    }

    #[test]
    fn an_overrun_drops_the_oldest_frames_and_counts_them() {
        // 120 samples hold 10 frames of 12 channels... too few for begin; use 2 channels.
        let h: OverbridgeHandle<128, 128> = OverbridgeHandle::new();
        h.begin(info(2, 2), 2, 2).unwrap(); // 64 frames of 2 channels
        for b in 0..10 {
            let block: std::vec::Vec<i32> = (0..14).map(|i| b * 100 + i).collect();
            h.push_rx(&block);
        }
        let s = h.stats();
        assert_eq!(s.overruns, 70 - 64);
        assert_eq!(s.fill_in, 64);
        let mut out = [0i32; 2];
        h.read(&mut out);
        assert_eq!(out, [12, 13], "frames 0-5 of block 0 were dropped");
    }

    #[test]
    fn the_tx_side_pads_silence_and_counts_underruns() {
        let h: OverbridgeHandle<1024, 256> = OverbridgeHandle::new();
        h.begin(info(12, 2), 12, 2).unwrap();
        assert_eq!(h.write(&[1, 2, 3, 4, 5, 6]), 3);
        let mut block = [9i32; 14];
        h.pull_tx(&mut block);
        assert_eq!(&block[..6], &[1, 2, 3, 4, 5, 6]);
        assert!(block[6..].iter().all(|&s| s == 0));
        assert_eq!(h.stats().underruns, 4);
    }

    #[test]
    fn a_handle_too_small_is_refused() {
        // 42 channels × 56 frames = 2352 samples; 2048 is too few.
        let h: OverbridgeHandle<2048, 512> = OverbridgeHandle::new();
        assert_eq!(h.begin(info(42, 8), 42, 8), Err(TooManyChannels));
        assert_eq!(h.info(), None, "a refused device is not connected");
    }

    #[test]
    fn begin_resets_rings_and_stats_and_counts_connections() {
        let h: OverbridgeHandle<1024, 256> = OverbridgeHandle::new();
        h.begin(info(2, 2), 2, 2).unwrap();
        h.push_rx(&[1; 14]);
        h.update_stats(|s| s.counter_gaps = 5);
        h.begin(info(2, 2), 2, 2).unwrap();
        assert_eq!(h.stats(), StreamStats::default());
        assert_eq!(h.read(&mut [0; 2]), 0);
        assert_eq!(h.connection(), 2);
    }
}
