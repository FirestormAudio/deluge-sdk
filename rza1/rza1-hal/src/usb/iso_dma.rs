//! Isochronous streams moved by the DMAC: no work per packet.
//!
//! A packet to the host is a link descriptor. Its end (`TEND`, sampled because `DnFBCFG.TENDE` is
//! set) commits a packet shorter than the endpoint's, so a stream of any packet sizes needs no
//! interrupt. The CPU writes a descriptor whole and marks it valid last; the DMAC clears the mark
//! after it, so the CPU sees what was sent, and stops at a descriptor that isn't valid: an underrun.

use core::cell::RefCell;

use critical_section::Mutex;
use embassy_usb_driver::EndpointType;

use super::driver::{Rusb1EndpointIn, Rusb1EndpointOut};
use super::fifo::FIFO_POLLS;
use super::pipe::{pipe_brdy_disable, pipe_disable, pipe_enable, pipe_reset};
use super::regs::{
    D0FBCFG_TENDE, FIFOCTR_FRDY, FIFOSEL_CURPIPE_MASK, FIFOSEL_DCLRM, FIFOSEL_DREQE,
    FIFOSEL_MBW_SHIFT, MBW_32, PIPECFG_BFRE, PIPECTR_PID_BUF, PIPECTR_PID_MASK, pipectr, usb,
};
use crate::dma_buf::{Uncached, UncachedMut};
use crate::dmac::{
    CHCFG_AM_BURST, CHCFG_DAD, CHCFG_DDS_32BIT, CHCFG_DEM, CHCFG_DMS, CHCFG_HIEN, CHCFG_LVL,
    CHCFG_REQD, CHCFG_SAD, CHCFG_SDS_32BIT, Dma,
};
use crate::pac::usb::Usb;
use crate::ring_buffer::RingDescs;

/// DMARS for each port's D0FIFO (MID 0b10_0000 / 0b10_0010, RID 0b11).
const D0FIFO_DMARS: [u32; 2] = [0x83, 0x8B];
/// DMARS for each port's D1FIFO (MID 0b10_0001 / 0b10_0011, RID 0b11).
const D1FIFO_DMARS: [u32; 2] = [0x87, 0x8F];
/// FIFOSEL with the 32-bit width and no pipe.
const MBW_WORDS: u16 = MBW_32 << FIFOSEL_MBW_SHIFT;

/// A descriptor's header once it is ready: LV (valid) and DIM; LE clear (the chain goes on), WBD
/// clear (the DMAC writes the header back with LV clear once it is done).
const READY: u32 = 0b1001;
/// A descriptor of a ring that never ends: LV, WBD (no write-back, so it stays valid) and DIM.
const LOOPING: u32 = 0b1101;

/// The chain of descriptors of a stream to the host: a ring the CPU fills and the DMAC empties.
pub(crate) struct Chain<'d> {
    descs: RingDescs<'d>,
    /// The FIFO register every packet goes to, and the channel's CHCFG.
    dst: u32,
    chcfg: u32,
    /// Descriptors made ready and taken back since the last reset.
    pushed: u64,
    sent: u64,
}

impl<'d> Chain<'d> {
    /// A chain on `descs`, all of it: nothing is ready.
    pub(crate) fn new(descs: RingDescs<'d>, dst: u32, chcfg: u32) -> Self {
        let mut chain = Self {
            descs,
            dst,
            chcfg,
            pushed: 0,
            sent: 0,
        };
        chain.reset();
        chain
    }

    /// How many descriptors there are.
    fn len(&self) -> usize {
        self.descs.len()
    }

    /// Forgets every packet: none is ready, and the next push is the first descriptor. The channel
    /// must be stopped.
    pub(crate) fn reset(&mut self) {
        for i in 0..self.len() {
            // SAFETY: the lent table, which the stopped channel does not read.
            unsafe { self.descs.word_ptr(i).write_volatile(0) };
        }
        (self.pushed, self.sent) = (0, 0);
    }

    /// The address of descriptor `i`, as the DMAC knows it.
    pub(crate) fn address(&mut self, i: usize) -> u32 {
        self.descs.word_ptr(i) as u32
    }

    /// Packets ready or being sent, as of the last [`reclaim`](Self::reclaim).
    pub(crate) fn queued(&self) -> usize {
        (self.pushed - self.sent) as usize
    }

    /// Makes one packet ready: `words` words from bus address `src`. False when every descriptor is
    /// in use.
    pub(crate) fn push(&mut self, src: u32, words: u32) -> bool {
        if self.queued() == self.len() {
            return false;
        }
        let i = (self.pushed % self.len() as u64) as usize;
        let next = self.address((i + 1) % self.len());
        let packet = [src, self.dst, words * 4, self.chcfg, 0, 0, next];
        // SAFETY: descriptor `i` is not ready (it was sent and taken back, or never used), so the
        // DMAC does not read it; the header goes last, and the barrier orders it after the rest.
        unsafe {
            let d = self.descs.word_ptr(i);
            for (k, w) in packet.into_iter().enumerate() {
                d.add(k + 1).write_volatile(w);
            }
            barrier();
            d.write_volatile(READY);
            barrier();
        }
        self.pushed += 1;
        true
    }

    /// Takes back every descriptor the DMAC has finished, in order. Returns how many.
    pub(crate) fn reclaim(&mut self) -> usize {
        let before = self.sent;
        while self.sent != self.pushed {
            let i = (self.sent % self.len() as u64) as usize;
            // SAFETY: the lent table; a header the DMAC may write back.
            if unsafe { self.descs.word_ptr(i).read_volatile() } & 1 != 0 {
                break;
            }
            self.sent += 1;
        }
        (self.sent - before) as usize
    }

    /// Where a stopped channel starts again: the first descriptor not yet sent.
    pub(crate) fn restart_at(&mut self) -> *const u32 {
        let i = (self.sent % self.len() as u64) as usize;
        self.descs.word_ptr(i)
    }
}

/// Orders the writes before it ahead of those after it, as the DMAC sees them.
fn barrier() {
    // SAFETY: a barrier, with no memory or register effects of its own.
    #[cfg(target_os = "none")]
    unsafe {
        core::arch::asm!("dsb", options(nostack))
    };
}

// ── A stream to the host ────────────────────────────────────────────────────

/// A port's stream to the host, from the pipe's endpoint to its DMA channel.
struct InStream {
    pipe: u8,
    dma: Dma<'static>,
    chain: Chain<'static>,
    start_ahead: usize,
    /// The host has the endpoint enabled, and the FIFO port is the stream's.
    armed: bool,
    /// The DMA has been started, and PID is BUF, since the endpoint was enabled.
    launched: bool,
    generation: u32,
    stops: u32,
}

/// Per port, the stream on its D0FIFO.
static IN_STREAM: [Mutex<RefCell<Option<InStream>>>; 2] =
    [const { Mutex::new(RefCell::new(None)) }; 2];

/// Runs `f` on port `port`'s stream to the host, if it has one.
fn with_in<R>(port: usize, f: impl FnOnce(&mut InStream) -> R) -> Option<R> {
    critical_section::with(|cs| IN_STREAM[port].borrow_ref_mut(cs).as_mut().map(f))
}

impl InStream {
    /// Takes the endpoint: the pipe reset and NAK, the FIFO port on it, nothing queued. The DMA starts
    /// when enough is pushed.
    ///
    /// # Safety
    /// The caller owns port `u` and its pipes.
    unsafe fn arm(&mut self, u: Usb) {
        self.disarm(u);
        let n = self.pipe as usize;
        unsafe {
            pipe_brdy_disable(u, n);
            pipe_reset(u, n); // NAK, deselected, its buffers and toggle cleared
        }
        self.chain.reset();
        self.generation = self.generation.wrapping_add(1);
        u.d0fbcfg().write_value(D0FBCFG_TENDE);
        u.d0fifosel().write_value(MBW_WORDS);
        let deselected = (0..FIFO_POLLS).any(|_| u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK == 0);
        u.d0fifosel().write_value(MBW_WORDS | u16::from(self.pipe));
        let ready = deselected
            && (0..FIFO_POLLS).any(|_| {
                u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK == u16::from(self.pipe)
                    && u.d0fifoctr().read() & FIFOCTR_FRDY != 0
            });
        if !ready {
            warn!("usb{}: D0FIFO not ready for the stream", self.pipe);
        }
        self.armed = ready;
    }

    /// Gives the endpoint and the port up: DREQE off, the channel stopped, the port deselected.
    fn disarm(&mut self, u: Usb) {
        if !self.armed {
            return;
        }
        u.d0fifosel().modify(|r| *r &= !FIFOSEL_DREQE);
        // The channel is halted before anything it reads is changed.
        crate::dmac::halt_channel(self.dma.number());
        self.dma.stop();
        u.d0fifosel().write_value(0);
        u.d0fbcfg().write_value(0);
        (self.armed, self.launched) = (false, false);
    }

    /// Starts the channel at the first packet not sent. The first time, the port asks for transfers and the pipe
    /// answers the host.
    fn launch(&mut self, u: Usb, port: usize) {
        let first = self.chain.restart_at();
        // SAFETY: the chain's descriptors are complete and lent for good; the channel is stopped.
        unsafe {
            self.dma
                .init_with_link_descriptor(first, D0FIFO_DMARS[port]);
            self.dma.channel_start();
        }
        if !self.launched {
            u.d0fifosel().modify(|r| *r |= FIFOSEL_DREQE);
            // SAFETY: the stream's pipe, armed.
            unsafe { pipe_enable(u, self.pipe as usize) };
            self.launched = true;
        }
    }

    fn push(&mut self, packet: &[u32], port: usize) -> bool {
        if !self.armed || packet.is_empty() {
            return false;
        }
        self.chain.reclaim();
        if !self.chain.push(packet.as_ptr() as u32, packet.len() as u32) {
            return false;
        }
        if !self.dma.enabled() && self.chain.queued() >= self.start_ahead {
            if self.launched {
                self.stops = self.stops.wrapping_add(1);
            }
            self.launch(usb(port as u8), port);
        }
        true
    }
}

/// A stream of packets to the host, moved by a DMA channel. Dropping it stops the DMA and frees the port.
///
/// Packets are whole 32-bit words: the endpoint's frames must be multiples of 4 bytes. Start the endpoint's class
/// handler as usual; the stream runs while the host has it enabled.
pub struct DmaIsoIn {
    port: u8,
    pipe: u8,
}

impl DmaIsoIn {
    /// Queues a packet: `packet` is in uncached memory, holds whole words, and must stay as it is until the stream has
    /// sent it. False when the host has the endpoint disabled or the queue is full.
    ///
    /// The DMA starts, and starts again after it stopped for want of a packet, when the stream's threshold
    /// ([`into_dma_stream`](Rusb1EndpointIn::into_dma_stream)) is queued.
    pub fn push(&mut self, packet: Uncached<'static, u32>) -> bool {
        with_in(self.port as usize, |s| {
            s.push(packet.as_slice(), self.port as usize)
        })
        .unwrap_or(false)
    }

    /// Packets queued and not sent.
    pub fn queued(&mut self) -> usize {
        with_in(self.port as usize, |s| {
            s.chain.reclaim();
            s.chain.queued()
        })
        .unwrap_or(0)
    }

    /// How many times the endpoint has been enabled: it changes when a stream starts over.
    pub fn generation(&self) -> u32 {
        with_in(self.port as usize, |s| s.generation).unwrap_or(0)
    }

    /// Times the DMA ran out of packets and was started again.
    pub fn stops(&self) -> u32 {
        with_in(self.port as usize, |s| s.stops).unwrap_or(0)
    }

    /// Whether the host has the endpoint enabled and the DMA is running.
    pub fn running(&self) -> bool {
        with_in(self.port as usize, |s| s.launched && s.dma.enabled()).unwrap_or(false)
    }

    /// The pipe it runs on.
    pub fn pipe(&self) -> u8 {
        self.pipe
    }
}

#[cfg(all(test, not(target_os = "none")))]
impl DmaIsoIn {
    /// The chain, for a test to play the DMAC's part.
    fn with_chain<R>(&mut self, f: impl FnOnce(&mut Chain<'static>) -> R) -> R {
        with_in(self.port as usize, |s| f(&mut s.chain)).unwrap()
    }

    fn first_descriptor(&mut self) -> u32 {
        self.with_chain(|c| c.address(0))
    }
}

impl Drop for DmaIsoIn {
    fn drop(&mut self) {
        critical_section::with(|cs| {
            let slot = &mut *IN_STREAM[self.port as usize].borrow_ref_mut(cs);
            if let Some(s) = slot.as_mut() {
                s.disarm(usb(self.port));
            }
            *slot = None;
        });
    }
}

/// Binds a stream to the host to `pipe` on `port`, with nothing enabled yet.
fn bind_in(
    port: u8,
    pipe: u8,
    dma: Dma<'static>,
    descs: RingDescs<'static>,
    start_ahead: usize,
) -> DmaIsoIn {
    let chcfg = CHCFG_DMS
        | CHCFG_DEM
        | CHCFG_DAD
        | CHCFG_DDS_32BIT
        | CHCFG_SDS_32BIT
        | CHCFG_AM_BURST
        | CHCFG_LVL
        | CHCFG_HIEN
        | CHCFG_REQD
        | u32::from(dma.number() & 7);
    let dst = usb(port).d0fifo().addr() as u32;
    critical_section::with(|cs| {
        let slot = &mut *IN_STREAM[port as usize].borrow_ref_mut(cs);
        assert!(
            slot.is_none(),
            "usb: port {} already has a DMA stream on D0FIFO",
            port
        );
        assert!(
            start_ahead <= descs.len(),
            "usb: more ahead than descriptors"
        );
        *slot = Some(InStream {
            pipe,
            dma,
            chain: Chain::new(descs, dst, chcfg),
            start_ahead,
            armed: false,
            launched: false,
            generation: 0,
            stops: 0,
        });
    });
    DmaIsoIn { port, pipe }
}

// ── A stream from the host ──────────────────────────────────────────────────

/// A port's stream from the host: a ring its DMA channel fills for as long as the endpoint is enabled.
struct OutStream {
    port: u8,
    pipe: u8,
    dma: Dma<'static>,
    ring: UncachedMut<'static, u32>,
    descs: RingDescs<'static>,
    armed: bool,
    generation: u32,
    /// Words received since the endpoint was enabled, and where in the ring the last look was.
    total: u32,
    last: usize,
}

/// Per port, the stream on its D1FIFO.
static OUT_STREAM: [Mutex<RefCell<Option<OutStream>>>; 2] =
    [const { Mutex::new(RefCell::new(None)) }; 2];

/// Runs `f` on port `port`'s stream from the host, if it has one.
fn with_out<R>(port: usize, f: impl FnOnce(&mut OutStream) -> R) -> Option<R> {
    critical_section::with(|cs| OUT_STREAM[port].borrow_ref_mut(cs).as_mut().map(f))
}

impl OutStream {
    /// Takes the endpoint and sets the ring going: the pipe set to raise no BRDY until it is read dry (BFRE), the
    /// buffer cleared as the DMA reads it (DCLRM), and the host answered.
    ///
    /// # Safety
    /// The caller owns port `u` and its pipes.
    unsafe fn arm(&mut self, u: Usb, port: usize) {
        self.disarm(u);
        let n = self.pipe as usize;
        // SAFETY: the stream's pipe.
        unsafe {
            pipe_brdy_disable(u, n);
            pipe_reset(u, n); // NAK, deselected, its buffers and toggle cleared
        }
        set_bfre(u, n, true);
        // A change of BFRE needs the pipe cleared after it (TRM Table 28.13).
        // SAFETY: the stream's pipe.
        unsafe { pipe_reset(u, n) };
        self.chain();
        self.generation = self.generation.wrapping_add(1);
        (self.total, self.last) = (0, 0);
        u.d1fifosel().write_value(MBW_WORDS);
        let deselected = (0..FIFO_POLLS).any(|_| u.d1fifosel().read() & FIFOSEL_CURPIPE_MASK == 0);
        u.d1fifosel()
            .write_value(MBW_WORDS | FIFOSEL_DCLRM | u16::from(self.pipe));
        let selected = deselected
            && (0..FIFO_POLLS)
                .any(|_| u.d1fifosel().read() & FIFOSEL_CURPIPE_MASK == u16::from(self.pipe));
        if !selected {
            warn!("usb{}: D1FIFO not ready for the stream", port);
            set_bfre(u, n, false);
            u.d1fifosel().write_value(0);
            return;
        }
        // SAFETY: the ring's descriptors, lent for good; the channel is stopped.
        unsafe {
            self.dma
                .init_with_link_descriptor(self.descs.word_ptr(0), D1FIFO_DMARS[port]);
            self.dma.channel_start();
        }
        u.d1fifosel().modify(|r| *r |= FIFOSEL_DREQE);
        // SAFETY: the stream's pipe.
        unsafe { pipe_enable(u, n) };
        self.armed = true;
    }

    /// Gives the endpoint and the port up: DREQE off, the channel stopped, the port deselected, BFRE off.
    fn disarm(&mut self, u: Usb) {
        if !self.armed {
            return;
        }
        u.d1fifosel().modify(|r| *r &= !FIFOSEL_DREQE);
        // The channel is halted before anything it writes is let go.
        crate::dmac::halt_channel(self.dma.number());
        self.dma.stop();
        u.d1fifosel().write_value(0);
        let n = self.pipe as usize;
        // SAFETY: the stream's pipe. PIPECFG changes with PID at NAK and no port on the pipe.
        unsafe { pipe_disable(u, n) };
        set_bfre(u, n, false);
        self.armed = false;
    }

    /// Writes the descriptors: the ring in equal parts, the last linked to the first.
    fn chain(&mut self) {
        let (n, words) = (self.descs.len(), self.ring.len() / self.descs.len());
        let base = self.ring.bus_addr();
        for i in 0..n {
            let next = self.descs.word_ptr((i + 1) % n) as u32;
            let dst = base + (i * words * 4) as u32;
            let d = self.descs.word_ptr(i);
            let w = [
                LOOPING,
                self.src(),
                dst,
                (words * 4) as u32,
                self.chcfg(),
                0,
                0,
                next,
            ];
            for (k, v) in w.into_iter().enumerate() {
                // SAFETY: the lent table; the channel is stopped.
                unsafe { d.add(k).write_volatile(v) };
            }
        }
        barrier();
    }

    fn src(&self) -> u32 {
        usb(self.port).d1fifo().addr() as u32
    }

    fn chcfg(&self) -> u32 {
        CHCFG_DMS
            | CHCFG_DEM
            | CHCFG_SAD
            | CHCFG_DDS_32BIT
            | CHCFG_SDS_32BIT
            | CHCFG_AM_BURST
            | CHCFG_LVL
            | CHCFG_HIEN
            | u32::from(self.dma.number() & 7)
    }

    /// Words received since the endpoint was enabled.
    fn position(&mut self) -> u32 {
        if self.armed {
            let len = self.ring.len();
            let word = self.dma.current_dst().wrapping_sub(self.ring.bus_addr()) / 4;
            let at = word as usize % len;
            self.total = self
                .total
                .wrapping_add(((at + len - self.last) % len) as u32);
            self.last = at;
        }
        self.total
    }
}

/// Sets or clears pipe `n`'s BFRE, with the pipe selected for it only here. PID must be NAK.
fn set_bfre(u: Usb, n: usize, on: bool) {
    critical_section::with(|_| {
        u.pipesel().write_value(n as u16);
        u.pipecfg().modify(|r| {
            *r = if on {
                *r | PIPECFG_BFRE
            } else {
                *r & !PIPECFG_BFRE
            };
        });
        u.pipesel().write_value(0);
    });
}

/// A stream of packets from the host, moved by a DMA channel into a ring. Dropping it stops the DMA and frees the port.
pub struct DmaIsoOut {
    port: u8,
    pipe: u8,
}

impl DmaIsoOut {
    /// Words received since the host last enabled the endpoint, wherever the ring is: `position % ring length` is the
    /// next word the DMA writes. Call it more often than the ring fills.
    pub fn position(&mut self) -> u32 {
        with_out(self.port as usize, |s| s.position()).unwrap_or(0)
    }

    /// How many times the endpoint has been enabled: it changes when the stream starts over, and the position with it.
    pub fn generation(&self) -> u32 {
        with_out(self.port as usize, |s| s.generation).unwrap_or(0)
    }

    /// Whether the host has the endpoint enabled and the DMA is running.
    pub fn running(&self) -> bool {
        with_out(self.port as usize, |s| s.armed && s.dma.enabled()).unwrap_or(false)
    }

    /// The pipe it runs on.
    pub fn pipe(&self) -> u8 {
        self.pipe
    }
}

#[cfg(all(test, not(target_os = "none")))]
impl DmaIsoOut {
    fn with_descs<R>(&mut self, f: impl FnOnce(&mut RingDescs<'static>) -> R) -> R {
        with_out(self.port as usize, |s| f(&mut s.descs)).unwrap()
    }
}

impl Drop for DmaIsoOut {
    fn drop(&mut self) {
        critical_section::with(|cs| {
            let slot = &mut *OUT_STREAM[self.port as usize].borrow_ref_mut(cs);
            if let Some(s) = slot.as_mut() {
                s.disarm(usb(self.port));
            }
            *slot = None;
        });
    }
}

/// Binds a stream from the host to `pipe` on `port`, with nothing enabled yet.
fn bind_out(
    port: u8,
    pipe: u8,
    dma: Dma<'static>,
    ring: UncachedMut<'static, u32>,
    descs: RingDescs<'static>,
) -> DmaIsoOut {
    assert!(
        descs.len() >= 2 && ring.len() % descs.len() == 0,
        "usb: the ring isn't whole descriptors"
    );
    critical_section::with(|cs| {
        let slot = &mut *OUT_STREAM[port as usize].borrow_ref_mut(cs);
        assert!(
            slot.is_none(),
            "usb: port {} already has a DMA stream on D1FIFO",
            port
        );
        *slot = Some(OutStream {
            port,
            pipe,
            dma,
            ring,
            descs,
            armed: false,
            generation: 0,
            total: 0,
            last: 0,
        });
    });
    DmaIsoOut { port, pipe }
}

impl Rusb1EndpointOut {
    /// Receives this isochronous endpoint's packets by DMA into `ring`, one after the other, whole words each. The ring
    /// is in as many equal parts as `descs` (at least two); read it with the stream's
    /// [`position`](DmaIsoOut::position).
    ///
    /// # Panics
    /// If the endpoint isn't isochronous, the ring isn't whole descriptors, or the port already has such a stream (it
    /// has one port for DMA: D1FIFO).
    pub fn into_dma_stream(
        self,
        dma: Dma<'static>,
        ring: UncachedMut<'static, u32>,
        descs: RingDescs<'static>,
    ) -> DmaIsoOut {
        assert!(
            self.info.ep_type == EndpointType::Isochronous,
            "usb: into_dma_stream needs an isochronous endpoint"
        );
        let stream = bind_out(self.port, self.pipe, dma, ring, descs);
        let u = usb(self.port);
        if pipectr(u, self.pipe as usize).read() & PIPECTR_PID_MASK == PIPECTR_PID_BUF {
            // SAFETY: this endpoint's pipe.
            unsafe { set_enabled(self.port, self.pipe, true) };
        }
        stream
    }
}

/// The host enabled or disabled a pipe's endpoint: true if a stream is bound to it, which has taken or given it up.
/// (The driver still disables the pipe itself.)
///
/// # Safety
/// The caller owns port `port` and its pipes.
pub(crate) unsafe fn set_enabled(port: u8, pipe: u8, enabled: bool) -> bool {
    let in_stream = with_in(port as usize, |s| {
        if s.pipe != pipe {
            return false;
        }
        if enabled {
            // SAFETY: the caller's contract.
            unsafe { s.arm(usb(port)) };
        } else {
            s.disarm(usb(port));
        }
        true
    })
    .unwrap_or(false);
    in_stream
        || with_out(port as usize, |s| {
            if s.pipe != pipe {
                return false;
            }
            if enabled {
                // SAFETY: the caller's contract.
                unsafe { s.arm(usb(port), port as usize) };
            } else {
                s.disarm(usb(port));
            }
            true
        })
        .unwrap_or(false)
}

/// The bus was reset: every stream of port `port` lets its endpoint and port go.
pub(crate) fn bus_reset(port: u8) {
    with_in(port as usize, |s| s.disarm(usb(port)));
    with_out(port as usize, |s| s.disarm(usb(port)));
}

impl Rusb1EndpointIn {
    /// Sends packets on this isochronous endpoint by DMA. `descs` is the queue of packets and `start_ahead` how many of
    /// them it takes to start the DMA, and to start it again after it ran dry: the latency the stream holds.
    ///
    /// # Panics
    /// If the endpoint isn't isochronous, or the port already has such a stream (it has one port for DMA: D0FIFO).
    pub fn into_dma_stream(
        self,
        dma: Dma<'static>,
        descs: RingDescs<'static>,
        start_ahead: usize,
    ) -> DmaIsoIn {
        assert!(
            self.info.ep_type == EndpointType::Isochronous,
            "usb: into_dma_stream needs an isochronous endpoint"
        );
        let stream = bind_in(self.port, self.pipe, dma, descs, start_ahead);
        // Bound after the host enabled the endpoint: take it now, as the enable would have.
        let u = usb(self.port);
        if pipectr(u, self.pipe as usize).read() & PIPECTR_PID_MASK == PIPECTR_PID_BUF {
            // SAFETY: this endpoint's pipe.
            unsafe { set_enabled(self.port, self.pipe, true) };
        }
        stream
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::ring_buffer::test_descs;

    const DST: u32 = 0xE801_0018;
    const CHCFG: u32 = 0x8122_2268;

    fn chain(n: usize) -> Chain<'static> {
        Chain::new(test_descs(n), DST, CHCFG)
    }

    fn word(c: &mut Chain, i: usize, w: usize) -> u32 {
        unsafe { c.descs.word_ptr(i).add(w).read_volatile() }
    }

    /// The DMAC has sent descriptor `i`: it clears the valid mark.
    fn sent(c: &mut Chain, i: usize) {
        unsafe { c.descs.word_ptr(i).write_volatile(0b1000) };
    }

    #[test]
    fn a_pushed_descriptor_is_complete_and_valid() {
        let mut c = chain(4);
        assert!(c.push(0x2000_0000, 12));
        assert_eq!(
            word(&mut c, 0, 0),
            0b1001,
            "valid, the chain goes on, written back"
        );
        assert_eq!(word(&mut c, 0, 1), 0x2000_0000);
        assert_eq!(word(&mut c, 0, 2), DST);
        assert_eq!(word(&mut c, 0, 3), 48, "bytes");
        assert_eq!(word(&mut c, 0, 4), CHCFG);
        let next = c.address(1);
        assert_eq!(word(&mut c, 0, 7), next);
    }

    #[test]
    fn the_last_descriptor_links_to_the_first() {
        let mut c = chain(3);
        for i in 0..3 {
            assert!(c.push(i * 64, 4));
        }
        let first = c.address(0);
        assert_eq!(word(&mut c, 2, 7), first);
    }

    #[test]
    fn a_full_chain_refuses_a_push() {
        let mut c = chain(3);
        for _ in 0..3 {
            assert!(c.push(0, 4));
        }
        assert!(!c.push(0, 4));
        assert_eq!(c.queued(), 3);
        assert_eq!(word(&mut c, 0, 1), 0, "nothing overwritten");
    }

    #[test]
    fn reclaim_takes_back_only_what_the_dmac_cleared_in_order() {
        let mut c = chain(4);
        for _ in 0..4 {
            assert!(c.push(0, 4));
        }
        sent(&mut c, 0);
        sent(&mut c, 2); // not next: the one before it is still going
        assert_eq!(c.reclaim(), 1);
        assert_eq!(c.queued(), 3);
        sent(&mut c, 1);
        assert_eq!(c.reclaim(), 2);
        assert_eq!(c.queued(), 1);
    }

    #[test]
    fn queued_survives_the_wrap() {
        let mut c = chain(3);
        for k in 0..50u32 {
            assert!(c.push(k * 16, 4));
            assert_eq!(c.queued(), 1);
            sent(&mut c, (k % 3) as usize);
            assert_eq!(c.reclaim(), 1);
            assert_eq!(c.queued(), 0);
        }
    }

    /// Reset forgets every descriptor, so none is left valid for the next start.
    #[test]
    fn reset_leaves_nothing_valid() {
        let mut c = chain(3);
        for _ in 0..3 {
            c.push(0, 4);
        }
        c.reset();
        assert_eq!(c.queued(), 0);
        for i in 0..3 {
            assert_eq!(word(&mut c, i, 0) & 1, 0);
        }
        assert!(c.push(0x10, 4));
        assert_eq!(word(&mut c, 0, 1), 0x10, "the first descriptor again");
    }

    #[test]
    fn the_first_unsent_descriptor_is_where_a_restart_begins() {
        let mut c = chain(4);
        for _ in 0..3 {
            c.push(0, 4);
        }
        sent(&mut c, 0);
        c.reclaim();
        let at = c.address(1);
        assert_eq!(c.restart_at() as u32, at);
    }

    // ── The stream to the host ──────────────────────────────────────────────
    use crate::dmac::{CHSTAT_EN, Dma};
    use crate::mmio::test;
    use crate::peripherals::DMA_CH0;
    use crate::usb::regs::{
        FIFOCTR_FRDY, FIFOSEL_CURPIPE_MASK, FIFOSEL_DREQE, PIPECTR_PID_BUF, PIPECTR_PID_MASK,
        pipectr, usb,
    };

    fn set_enabled(port: u8, pipe: u8, enabled: bool) -> bool {
        // SAFETY: the test owns the port (SERIAL).
        unsafe { super::set_enabled(port, pipe, enabled) }
    }

    /// The streams are statics, one a port and direction: these tests share port 1's.
    use crate::usb::PORT1_TESTS as SERIAL;

    const PIPE: u8 = 2;
    const START: usize = 3;

    fn chctrl() -> usize {
        crate::pac::DMAC.ch(0).chctrl().addr()
    }
    fn chstat() -> usize {
        crate::pac::DMAC.ch(0).chstat().addr()
    }

    /// A stream on port 1, pipe 2, channel 0, on 8 descriptors, with nothing enabled yet and the FIFO ready.
    fn stream() -> DmaIsoIn {
        test::reset();
        test::poke16(usb(1).d0fifoctr().addr(), FIFOCTR_FRDY);
        let dma = unsafe { Dma::steal::<DMA_CH0>() };
        bind_in(1, PIPE, dma, test_descs(8), START)
    }

    fn packet() -> &'static [u32] {
        std::vec![0x1234_5678u32; 6].leak()
    }

    fn push(s: &mut DmaIsoIn) -> bool {
        s.push(crate::dma_buf::Uncached::new(packet()).unwrap())
    }

    fn channel_started() -> usize {
        test::log()
            .iter()
            .filter(|a| a.write && a.addr == chctrl() && a.val & 1 != 0)
            .count()
    }

    /// Where the last write to `addr` was.
    fn last(addr: usize) -> Option<usize> {
        test::log().iter().rposition(|a| a.write && a.addr == addr)
    }

    #[test]
    fn enabling_arms_the_port_and_starts_nothing() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let s = stream();
        assert!(set_enabled(1, PIPE, true));
        let u = usb(1);
        assert_eq!(test::peek16(u.d0fbcfg().addr()), 0x0010, "TENDE");
        assert_eq!(
            test::peek16(u.d0fifosel().addr()) & FIFOSEL_CURPIPE_MASK,
            u16::from(PIPE)
        );
        assert_eq!(test::peek16(u.d0fifosel().addr()) & FIFOSEL_DREQE, 0);
        assert_eq!(
            test::peek16(pipectr(u, PIPE as usize).addr()) & PIPECTR_PID_MASK,
            0,
            "NAK"
        );
        assert_eq!(channel_started(), 0);
        assert_eq!(s.generation(), 1);
        assert!(!s.running());
    }

    /// TENDE is on before the pipe is selected, and the port is deselected before it is selected again.
    #[test]
    fn the_port_is_set_up_in_order() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _s = stream();
        set_enabled(1, PIPE, true);
        let u = usb(1);
        let log = test::log();
        let at = |pred: &dyn Fn(&crate::mmio::test::Access) -> bool| {
            log.iter().position(|a| a.write && pred(a)).unwrap()
        };
        let tende = at(&|a| a.addr == u.d0fbcfg().addr() && a.val == 0x0010);
        let select = at(&|a| a.addr == u.d0fifosel().addr() && a.val & 0xF == u32::from(PIPE));
        let deselect = log[..select]
            .iter()
            .rposition(|a| a.write && a.addr == u.d0fifosel().addr() && a.val & 0xF == 0)
            .unwrap();
        assert!(tende < select && deselect < select);
    }

    #[test]
    fn pushing_before_the_host_enables_the_endpoint_is_refused() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stream();
        assert!(!push(&mut s));
        assert_eq!(s.queued(), 0);
    }

    /// The DMA starts once enough is queued: not with a packet less. DREQE, then PID = BUF.
    #[test]
    fn it_starts_only_with_enough_queued() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stream();
        set_enabled(1, PIPE, true);
        for _ in 0..START - 1 {
            assert!(push(&mut s));
        }
        assert_eq!(channel_started(), 0);
        assert!(push(&mut s));
        assert_eq!(channel_started(), 1);
        let u = usb(1);
        assert_eq!(
            test::peek32(crate::pac::DMAC.ch(0).nxla().addr()),
            s.first_descriptor()
        );
        assert_ne!(test::peek16(u.d0fifosel().addr()) & FIFOSEL_DREQE, 0);
        assert_eq!(
            test::peek16(pipectr(u, PIPE as usize).addr()) & PIPECTR_PID_MASK,
            PIPECTR_PID_BUF
        );
        let start = last(chctrl()).unwrap();
        let dreqe = last(u.d0fifosel().addr()).unwrap();
        let buf = last(pipectr(u, PIPE as usize).addr()).unwrap();
        assert!(
            start < dreqe && dreqe < buf,
            "start, then DREQE, then PID=BUF"
        );
    }

    /// A channel that has run on keeps going: nothing is restarted while it is enabled.
    #[test]
    fn a_running_channel_is_left_alone() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stream();
        set_enabled(1, PIPE, true);
        for _ in 0..START {
            push(&mut s);
        }
        test::poke32(chstat(), CHSTAT_EN);
        for _ in 0..3 {
            assert!(push(&mut s));
        }
        assert_eq!(channel_started(), 1);
        assert_eq!(s.stops(), 0);
        assert!(s.running());
    }

    /// The DMAC stopped at a descriptor that wasn't ready: the next pushes start it at the first one not sent, and
    /// nothing is sent twice.
    #[test]
    fn a_stopped_channel_restarts_at_the_first_unsent_descriptor() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stream();
        set_enabled(1, PIPE, true);
        for _ in 0..START {
            push(&mut s);
        }
        s.with_chain(|c| {
            for i in 0..START {
                sent(c, i);
            }
        });
        test::poke32(chstat(), 0);
        for _ in 0..START - 1 {
            assert!(push(&mut s));
        }
        assert_eq!(s.stops(), 0, "not until enough is queued again");
        assert!(push(&mut s));
        assert_eq!(s.stops(), 1);
        assert_eq!(channel_started(), 2);
        let want = s.with_chain(|c| c.address(START));
        assert_eq!(test::peek32(crate::pac::DMAC.ch(0).nxla().addr()), want);
    }

    #[test]
    fn disabling_stops_the_dma_and_frees_the_port() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stream();
        set_enabled(1, PIPE, true);
        for _ in 0..START {
            push(&mut s);
        }
        test::poke32(chstat(), CHSTAT_EN);
        assert!(set_enabled(1, PIPE, false));
        let u = usb(1);
        assert_eq!(test::peek16(u.d0fifosel().addr()), 0);
        assert_eq!(test::peek16(u.d0fbcfg().addr()), 0);
        assert_eq!(test::peek32(chctrl()) & 1, 0, "stopped");
        assert!(!push(&mut s), "and refuses packets");
        // DREQE goes off before the pipe is deselected.
        let log = test::log();
        let sel = |a: &crate::mmio::test::Access| a.write && a.addr == u.d0fifosel().addr();
        let on = log
            .iter()
            .rposition(|a| sel(a) && a.val & u32::from(FIFOSEL_DREQE) != 0)
            .expect("DREQE was on");
        let off = on
            + log[on..]
                .iter()
                .position(|a| {
                    sel(a)
                        && a.val & u32::from(FIFOSEL_DREQE) == 0
                        && a.val & 0xF == u32::from(PIPE)
                })
                .expect("DREQE off with the pipe still selected");
        let gone = log.iter().rposition(|a| sel(a) && a.val == 0).unwrap();
        assert!(off < gone);
    }

    #[test]
    fn enabling_again_forgets_what_was_queued() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stream();
        set_enabled(1, PIPE, true);
        push(&mut s);
        push(&mut s);
        set_enabled(1, PIPE, true);
        assert_eq!(s.queued(), 0);
        assert_eq!(s.generation(), 2);
        assert_eq!(channel_started(), 0);
    }

    #[test]
    fn a_bus_reset_stops_the_stream() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = stream();
        set_enabled(1, PIPE, true);
        for _ in 0..START {
            push(&mut s);
        }
        test::poke32(chstat(), CHSTAT_EN);
        bus_reset(1);
        assert_eq!(test::peek32(chctrl()) & 1, 0);
        assert_eq!(test::peek16(usb(1).d0fifosel().addr()), 0);
        assert!(!push(&mut s));
    }

    /// Another pipe's endpoint is not this stream's business.
    #[test]
    fn another_pipe_is_not_the_streams() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _s = stream();
        assert!(!set_enabled(1, PIPE + 1, true));
        assert!(!set_enabled(0, PIPE, true));
    }

    #[test]
    #[should_panic(expected = "D0FIFO")]
    fn a_second_stream_on_a_port_is_refused() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _s = stream();
        let _second = bind_in(
            1,
            1,
            unsafe { Dma::steal::<DMA_CH0>() },
            test_descs(8),
            START,
        );
    }

    #[test]
    fn dropping_a_stream_frees_its_port() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        drop(stream());
        let _again = stream();
    }

    // ── The stream from the host ────────────────────────────────────────────
    use crate::dma_buf::UncachedMut;
    use crate::peripherals::DMA_CH1;
    use crate::usb::regs::{FIFOSEL_DCLRM, PIPECFG_BFRE, PIPECTR_ACLRM};

    const OUT_PIPE: u8 = 1;
    const WORDS: usize = 64;

    fn out_chctrl() -> usize {
        crate::pac::DMAC.ch(1).chctrl().addr()
    }

    /// A stream on port 1, pipe 1, channel 1, with a 64-word ring in 4 descriptors, and the ring.
    fn out_stream() -> (DmaIsoOut, u32) {
        test::reset();
        test::poke16(usb(1).d1fifoctr().addr(), FIFOCTR_FRDY);
        let ring = UncachedMut::new(std::vec![0u32; WORDS].leak()).unwrap();
        let base = ring.bus_addr();
        let dma = unsafe { Dma::steal::<DMA_CH1>() };
        (bind_out(1, OUT_PIPE, dma, ring, test_descs(4)), base)
    }

    fn at(base: u32, word: usize) {
        test::poke32(crate::pac::DMAC.ch(1).crda().addr(), base + word as u32 * 4);
    }

    #[test]
    fn enabling_an_out_endpoint_sets_the_ring_going() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, base) = out_stream();
        assert!(set_enabled(1, OUT_PIPE, true));
        let u = usb(1);
        assert_ne!(test::peek16(u.pipecfg().addr()) & PIPECFG_BFRE, 0, "BFRE");
        assert_eq!(
            test::peek16(u.pipesel().addr()),
            0,
            "PIPESEL left on no pipe"
        );
        let sel = test::peek16(u.d1fifosel().addr());
        assert_eq!(sel & FIFOSEL_CURPIPE_MASK, u16::from(OUT_PIPE));
        assert_ne!(sel & FIFOSEL_DCLRM, 0, "DCLRM");
        assert_ne!(sel & FIFOSEL_DREQE, 0, "DREQE");
        assert_eq!(
            test::peek16(pipectr(u, OUT_PIPE as usize).addr()) & PIPECTR_PID_MASK,
            PIPECTR_PID_BUF
        );
        assert_ne!(test::peek32(out_chctrl()) & 1, 0, "channel started");
        assert_eq!(s.generation(), 1);
        // The ring's four quarters, in a loop that never ends.
        s.with_descs(|d| {
            for i in 0..4 {
                let mut w = |k: usize| unsafe { d.word_ptr(i).add(k).read_volatile() };
                assert_eq!(w(0), 0b1101, "valid for good: no write-back");
                assert_eq!(w(2), base + (i * WORDS / 4 * 4) as u32);
                assert_eq!(w(3), (WORDS / 4 * 4) as u32);
                assert_eq!(w(1), usb(1).d1fifo().addr() as u32);
            }
            let first = d.word_ptr(0) as u32;
            assert_eq!(unsafe { d.word_ptr(3).add(7).read_volatile() }, first);
        });
    }

    /// DREQE comes on with the pipe selected and the channel started, and the pipe answers after it.
    #[test]
    fn the_out_port_is_set_up_in_order() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_s, _) = out_stream();
        set_enabled(1, OUT_PIPE, true);
        let u = usb(1);
        let start = last(out_chctrl()).unwrap();
        let dreqe = last(u.d1fifosel().addr()).unwrap();
        let buf = last(pipectr(u, OUT_PIPE as usize).addr()).unwrap();
        assert!(start < dreqe && dreqe < buf);
        let log = test::log();
        let select = log
            .iter()
            .position(|a| {
                a.write && a.addr == u.d1fifosel().addr() && a.val & 0xF == u32::from(OUT_PIPE)
            })
            .unwrap();
        let deselect = log[..select]
            .iter()
            .rposition(|a| a.write && a.addr == u.d1fifosel().addr() && a.val & 0xF == 0)
            .unwrap();
        let bfre = log
            .iter()
            .position(|a| {
                a.write && a.addr == u.pipecfg().addr() && a.val & u32::from(PIPECFG_BFRE) != 0
            })
            .unwrap();
        assert!(
            bfre < select && deselect < select,
            "BFRE is set while the pipe is on no port"
        );
        let cleared = log[bfre..].iter().any(|a| {
            a.write
                && a.addr == pipectr(u, OUT_PIPE as usize).addr()
                && a.val as u16 & PIPECTR_ACLRM != 0
        });
        assert!(cleared, "the pipe is cleared after BFRE changes");
    }

    #[test]
    fn position_counts_across_wraps() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, base) = out_stream();
        assert_eq!(s.position(), 0, "nothing before it is enabled");
        set_enabled(1, OUT_PIPE, true);
        at(base, 0);
        assert_eq!(s.position(), 0);
        at(base, 10);
        assert_eq!(s.position(), 10);
        at(base, 60);
        assert_eq!(s.position(), 60);
        at(base, 4); // past the end and round
        assert_eq!(s.position(), 68);
        at(base, 4);
        assert_eq!(s.position(), 68, "nothing new");
        for _ in 0..10 {
            at(base, 40);
            s.position();
            at(base, 4);
            s.position();
        }
        assert_eq!(s.position(), 68 + 10 * 64);
        at(base, WORDS); // CRDA can read one past the end while the next descriptor loads
        assert_eq!(s.position(), 68 + 10 * 64 + 60);
    }

    #[test]
    fn enabling_again_starts_the_position_over() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, base) = out_stream();
        set_enabled(1, OUT_PIPE, true);
        at(base, 30);
        assert_eq!(s.position(), 30);
        at(base, 0);
        set_enabled(1, OUT_PIPE, true);
        assert_eq!(s.generation(), 2);
        assert_eq!(s.position(), 0);
    }

    #[test]
    fn disabling_an_out_endpoint_releases_d1() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (mut s, base) = out_stream();
        set_enabled(1, OUT_PIPE, true);
        at(base, 20);
        s.position();
        assert!(set_enabled(1, OUT_PIPE, false));
        let u = usb(1);
        assert_eq!(test::peek16(u.d1fifosel().addr()), 0);
        assert_eq!(
            test::peek16(u.pipecfg().addr()) & PIPECFG_BFRE,
            0,
            "BFRE off again"
        );
        assert_eq!(test::peek16(u.pipesel().addr()), 0);
        assert_eq!(test::peek32(out_chctrl()) & 1, 0, "stopped");
        at(base, 40);
        assert_eq!(s.position(), 20, "holds still");
        let log = test::log();
        let on = log
            .iter()
            .rposition(|a| {
                a.write && a.addr == u.d1fifosel().addr() && a.val & u32::from(FIFOSEL_DREQE) != 0
            })
            .unwrap();
        let off = on
            + log[on..]
                .iter()
                .position(|a| {
                    a.write
                        && a.addr == u.d1fifosel().addr()
                        && a.val & u32::from(FIFOSEL_DREQE) == 0
                        && a.val & 0xF == u32::from(OUT_PIPE)
                })
                .expect("DREQE off with the pipe still selected");
        let gone = log
            .iter()
            .rposition(|a| a.write && a.addr == u.d1fifosel().addr() && a.val == 0)
            .unwrap();
        assert!(off < gone);
    }

    #[test]
    fn a_bus_reset_stops_the_receive() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (s, _) = out_stream();
        set_enabled(1, OUT_PIPE, true);
        test::poke32(crate::pac::DMAC.ch(1).chstat().addr(), CHSTAT_EN);
        assert!(s.running());
        bus_reset(1);
        assert_eq!(test::peek32(out_chctrl()) & 1, 0);
        assert_eq!(test::peek16(usb(1).d1fifosel().addr()), 0);
        assert!(!s.running());
    }

    #[test]
    fn another_pipe_is_not_the_out_streams() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (s, _) = out_stream();
        assert!(!set_enabled(1, OUT_PIPE + 1, true));
        assert!(!set_enabled(0, OUT_PIPE, true));
        assert_eq!(s.generation(), 0);
        assert_eq!(test::peek32(out_chctrl()) & 1, 0);
    }

    #[test]
    fn the_two_streams_of_a_port_are_separate() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (out, _) = out_stream();
        let dma = unsafe { Dma::steal::<DMA_CH0>() };
        let into = bind_in(1, PIPE, dma, test_descs(8), START);
        test::poke16(usb(1).d0fifoctr().addr(), FIFOCTR_FRDY);
        assert!(set_enabled(1, OUT_PIPE, true));
        assert!(set_enabled(1, PIPE, true));
        assert_eq!(out.generation(), 1);
        assert_eq!(into.generation(), 1);
        assert!(set_enabled(1, PIPE, false));
        assert_ne!(
            test::peek16(usb(1).d1fifosel().addr()),
            0,
            "the other goes on"
        );
    }

    #[test]
    #[should_panic(expected = "D1FIFO")]
    fn a_second_out_stream_on_a_port_is_refused() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_s, _) = out_stream();
        let ring = UncachedMut::new(std::vec![0u32; WORDS].leak()).unwrap();
        let _second = bind_out(
            1,
            2,
            unsafe { Dma::steal::<DMA_CH1>() },
            ring,
            test_descs(4),
        );
    }

    #[test]
    #[should_panic(expected = "whole descriptors")]
    fn a_ring_that_is_not_whole_descriptors_is_refused() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        test::reset();
        let ring = UncachedMut::new(std::vec![0u32; 63].leak()).unwrap();
        let _ = bind_out(
            1,
            OUT_PIPE,
            unsafe { Dma::steal::<DMA_CH1>() },
            ring,
            test_descs(4),
        );
    }

    #[test]
    fn dropping_an_out_stream_frees_its_port() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        drop(out_stream());
        let _again = out_stream();
    }
}
