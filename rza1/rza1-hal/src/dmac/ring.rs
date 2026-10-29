//! Circular DMA over a lent ring, split into blocks.
//!
//! A [`RingDma`] runs one link descriptor per block, each linked to the next and the last back to
//! the first, so the DMAC sweeps the ring forever. Every descriptor ends with an interrupt
//! ([`BlockInterruptHandler`](super::BlockInterruptHandler)), which counts the block; the count
//! and the DMAC's current address give [`RingDma::absolute`], the words moved since the start.
//!
//! The descriptors live in a per-channel table here: DMA-controller state, not the caller's ring.

use core::sync::atomic::{AtomicU32, Ordering};

use super::BlockDma;
use crate::dma_buf::UncachedMut;

/// The most blocks a ring can have.
pub(crate) const MAX_RING_BLOCKS: usize = 16;

/// Link descriptor header: LV (valid), LE (the next is a descriptor), WBD (write back), NXA.
const DESC_HEADER: u32 = 0b1101;

/// One 8-word link descriptor.
#[repr(C, align(32))]
struct LinkDesc([u32; 8]);

/// Channel n's descriptor table. SAFETY: written only by the [`RingDma::start`] of the channel's
/// owner, through the uncached alias the DMAC reads it by.
static mut RING_DESC: [[LinkDesc; MAX_RING_BLOCKS]; 16] =
    [const { [const { LinkDesc([0; 8]) }; MAX_RING_BLOCKS] }; 16];

/// Blocks channel n has completed since its ring started: bumped by the block interrupt.
pub(crate) static BLOCKS_DONE: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];

/// Which way a ring moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dir {
    /// The ring feeds the peripheral register `dst`.
    MemToPeriph { dst: u32 },
    /// The peripheral register `src` fills the ring.
    PeriphToMem { src: u32 },
}

/// A channel sweeping a lent ring in `blocks` blocks, with an absolute position. Dropping it
/// halts the channel before the ring's borrow ends.
pub(crate) struct RingDma<'d> {
    dma: BlockDma<'d>,
    buf: UncachedMut<'d, u32>,
    blocks: usize,
    frame_words: usize,
    dir: Dir,
}

impl<'d> RingDma<'d> {
    /// Program `blocks` descriptors over `buf` with `chcfg` (DEM clear, the channel's index in
    /// bits 2:0) and request `dmars`, then start the channel.
    ///
    /// # Panics
    /// If `blocks` is 0 or more than [`MAX_RING_BLOCKS`], or `buf` isn't whole blocks of whole
    /// `frame_words`-word frames.
    pub(crate) fn start(
        mut dma: BlockDma<'d>,
        buf: UncachedMut<'d, u32>,
        blocks: usize,
        frame_words: usize,
        chcfg: u32,
        dir: Dir,
        dmars: u32,
    ) -> Self {
        assert!(
            (1..=MAX_RING_BLOCKS).contains(&blocks),
            "dmac ring: {} blocks (1 to {})",
            blocks,
            MAX_RING_BLOCKS
        );
        assert!(
            frame_words > 0 && buf.len() % (blocks * frame_words) == 0,
            "dmac ring: {} words isn't {} blocks of {}-word frames",
            buf.len(),
            blocks,
            frame_words
        );
        let ch = dma.number() as usize;
        let block_bytes = (buf.len() / blocks * 4) as u32;
        let base = buf.bus_addr();
        // SAFETY: channel ch's table, ch owned through `dma`; written through the uncached alias
        // the DMAC fetches descriptors by.
        let head = unsafe {
            let desc = |i: usize| {
                crate::uncached(core::ptr::addr_of_mut!(RING_DESC[ch][i]) as *mut u32)
            };
            for i in 0..blocks {
                let d = desc(i);
                let mem = base + i as u32 * block_bytes;
                let (src, dst) = match dir {
                    Dir::MemToPeriph { dst } => (mem, dst),
                    Dir::PeriphToMem { src } => (src, mem),
                };
                d.add(0).write_volatile(DESC_HEADER);
                d.add(1).write_volatile(src);
                d.add(2).write_volatile(dst);
                d.add(3).write_volatile(block_bytes);
                d.add(4).write_volatile(chcfg);
                d.add(5).write_volatile(0);
                d.add(6).write_volatile(0);
                d.add(7).write_volatile(desc((i + 1) % blocks) as u32);
            }
            desc(0) as *const u32
        };
        BLOCKS_DONE[ch].store(0, Ordering::Release);
        // SAFETY: the descriptors above describe `buf`, which this ring holds until it halts.
        unsafe {
            dma.init_with_link_descriptor(head, dmars);
            dma.channel_start();
        }
        Self { dma, buf, blocks, frame_words, dir }
    }

    /// The ring's length in words.
    pub(crate) fn len(&self) -> usize {
        self.buf.len()
    }

    /// Words per frame.
    pub(crate) fn frame_words(&self) -> usize {
        self.frame_words
    }

    /// The word the DMAC moves next, rounded down to a frame. CRSA/CRDA can read one past the
    /// ring's end while the next descriptor loads, which wraps to 0.
    pub(crate) fn position(&self) -> usize {
        let addr = match self.dir {
            Dir::MemToPeriph { .. } => self.dma.current_src(),
            Dir::PeriphToMem { .. } => self.dma.current_dst(),
        };
        let word = (addr.wrapping_sub(self.buf.bus_addr()) / 4) as usize % self.len();
        word - word % self.frame_words
    }

    /// Words moved since the ring started, never decreasing. The block count lags the DMAC by
    /// the block interrupt's latency, so a position in the block after the counted one counts
    /// that block too: right while the latency stays under one block.
    pub(crate) fn absolute(&self) -> u64 {
        let ch = self.dma.number() as usize;
        let block_words = self.len() / self.blocks;
        loop {
            let c1 = BLOCKS_DONE[ch].load(Ordering::Acquire);
            let p = self.position();
            let c2 = BLOCKS_DONE[ch].load(Ordering::Acquire);
            if c1 != c2 {
                continue;
            }
            let in_block = (p / block_words) as u32;
            let counted = if in_block == c1 % self.blocks as u32 { c1 } else { c1.wrapping_add(1) };
            return counted as u64 * block_words as u64 + (p % block_words) as u64;
        }
    }

    /// Wait for the next block to complete.
    pub(crate) async fn wait_block(&self) {
        self.dma.wait_block().await
    }

    /// The DMAC channel.
    pub(crate) fn channel(&self) -> u8 {
        self.dma.number()
    }

    /// The ring.
    pub(crate) fn buf(&self) -> &[u32] {
        self.buf.as_slice()
    }

    /// The ring, writable.
    pub(crate) fn buf_mut(&mut self) -> &mut [u32] {
        self.buf.as_mut_slice()
    }
}

/// Wait for the next block on channel `ch`'s ring, without its [`RingDma`]: a task waits while
/// an interrupt owns the ring.
pub(crate) async fn wait_block_on(ch: u8) {
    super::raw::wait_block(ch).await
}

impl Drop for RingDma<'_> {
    fn drop(&mut self) {
        if !super::halt_channel(self.dma.number()) {
            warn!("dmac ring: ch{} still active after the halt", self.dma.number());
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::dmac::BlockInterruptHandler;
    use crate::interrupt::Handler;
    use crate::mmio::test;
    use crate::peripherals::DMA_CH11;

    crate::bind_interrupts!(struct Irqs {
        DMAINT11 => BlockInterruptHandler<DMA_CH11>;
    });

    const CHCFG: u32 = 0x8020_0000 | 3; // DMS | DAD, channel 11's index in its group

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A 64-word ring in 4 blocks of 2-word frames, feeding 0xE820_B018.
    fn ring() -> RingDma<'static> {
        test::reset();
        let buf = UncachedMut::new(std::vec![0u32; 64].leak()).unwrap();
        let dma = BlockDma::new(unsafe { DMA_CH11::steal() }, Irqs);
        RingDma::start(dma, buf, 4, 2, CHCFG, Dir::MemToPeriph { dst: 0xE820_B018 }, 0xE1)
    }

    fn crsa() -> usize {
        crate::pac::DMAC.ch(11).crsa().addr()
    }

    fn block_end() {
        unsafe { <BlockInterruptHandler<DMA_CH11> as Handler<_>>::on_interrupt() };
    }

    #[test]
    fn a_ring_links_its_blocks_in_a_loop() {
        let _serial = serial();
        let r = ring();
        let base = r.buf.bus_addr();
        let desc = |i: usize| unsafe { &*core::ptr::addr_of!(RING_DESC[11][i]) }.0;
        for i in 0..4 {
            let d = desc(i);
            assert_eq!(d[0], DESC_HEADER);
            assert_eq!(d[1], base + i as u32 * 64, "src of block {i}");
            assert_eq!(d[2], 0xE820_B018);
            assert_eq!(d[3], 64);
            assert_eq!(d[4] & (1 << 24), 0, "DEM clear: an END interrupt per block");
            let next = unsafe { core::ptr::addr_of!(RING_DESC[11][(i + 1) % 4]) } as u32;
            assert_eq!(d[7], next);
        }
        let nxla = crate::pac::DMAC.ch(11).nxla().addr();
        let head = unsafe { core::ptr::addr_of!(RING_DESC[11][0]) } as u32;
        assert_eq!(test::peek32(nxla), head);
    }

    #[test]
    fn position_wraps_a_crsa_one_past_the_end() {
        let _serial = serial();
        let r = ring();
        test::poke32(crsa(), r.buf.bus_addr() + 256);
        assert_eq!(r.position(), 0);
    }

    #[test]
    fn position_rounds_down_to_a_frame() {
        let _serial = serial();
        let r = ring();
        test::poke32(crsa(), r.buf.bus_addr() + 4 * 5);
        assert_eq!(r.position(), 4);
    }

    #[test]
    fn absolute_position_is_monotonic_across_an_uncounted_block_end() {
        let _serial = serial();
        let r = ring();
        // The DMAC is in block 1 (word 18) but block 0's END hasn't been counted yet.
        test::poke32(crsa(), r.buf.bus_addr() + 4 * 18);
        assert_eq!(r.absolute(), 18);
        block_end();
        assert_eq!(r.absolute(), 18);
    }

    #[test]
    fn absolute_counts_whole_laps() {
        let _serial = serial();
        let r = ring();
        for _ in 0..5 {
            block_end();
        }
        test::poke32(crsa(), r.buf.bus_addr() + 4 * 20);
        assert_eq!(r.absolute(), 5 * 16 + 4);
    }

    #[test]
    fn dropping_a_ring_halts_its_channel_before_returning() {
        let _serial = serial();
        let r = ring();
        let chctrl = crate::pac::DMAC.ch(11).chctrl().addr();
        let chstat = crate::pac::DMAC.ch(11).chstat().addr();
        test::poke32(chstat, 0);
        let start = test::log().len();
        drop(r);
        let log = test::log()[start..].to_vec();
        let clren = log.iter().position(|a| a.write && a.addr == chctrl && a.val & 2 != 0);
        let stat = log.iter().rposition(|a| !a.write && a.addr == chstat);
        assert!(matches!((clren, stat), (Some(c), Some(s)) if c < s), "CLREN, then CHSTAT polled");
    }
}
