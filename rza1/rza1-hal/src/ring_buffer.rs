//! DMA rings lent by the caller: an embassy-style copy API and a positional window.
//!
//! A [`WritableRing`] feeds a peripheral (SSI TX, a SCUX FFD) and a [`ReadableRing`] is filled by
//! one (SSI RX, a SCUX FFU). The DMAC sweeps the ring forever, one block at a time; each block's
//! end raises an interrupt that counts it and wakes waiters.
//!
//! Two ways to use a ring:
//! - **The copy API**: [`WritableRing::write`] / [`ReadableRing::read`] (and their `_immediate`
//!   forms) keep a cursor of their own. A writer starts [`set_lead`](WritableRing::set_lead) words
//!   ahead of the DMAC; one the DMAC overtakes gets [`Error::Underrun`] once and starts again a lead
//!   ahead. A reader starts at the DMAC; one more than a ring behind gets [`Error::Overrun`] once
//!   and starts again at the DMAC.
//! - **The positional window**: [`position`](WritableRing::position) is the word the DMAC moves
//!   next, and [`ring_mut`](WritableRing::ring_mut) / [`ring`](ReadableRing::ring) is the whole ring,
//!   for a caller that writes N frames ahead of the hardware (or reads behind it) itself. Every
//!   window method is synchronous and lock-free, so an interrupt handler can own a ring and use it.
//!
//! The copy cursor doesn't see window writes: use one or the other on a ring.
//!
//! The ring is lent for `'d` and the DMAC runs over it until the ring is dropped, which halts the
//! channel first. Leaking a ring (`mem::forget`, reachable from safe code) leaves the DMAC running
//! over memory whose borrow has ended: the same gap as embassy's ring-buffered drivers. A
//! `'static` buffer, as a board crate usually lends, can't be freed under it.
//!
//! Each ring has one waker: wait on it from one task at a time (one [`BlockSignal`], or the copy
//! API's `write`/`read`), or the waiters steal each other's wake.

use crate::dma_buf::UncachedMut;
use crate::dmac::BlockDma;
use crate::dmac::ring::RingDma;

/// A copy-API error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum Error {
    /// The DMAC overtook the writer: it played words that were never written.
    Underrun,
    /// The DMAC lapped the reader: words were overwritten before they were read.
    Overrun,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::Underrun => "ring underrun",
            Error::Overrun => "ring overrun",
        })
    }
}

impl core::error::Error for Error {}

/// What a driver takes for one direction: a DMA channel with its block interrupt bound, the lent
/// ring, and how many blocks (each ending with an interrupt) to split it into.
pub struct RingBuf<'d> {
    /// The channel.
    pub dma: BlockDma<'d>,
    /// The ring: whole blocks of whole frames.
    pub buf: UncachedMut<'d, u32>,
    /// Blocks, 1 to 16.
    pub blocks: usize,
}

/// Waits for a ring's next block without borrowing the ring. One waiter per ring at a time: the
/// channel has one waker.
#[derive(Debug, Clone, Copy)]
pub struct BlockSignal {
    ch: u8,
}

impl BlockSignal {
    /// Wait for the next block to complete. Recurring: call it in a loop.
    pub async fn wait(&self) {
        crate::dmac::ring::wait_block_on(self.ch).await
    }
}

/// Copy `data` into `ring` from word `at`, wrapping.
fn copy_in(ring: &mut [u32], at: usize, data: &[u32]) {
    let first = data.len().min(ring.len() - at);
    ring[at..at + first].copy_from_slice(&data[..first]);
    ring[..data.len() - first].copy_from_slice(&data[first..]);
}

/// Copy from `ring` at word `at` into `out`, wrapping.
fn copy_out(ring: &[u32], at: usize, out: &mut [u32]) {
    let first = out.len().min(ring.len() - at);
    let (a, b) = out.split_at_mut(first);
    a.copy_from_slice(&ring[at..at + first]);
    b.copy_from_slice(&ring[..b.len()]);
}

/// A ring the DMAC feeds a peripheral from.
pub struct WritableRing<'d> {
    ring: RingDma<'d>,
    cursor: Option<u64>,
    lead: usize,
}

impl<'d> WritableRing<'d> {
    pub(crate) fn new(ring: RingDma<'d>) -> Self {
        let lead = ring.len() / 2 - (ring.len() / 2) % ring.frame_words();
        Self { ring, cursor: None, lead }
    }

    /// The ring's length in words.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Words per frame.
    pub fn frame_words(&self) -> usize {
        self.ring.frame_words()
    }

    /// The word the DMAC moves next, on a frame boundary: the window's play position.
    pub fn position(&self) -> usize {
        self.ring.position()
    }

    /// The whole ring: the window. Words just ahead of [`position`](Self::position) play soonest.
    pub fn ring_mut(&mut self) -> &mut [u32] {
        self.ring.buf_mut()
    }

    /// Waits for this ring's blocks without borrowing it.
    pub fn block_signal(&self) -> BlockSignal {
        BlockSignal { ch: self.ring.channel() }
    }

    /// How far ahead of the DMAC the copy API starts writing, and starts again after an
    /// underrun: `words`, rounded down to a frame. Half the ring by default.
    ///
    /// # Panics
    /// If `words` isn't less than the ring.
    pub fn set_lead(&mut self, words: usize) {
        assert!(words < self.len(), "ring lead of {} words in a {}-word ring", words, self.len());
        self.lead = words - words % self.frame_words();
        self.cursor = None;
    }

    /// Write as many whole frames of `data` as fit now, and return how many words that was (0
    /// when the ring is full). [`Error::Underrun`] once if the DMAC overtook the writer.
    pub fn write_immediate(&mut self, data: &[u32]) -> Result<usize, Error> {
        let hw = self.ring.absolute();
        let cursor = match self.cursor {
            None => hw + self.lead as u64,
            Some(c) if c < hw => {
                self.cursor = Some(hw + self.lead as u64);
                return Err(Error::Underrun);
            }
            Some(c) => c,
        };
        let len = self.len();
        let free = len - (cursor - hw) as usize;
        let n = free.min(data.len());
        let n = n - n % self.frame_words();
        copy_in(self.ring.buf_mut(), (cursor % len as u64) as usize, &data[..n]);
        self.cursor = Some(cursor + n as u64);
        Ok(n)
    }

    /// Write whole frames of `data`, waiting for room for at least one; returns the words
    /// written. [`Error::Underrun`] once if the DMAC overtook the writer.
    pub async fn write(&mut self, data: &[u32]) -> Result<usize, Error> {
        loop {
            match self.write_immediate(data)? {
                0 if data.len() >= self.frame_words() => self.ring.wait_block().await,
                n => return Ok(n),
            }
        }
    }
}

/// A ring a peripheral fills through the DMAC.
pub struct ReadableRing<'d> {
    ring: RingDma<'d>,
    cursor: Option<u64>,
}

impl<'d> ReadableRing<'d> {
    pub(crate) fn new(ring: RingDma<'d>) -> Self {
        Self { ring, cursor: None }
    }

    /// The ring's length in words.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Words per frame.
    pub fn frame_words(&self) -> usize {
        self.ring.frame_words()
    }

    /// The word the DMAC fills next, on a frame boundary: everything before it has arrived.
    pub fn position(&self) -> usize {
        self.ring.position()
    }

    /// The whole ring: the window. Words just behind [`position`](Self::position) are newest.
    pub fn ring(&self) -> &[u32] {
        self.ring.buf()
    }

    /// Waits for this ring's blocks without borrowing it.
    pub fn block_signal(&self) -> BlockSignal {
        BlockSignal { ch: self.ring.channel() }
    }

    /// Copy as many whole frames as have arrived into `out` and return how many words that was.
    /// The first call starts at the DMAC, so it returns 0. [`Error::Overrun`] once if the DMAC
    /// lapped the reader, or is about to (less than a frame of margin).
    pub fn read_immediate(&mut self, out: &mut [u32]) -> Result<usize, Error> {
        let hw = self.ring.absolute();
        let len = self.len();
        let cursor = match self.cursor {
            None => {
                self.cursor = Some(hw);
                return Ok(0);
            }
            // A whole ring behind, the oldest frame is being overwritten as it's copied.
            Some(c) if hw - c > (len - self.frame_words()) as u64 => {
                self.cursor = Some(hw);
                return Err(Error::Overrun);
            }
            Some(c) => c,
        };
        let n = ((hw - cursor) as usize).min(out.len());
        let n = n - n % self.frame_words();
        copy_out(self.ring.buf(), (cursor % len as u64) as usize, &mut out[..n]);
        self.cursor = Some(cursor + n as u64);
        Ok(n)
    }

    /// Wait until at least one frame has arrived, then copy what has into `out`. [`Error::Overrun`]
    /// once if the DMAC lapped the reader.
    pub async fn read(&mut self, out: &mut [u32]) -> Result<usize, Error> {
        loop {
            match self.read_immediate(out)? {
                0 if out.len() >= self.frame_words() => self.ring.wait_block().await,
                n => return Ok(n),
            }
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::dmac::BlockInterruptHandler;
    use crate::dmac::ring::{Dir, RingDma};
    use crate::interrupt::Handler;
    use crate::mmio::test;
    use crate::peripherals::DMA_CH9;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    crate::bind_interrupts!(struct Irqs {
        DMAINT9 => BlockInterruptHandler<DMA_CH9>;
    });

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A 64-word, 4-block ring of 2-word frames on channel 9, and its first word's bus address.
    fn raw_ring(dir: Dir) -> (RingDma<'static>, u32) {
        test::reset();
        let buf = UncachedMut::new(std::vec![0u32; 64].leak()).unwrap();
        let base = buf.as_slice().as_ptr() as u32;
        let dma = BlockDma::new(unsafe { DMA_CH9::steal() }, Irqs);
        (RingDma::start(dma, buf, 4, 2, 0x8000_0001, dir, 0xE1), base)
    }

    fn writable() -> (WritableRing<'static>, u32) {
        let (r, base) = raw_ring(Dir::MemToPeriph { dst: 0xE820_B018 });
        (WritableRing::new(r), base)
    }

    fn readable() -> (ReadableRing<'static>, u32) {
        let (r, base) = raw_ring(Dir::PeriphToMem { src: 0xE820_B01C });
        (ReadableRing::new(r), base)
    }

    /// The DMAC at word `w` of the ring, after `blocks` counted block ends.
    fn dma_at(base: u32, w: u32, blocks: u32) {
        for _ in 0..blocks {
            unsafe { <BlockInterruptHandler<DMA_CH9> as Handler<_>>::on_interrupt() };
        }
        test::poke32(crate::pac::DMAC.ch(9).crsa().addr(), base + 4 * w);
        test::poke32(crate::pac::DMAC.ch(9).crda().addr(), base + 4 * w);
    }

    #[test]
    fn the_first_write_lands_lead_words_ahead_of_the_dma() {
        let _serial = serial();
        let (mut tx, base) = writable();
        tx.set_lead(16);
        dma_at(base, 4, 0);
        assert_eq!(tx.write_immediate(&[1, 2, 3, 4]), Ok(4));
        assert_eq!(&tx.ring_mut()[20..24], &[1, 2, 3, 4]);
    }

    #[test]
    fn a_full_ring_takes_nothing_until_the_dma_moves() {
        let _serial = serial();
        let (mut tx, base) = writable();
        tx.set_lead(16);
        dma_at(base, 0, 0);
        assert_eq!(tx.write_immediate(&[7; 64]), Ok(48));
        assert_eq!(tx.write_immediate(&[7; 64]), Ok(0));
        dma_at(base, 16, 1);
        assert_eq!(tx.write_immediate(&[7; 64]), Ok(16));
    }

    #[test]
    fn a_writer_lapped_by_the_dma_reports_underrun_once() {
        let _serial = serial();
        let (mut tx, base) = writable();
        tx.set_lead(16);
        dma_at(base, 0, 0);
        assert_eq!(tx.write_immediate(&[1, 2, 3, 4]), Ok(4));
        dma_at(base, 16, 5); // absolute 80: past the cursor (20)
        assert_eq!(tx.write_immediate(&[9, 9]), Err(Error::Underrun));
        assert_eq!(tx.write_immediate(&[9, 9]), Ok(2));
        assert_eq!(&tx.ring_mut()[32..34], &[9, 9], "lead (16) ahead of 80, mod 64");
    }

    #[test]
    fn writes_are_whole_frames() {
        let _serial = serial();
        let (mut tx, base) = writable();
        dma_at(base, 0, 0);
        assert_eq!(tx.write_immediate(&[1, 2, 3]), Ok(2));
    }

    #[test]
    fn a_reader_gets_what_the_dma_wrote_and_overrun_after_a_lap() {
        let _serial = serial();
        let (mut rx, base) = readable();
        dma_at(base, 0, 0);
        let mut out = [0u32; 8];
        assert_eq!(rx.read_immediate(&mut out), Ok(0), "anchored at the DMAC");
        let ring = unsafe { core::slice::from_raw_parts_mut(base as *mut u32, 64) };
        ring[..6].copy_from_slice(&[10, 11, 12, 13, 14, 15]);
        dma_at(base, 6, 0);
        assert_eq!(rx.read_immediate(&mut out), Ok(6));
        assert_eq!(&out[..6], &[10, 11, 12, 13, 14, 15]);
        dma_at(base, 16, 5); // absolute 80: more than a ring past the cursor (6)
        assert_eq!(rx.read_immediate(&mut out), Err(Error::Overrun));
        assert_eq!(rx.read_immediate(&mut out), Ok(0));
    }

    #[test]
    fn a_reader_lapped_by_the_dma_reports_overrun_once() {
        let _serial = serial();
        let (mut rx, base) = readable();
        dma_at(base, 0, 0);
        let mut out = [0u32; 8];
        assert_eq!(rx.read_immediate(&mut out), Ok(0));
        dma_at(base, 16, 5);
        assert_eq!(rx.read_immediate(&mut out), Err(Error::Overrun));
        dma_at(base, 20, 0);
        assert_eq!(rx.read_immediate(&mut out), Ok(4));
    }

    /// A reader exactly a ring behind would copy its oldest frame while the DMAC overwrites it.
    #[test]
    fn a_reader_a_whole_ring_behind_reports_overrun() {
        let _serial = serial();
        let (mut rx, base) = readable();
        dma_at(base, 0, 0);
        let mut out = [0u32; 8];
        assert_eq!(rx.read_immediate(&mut out), Ok(0));
        dma_at(base, 0, 4); // absolute 64: exactly one ring
        assert_eq!(rx.read_immediate(&mut out), Err(Error::Overrun));
    }

    #[test]
    fn async_write_waits_for_a_block() {
        let _serial = serial();
        let (mut tx, base) = writable();
        tx.set_lead(16);
        dma_at(base, 0, 0);
        assert_eq!(tx.write_immediate(&[7; 64]), Ok(48));
        let count = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(count.clone());
        let mut cx = Context::from_waker(&waker);
        let data = [5u32; 64];
        let mut fut = core::pin::pin!(tx.write(&data));
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        dma_at(base, 16, 1);
        assert!(count.0.load(Ordering::SeqCst) >= 1, "the block end woke the writer");
        assert_eq!(fut.as_mut().poll(&mut cx), Poll::Ready(Ok(16)));
    }

    #[test]
    fn the_window_shows_the_dma_position() {
        let _serial = serial();
        let (tx, base) = writable();
        dma_at(base, 10, 0);
        assert_eq!(tx.position(), 10);
        assert_eq!(tx.len(), 64);
        assert_eq!(tx.frame_words(), 2);
    }

    #[test]
    fn rings_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<WritableRing<'static>>();
        assert_send::<ReadableRing<'static>>();
        assert_send::<BlockSignal>();
    }
}
