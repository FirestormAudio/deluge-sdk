//! Circular DMA over a lent ring, split into blocks.
//!
//! A [`RingDma`] runs one link descriptor per block, each linked to the next and the last back to
//! the first, so the DMAC sweeps the ring forever. Every descriptor ends with an interrupt
//! ([`BlockInterruptHandler`](super::BlockInterruptHandler)), which counts the block; the count
//! and the DMAC's current address give [`RingDma::absolute`], the words moved since the start.
//!
//! The descriptors live in a table the caller lends with the ring ([`RingDescs`]), and the ring
//! holds it while the DMAC runs.

use core::sync::atomic::{AtomicU64, Ordering};

use super::BlockDma;
use crate::dma_buf::UncachedMut;
use crate::ring_buffer::RingDescs;

/// The most blocks a ring can have.
pub(crate) const MAX_RING_BLOCKS: usize = 16;

/// Link descriptor header (HW manual Table 9.15): LV = 1 (valid), LE = 0 (the link continues),
/// WBD = 1 (no write-back), DIM = 1.
const DESC_HEADER: u32 = 0b1101;

/// Block interrupts channel n has taken since its ring started. An edge-triggered interrupt can
/// cover two block ends, and a stale one can land after a restart, so this is only a hint:
/// [`RingDma::absolute`] reconciles it with where the DMAC is.
pub(crate) static BLOCKS_DONE: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

/// Which way a ring moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dir {
    /// The ring feeds the peripheral register `dst`.
    MemToPeriph { dst: u32 },
    /// The peripheral register `src` fills the ring.
    PeriphToMem { src: u32 },
}

/// `x % n`, as a mask when `n` is a power of two (every ring the drivers lend): the Cortex-A9 has no
/// divider, and `position` runs in interrupt handlers.
#[inline]
fn wrap(x: usize, n: usize) -> usize {
    if n.is_power_of_two() { x & (n - 1) } else { x % n }
}

/// A channel sweeping a lent ring in `blocks` blocks, with an absolute position. Dropping it
/// halts the channel before the ring's borrow ends.
pub(crate) struct RingDma<'d> {
    dma: BlockDma<'d>,
    buf: UncachedMut<'d, u32>,
    /// Read by the DMAC until the channel halts.
    _descs: RingDescs<'d>,
    blocks: usize,
    frame_words: usize,
    dir: Dir,
}

impl<'d> RingDma<'d> {
    /// Program `blocks` of `descs` over `buf` with `chcfg` (DEM clear, the channel's index in
    /// bits 2:0) and request `dmars`, then start the channel.
    ///
    /// # Panics
    /// If `blocks` is less than 2 or more than [`MAX_RING_BLOCKS`] (one block gives no position
    /// to check the count against), `descs` holds fewer than `blocks`, or `buf` isn't whole blocks
    /// of whole `frame_words`-word frames.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        mut dma: BlockDma<'d>,
        buf: UncachedMut<'d, u32>,
        mut descs: RingDescs<'d>,
        blocks: usize,
        frame_words: usize,
        chcfg: u32,
        dir: Dir,
        dmars: u32,
    ) -> Self {
        assert!(
            (2..=MAX_RING_BLOCKS).contains(&blocks),
            "dmac ring: {} blocks (2 to {})",
            blocks,
            MAX_RING_BLOCKS
        );
        assert!(
            descs.len() >= blocks,
            "dmac ring: {} blocks but {} descriptors",
            blocks,
            descs.len()
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
        // SAFETY: the lent table, held by this ring. `RingDescs::new` checked it is uncached
        // memory, so these writes are what the DMAC fetches, and its addresses are bus addresses.
        let head = unsafe {
            let mut desc = |i: usize| descs.word_ptr(i);
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
                // DEM clear: every block's end raises the interrupt that counts it.
                d.add(4).write_volatile(chcfg & !super::CHCFG_DEM);
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
        Self { dma, buf, _descs: descs, blocks, frame_words, dir }
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
        let word = wrap((addr.wrapping_sub(self.buf.bus_addr()) / 4) as usize, self.len());
        word - wrap(word, self.frame_words)
    }

    /// Words moved since the ring started, never decreasing. The block count is a hint (see
    /// [`BLOCKS_DONE`]); the block the DMAC is in decides: the count is moved to the nearest
    /// count with that block, forward or back. Right while the count is off by at most half the
    /// ring's blocks, so an interrupt can be late, lost or doubled by that much.
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
            let blocks = self.blocks as u64;
            let in_block = (p / block_words) as u64;
            // How far ahead of the count's block the DMAC is, modulo the ring.
            let ahead = (in_block + blocks - c1 % blocks) % blocks;
            let counted = if ahead <= blocks / 2 {
                c1 + ahead
            } else {
                c1.saturating_sub(blocks - ahead)
            };
            return counted * block_words as u64 + (p % block_words) as u64;
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
        // A channel that won't halt keeps reading the ring and its descriptors after their borrows
        // end; nothing better than a warning is possible here.
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
    use crate::ring_buffer::{RingDesc, RingDescs};

    crate::bind_interrupts!(struct Irqs {
        DMAINT11 => BlockInterruptHandler<DMA_CH11>;
    });

    const CHCFG: u32 = 0x8020_0000 | 3; // DMS | DAD, channel 11's index in its group

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A leaked table of `n` descriptors.
    fn table(n: usize) -> &'static mut [RingDesc] {
        (0..n).map(|_| RingDesc::NEW).collect::<std::vec::Vec<_>>().leak()
    }

    /// A 64-word ring in 4 blocks of 2-word frames, feeding 0xE820_B018, on `descs`.
    fn ring_on(descs: &'static mut [RingDesc]) -> RingDma<'static> {
        test::reset();
        let buf = UncachedMut::new(std::vec![0u32; 64].leak()).unwrap();
        let dma = BlockDma::new(unsafe { DMA_CH11::steal() }, Irqs);
        let descs = RingDescs::new(descs).unwrap();
        RingDma::start(dma, buf, descs, 4, 2, CHCFG, Dir::MemToPeriph { dst: 0xE820_B018 }, 0xE1)
    }

    fn ring() -> RingDma<'static> {
        ring_on(table(4))
    }

    fn crsa() -> usize {
        crate::pac::DMAC.ch(11).crsa().addr()
    }

    fn block_end() {
        unsafe { <BlockInterruptHandler<DMA_CH11> as Handler<_>>::on_interrupt() };
    }

    #[test]
    fn ring_descriptors_live_in_the_lent_table() {
        let _serial = serial();
        let t = table(4);
        let at = t.as_ptr() as usize;
        let r = ring_on(t);
        let base = r.buf.bus_addr();
        // The lent table is the uncached memory the DMAC reads: descriptor i is at its 32 * i.
        let addr = |i: usize| at + i * 32;
        let word = |i: usize, w: usize| unsafe { (addr(i) as *const u32).add(w).read_volatile() };
        for i in 0..4 {
            assert_eq!(word(i, 0), DESC_HEADER);
            assert_eq!(word(i, 1), base + i as u32 * 64, "src of block {i}");
            assert_eq!(word(i, 2), 0xE820_B018);
            assert_eq!(word(i, 3), 64);
            assert_eq!(word(i, 4) & (1 << 24), 0, "DEM clear: an END interrupt per block");
            assert_eq!(word(i, 7), addr((i + 1) % 4) as u32, "block {i} links to the next");
        }
        let nxla = crate::pac::DMAC.ch(11).nxla().addr();
        assert_eq!(test::peek32(nxla), addr(0) as u32);
    }

    #[test]
    #[should_panic(expected = "4 blocks but 3 descriptors")]
    fn a_short_descriptor_table_is_refused() {
        let _serial = serial();
        let _ = ring_on(table(3));
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

    /// Two block ends inside one masked-interrupt window raise one (edge-triggered) interrupt,
    /// so the count can lag by more than a block: the position still never goes back.
    #[test]
    fn a_lost_block_count_never_moves_the_position_back() {
        let _serial = serial();
        let r = ring();
        block_end(); // counted 1; the DMAC has really finished 2 blocks
        test::poke32(crsa(), r.buf.bus_addr() + 4 * 40); // block 2, word 8
        let first = r.absolute();
        test::poke32(crsa(), r.buf.bus_addr() + 4 * 50); // block 3, word 2: still no interrupt
        assert!(r.absolute() > first, "{} then {}", first, r.absolute());
        assert_eq!(r.absolute(), 50);
    }

    /// A stale pending block end from the channel's previous ring counts once too many: the
    /// position comes from where the DMAC is, not from the count alone.
    #[test]
    fn an_extra_block_count_is_resolved_by_the_position() {
        let _serial = serial();
        let r = ring();
        block_end();
        block_end(); // counted 2, but the DMAC is only in block 1
        test::poke32(crsa(), r.buf.bus_addr() + 4 * 20);
        assert_eq!(r.absolute(), 20);
    }

    #[test]
    #[should_panic(expected = "blocks")]
    fn a_one_block_ring_is_refused() {
        let _serial = serial();
        test::reset();
        let buf = UncachedMut::new(std::vec![0u32; 64].leak()).unwrap();
        let dma = BlockDma::new(unsafe { DMA_CH11::steal() }, Irqs);
        let descs = RingDescs::new(table(4)).unwrap();
        let dir = Dir::MemToPeriph { dst: 0xE820_B018 };
        let _ = RingDma::start(dma, buf, descs, 1, 2, CHCFG, dir, 0xE1);
    }

    #[test]
    fn start_clears_dem_so_every_block_interrupts() {
        let _serial = serial();
        test::reset();
        let buf = UncachedMut::new(std::vec![0u32; 64].leak()).unwrap();
        let dma = BlockDma::new(unsafe { DMA_CH11::steal() }, Irqs);
        let t = table(4);
        let at = t.as_ptr() as usize;
        let _r = RingDma::start(
            dma,
            buf,
            RingDescs::new(t).unwrap(),
            4,
            2,
            CHCFG | (1 << 24),
            Dir::MemToPeriph { dst: 0xE820_B018 },
            0xE1,
        );
        for i in 0..4 {
            let chcfg = (at + i * 32 + 16) as *const u32;
            assert_eq!(unsafe { chcfg.read_volatile() } & (1 << 24), 0);
        }
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
