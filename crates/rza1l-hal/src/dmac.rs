//! Renesas RZ/A1L DMA Controller (DMAC) low-level register driver.
//!
//! The DMAC has 16 channels (0–15), split into two groups:
//!   - Channels  0–7: registers at `0xE820_0000 + ch * 64`
//!   - Channels 8–15: registers at `0xE820_0000 + ch * 64 + 0x200`
//!
//! Each channel has 16 × 32-bit registers; the stride between channels is
//! exactly 64 bytes (16 × 4).
//!
//! Group control registers:
//!   - `DCTRL_0_7`  at `0xE820_0300`
//!   - `DCTRL_8_15` at `0xE820_0700`
//!
//! DMA Resource Selector (DMARS) registers are at `0xFCFE_1000 + (ch/2) * 4`.
//! Each 32-bit DMARS register covers a channel pair:
//!   - bits `[15:0]`  → even channel of the pair
//!   - bits `[31:16]` → odd  channel of the pair
//!
//! (RZ/A1L TRM §9, register table: DMARS0=0xFCFE1000 … DMARS7=0xFCFE101C)

const DMAC_BASE: usize = 0xE820_0000;
const DMARS_BASE: usize = 0xFCFE_1000;

// ── Channel register byte offsets within one `st_dmac_n` block ──────────────
// Register-mode (N0/N1) source, destination, and byte-count registers.
const OFF_N0SA: usize = 0x00; // Next-0 source address
const OFF_N0DA: usize = 0x04; // Next-0 destination address
const OFF_N0TB: usize = 0x08; // Next-0 transfer byte count
const OFF_CRSA: usize = 0x18; // Current source address
const OFF_CRDA: usize = 0x1C; // Current destination address
const OFF_CHCTRL: usize = 0x28; // Channel control
const OFF_CHCFG: usize = 0x2C; // Channel config
const OFF_CHITVL: usize = 0x30; // Channel interval
const OFF_CHEXT: usize = 0x34; // Channel extension
const OFF_CHSTAT: usize = 0x24; // Channel status (TC = bit 6)
const OFF_NXLA: usize = 0x38; // Next link-descriptor address

// ── CHCTRL bits ──────────────────────────────────────────────────────────────
const CHCTRL_SETEN: u32 = 1 << 0; // Set enable  (start transfer)
const CHCTRL_CLREN: u32 = 1 << 1; // Clear enable (stop transfer)
const CHCTRL_SWRST: u32 = 1 << 3; // Software reset (clears status)
const CHCTRL_CLRTC: u32 = 1 << 6; // Clear terminal count (TC bit)

// ── CHSTAT bits ──────────────────────────────────────────────────────────────
/// CHSTAT bit 0: EN — the channel is enabled, i.e. a transfer has been started and not yet ended.
pub(crate) const CHSTAT_EN: u32 = 1 << 0;
/// CHSTAT bit 6: TC — Terminal Count flag set when transfer completes.
pub(crate) const CHSTAT_TC: u32 = 1 << 6;

// ── CHCFG register field constants (TRM §9.3.14; dmac_iobitmask.h) ───────────
/// CHCFG bit 31: DMS — DMA Mode Select (0 = register mode, 1 = link mode).
pub(crate) const CHCFG_DMS: u32 = 1 << 31;
/// CHCFG bit 24: DEM — DMA End interrupt Mask (1 = end interrupt suppressed).
pub(crate) const CHCFG_DEM: u32 = 1 << 24;
/// CHCFG bit 21: DAD — Destination Address Direction (1 = fixed / no increment).
/// Set when the destination is a peripheral register (e.g. SSI TX FIFO, SCUX FFD).
pub(crate) const CHCFG_DAD: u32 = 1 << 21;
/// CHCFG bit 20: SAD — Source Address Direction (1 = fixed / no increment).
/// Set when the source is a peripheral register (e.g. SSI RX FIFO, SCUX FFU).
pub(crate) const CHCFG_SAD: u32 = 1 << 20;
/// CHCFG bits [19:16]: DDS — Destination Data Size, value 2 = 4 bytes (32-bit).
pub(crate) const CHCFG_DDS_32BIT: u32 = 2 << 16;
/// CHCFG bits [15:12]: SDS — Source Data Size, value 2 = 4 bytes (32-bit).
pub(crate) const CHCFG_SDS_32BIT: u32 = 2 << 12;
/// CHCFG bits [10:8]: AM — Acknowledge Mode, value 2 = burst-transfer mode.
pub(crate) const CHCFG_AM_BURST: u32 = 2 << 8;
/// CHCFG bit 5: HIEN — High Enable.
/// Selects the DMA request signal edge detected: rising edge (LVL=0) or High level (LVL=1).
pub(crate) const CHCFG_HIEN: u32 = 1 << 5;
/// CHCFG bit 6: LVL — Level-triggered DMA (1 = level, 0 = edge).
/// BSP: `DMA_LVL_FOR_SSI = (1 << 6)` in `cpu_specific.h`.
pub(crate) const CHCFG_LVL: u32 = 1 << 6;
/// CHCFG bit 3: REQD — Request Direction (1 = dest-select, 0 = src-select).
/// Set for TX (destination peripheral requests); clear for RX (source peripheral).
pub(crate) const CHCFG_REQD: u32 = 1 << 3;

// ── GIC priority for DMAC completion interrupts ───────────────────────────────
/// GIC priority assigned to DMAC completion (DMAINT) interrupts.
/// Matches the original C firmware value for OLED DMA.
const DMAC_IRQ_PRIORITY: u8 = 13;

// ── DCTRL group-control registers ───────────────────────────────────────────
const DCTRL_0_7: usize = DMAC_BASE + 0x300;
const DCTRL_8_15: usize = DMAC_BASE + 0x700;

// ── Helpers ──────────────────────────────────────────────────────────────────

#[inline]
fn ch_base(ch: u8) -> usize {
    DMAC_BASE + (ch as usize) * 64 + if ch >= 8 { 0x200 } else { 0 }
}

#[inline]
fn ch_reg(ch: u8, off: usize) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(ch_base(ch) + off)
}

#[inline]
fn dctrl_reg(ch: u8) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(if ch < 8 { DCTRL_0_7 } else { DCTRL_8_15 })
}

#[inline]
fn dmars_reg(ch: u8) -> crate::mmio::Reg32 {
    crate::mmio::Reg32(DMARS_BASE + (ch as usize / 2) * 4)
}

// ── Public API ───────────────────────────────────────────────────────────────

// ── Register-mode (one-shot) DMA ─────────────────────────────────────────────
//
// Used for memory→peripheral block transfers where the transfer size is known
// in advance (e.g. OLED SPI TX).  Unlike link-descriptor mode, the channel
// stops after one transfer and can be re-armed by calling `start_transfer`
// again.  Completion is signalled via a GIC interrupt (DMAINT_n, GIC IDs
// 41–56 for channels 0–15).

#[doc(hidden)]
pub mod raw;

// The table handlers and tests below call the channel-number functions by name.
use raw::*;

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Poll;
use embassy_sync::waitqueue::AtomicWaker;

use core::marker::PhantomData;

use crate::Peri;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// A DMAC channel singleton (`peripherals::DMA_CH0`–`DMA_CH15`).
pub trait Channel: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number, 0–15.
    const NUMBER: u8;
    /// Its completion interrupt (DMAINT<NUMBER>).
    type Interrupt: Interrupt;
}

macro_rules! channels {
    ($($ch:ident = $n:literal, $irq:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$ch {}
        impl Channel for crate::peripherals::$ch {
            const NUMBER: u8 = $n;
            type Interrupt = crate::interrupt::typelevel::$irq;
        }
    )*};
}

channels! {
    DMA_CH0 = 0, DMAINT0; DMA_CH1 = 1, DMAINT1; DMA_CH2 = 2, DMAINT2; DMA_CH3 = 3, DMAINT3;
    DMA_CH4 = 4, DMAINT4; DMA_CH5 = 5, DMAINT5; DMA_CH6 = 6, DMAINT6; DMA_CH7 = 7, DMAINT7;
    DMA_CH8 = 8, DMAINT8; DMA_CH9 = 9, DMAINT9; DMA_CH10 = 10, DMAINT10; DMA_CH11 = 11, DMAINT11;
    DMA_CH12 = 12, DMAINT12; DMA_CH13 = 13, DMAINT13; DMA_CH14 = 14, DMAINT14; DMA_CH15 = 15, DMAINT15;
}

/// Channel `C`'s completion interrupt handler: wakes [`Dma::wait_transfer_complete`].
pub struct InterruptHandler<C: Channel> {
    _channel: PhantomData<C>,
}

impl<C: Channel> Handler<C::Interrupt> for InterruptHandler<C> {
    unsafe fn on_interrupt() {
        raw::on_dma_int(C::NUMBER);
    }
}

/// An owned DMAC channel.
///
/// The methods that take bus addresses stay `unsafe`: the DMAC writes wherever it is told.
/// Owning the channel is what the caller no longer has to promise.
pub struct Dma<'d> {
    ch: u8,
    _channel: PhantomData<&'d mut ()>,
}

impl<'d> Dma<'d> {
    /// Take channel `C`, install its completion handler, and enable its interrupt.
    pub fn new<C: Channel>(
        _channel: Peri<'d, C>,
        irq: impl Binding<C::Interrupt, InterruptHandler<C>> + 'd,
    ) -> Self {
        interrupt::bind(irq);
        // SAFETY: this channel's own interrupt, at the DMAC priority raw::register_completion_irq uses.
        unsafe {
            crate::gic::set_priority(C::Interrupt::ID, DMAC_IRQ_PRIORITY);
            crate::gic::enable(C::Interrupt::ID);
        }
        Self {
            ch: C::NUMBER,
            _channel: PhantomData,
        }
    }

    /// Take channel `C` for a transfer nobody awaits (a polled ring): its completion interrupt is
    /// masked at the GIC and nothing is bound to it. A ring with DEM clear raises DMAINT on every
    /// pass, so an enabled line with no one clearing TC would storm.
    pub fn new_without_interrupt<C: Channel>(_channel: Peri<'d, C>) -> Self {
        let mut dma = Self {
            ch: C::NUMBER,
            _channel: PhantomData,
        };
        dma.mask_interrupt();
        dma
    }

    /// Channel `C` without its singleton, for code that can't reach the channel's owner: a fault
    /// handler draining a transfer, or a loader stopping DMA before it hands over the chip.
    /// Touches no register and binds no interrupt.
    ///
    /// # Safety
    /// The channel's owner must not run while the stolen handle is in use (interrupts masked, or
    /// the owner gone for good).
    pub unsafe fn steal<C: Channel>() -> Dma<'static> {
        Dma {
            ch: C::NUMBER,
            _channel: PhantomData,
        }
    }

    /// The channel number.
    pub fn number(&self) -> u8 {
        self.ch
    }

    /// Mask the channel's completion interrupt at the GIC, for a channel that never awaits
    /// completion (a circular ring read by polling).
    pub fn mask_interrupt(&mut self) {
        // SAFETY: this owned channel's own interrupt.
        unsafe { crate::gic::disable(DMAINT_BASE + self.ch as u16) }
    }

    /// See [`raw::init_register_mode`].
    ///
    /// # Safety
    /// `dst` is the bus address every transfer writes to.
    pub unsafe fn init_register_mode(&mut self, chcfg: u32, dst: u32, dmars: u32) {
        unsafe { raw::init_register_mode(self.ch, chcfg, dst, dmars) }
    }

    /// See [`raw::init_register_mode_rx`].
    ///
    /// # Safety
    /// `src` is the bus address every transfer reads from.
    pub unsafe fn init_register_mode_rx(&mut self, chcfg: u32, src: u32, dmars: u32) {
        unsafe { raw::init_register_mode_rx(self.ch, chcfg, src, dmars) }
    }

    /// See [`raw::init_with_link_descriptor`].
    ///
    /// # Safety
    /// `descriptor` is the bus address of a 32-byte-aligned, 8-word link descriptor that stays
    /// valid, with every descriptor it links to, for as long as the channel runs.
    pub unsafe fn init_with_link_descriptor(&mut self, descriptor: *const u32, dmars: u32) {
        unsafe { raw::init_with_link_descriptor(self.ch, descriptor, dmars) }
    }

    /// See [`raw::channel_start`].
    ///
    /// # Safety
    /// [`init_with_link_descriptor`](Self::init_with_link_descriptor) has run, and its
    /// descriptors' buffers are valid and unaliased while the channel runs.
    pub unsafe fn channel_start(&mut self) {
        unsafe { raw::channel_start(self.ch) }
    }

    /// See [`raw::start_transfer`].
    ///
    /// # Safety
    /// `src..src + count` must stay valid and unaliased until the transfer completes.
    pub unsafe fn start_transfer(&mut self, src: u32, count: u32) {
        unsafe { raw::start_transfer(self.ch, src, count) }
    }

    /// See [`raw::start_transfer_rx`].
    ///
    /// # Safety
    /// `dst..dst + count` must stay valid and unaliased until the transfer completes.
    pub unsafe fn start_transfer_rx(&mut self, dst: u32, count: u32) {
        unsafe { raw::start_transfer_rx(self.ch, dst, count) }
    }

    /// Wait for the transfer in flight to complete.
    pub async fn wait_transfer_complete(&mut self) {
        raw::wait_transfer_complete(self.ch).await
    }

    /// Stop the channel and release its peripheral-request route.
    pub fn stop(&mut self) {
        // SAFETY: this channel is owned, so no other code drives it.
        unsafe { raw::stop(self.ch) }
    }

    /// Whether a transfer is in flight.
    pub fn transfer_in_flight(&self) -> bool {
        // SAFETY: a read of this owned channel's status.
        unsafe { raw::transfer_in_flight(self.ch) }
    }

    /// The current source address.
    pub fn current_src(&self) -> u32 {
        // SAFETY: a read of this owned channel's register.
        unsafe { raw::current_src(self.ch) }
    }

    /// The current destination address.
    pub fn current_dst(&self) -> u32 {
        // SAFETY: a read of this owned channel's register.
        unsafe { raw::current_dst(self.ch) }
    }
}

/// Stop every channel and release its request route: [`Dma::stop`] for channels 0–15.
///
/// For a loader handing the chip to another program, where a circular channel (a UART receive
/// ring) would keep writing into memory the next program owns.
///
/// # Safety
/// Every channel's transfer ends; no `Dma` may be used afterwards.
pub unsafe fn stop_all() {
    for ch in 0..16 {
        unsafe { raw::stop(ch) };
    }
}

/// Channel `C`'s per-block interrupt, for a descriptor ring whose descriptors each end with an
/// interrupt: clears TC without stopping the ring and wakes [`BlockDma::wait_block`].
pub struct BlockInterruptHandler<C: Channel> {
    _channel: PhantomData<C>,
}

impl<C: Channel> Handler<C::Interrupt> for BlockInterruptHandler<C> {
    unsafe fn on_interrupt() {
        on_block_int(C::NUMBER);
    }
}

/// An owned DMAC channel running a descriptor ring with a per-block interrupt. It derefs to
/// its [`Dma`], which programs the ring.
pub struct BlockDma<'d> {
    dma: Dma<'d>,
}

impl<'d> BlockDma<'d> {
    /// Take channel `C`, install its block handler, and enable its interrupt: before the ring
    /// starts, so no block's interrupt is lost.
    pub fn new<C: Channel>(
        _channel: Peri<'d, C>,
        irq: impl Binding<C::Interrupt, BlockInterruptHandler<C>> + 'd,
    ) -> Self {
        interrupt::bind(irq);
        // SAFETY: this channel's own interrupt, at the DMAC priority.
        unsafe {
            crate::gic::set_priority(C::Interrupt::ID, DMAC_IRQ_PRIORITY);
            crate::gic::enable(C::Interrupt::ID);
        }
        Self {
            dma: Dma {
                ch: C::NUMBER,
                _channel: PhantomData,
            },
        }
    }

    /// Wait for the next block to complete. Recurring: call it in a loop. Takes `&self`, so a
    /// ring kept in a static can be waited on.
    pub async fn wait_block(&self) {
        raw::wait_block(self.dma.ch).await
    }
}

impl<'d> core::ops::Deref for BlockDma<'d> {
    type Target = Dma<'d>;
    fn deref(&self) -> &Dma<'d> {
        &self.dma
    }
}

impl<'d> core::ops::DerefMut for BlockDma<'d> {
    fn deref_mut(&mut self) -> &mut Dma<'d> {
        &mut self.dma
    }
}

/// GIC interrupt ID base for DMAC completion interrupts.
/// DMAINT0 = GIC ID 41; channels 0–15 map to IDs 41–56 (RZ/A1L TRM §A.2).
const DMAINT_BASE: u16 = 41;

/// Per-channel waker for DMA completion interrupts.
static DMAC_WAKERS: [AtomicWaker; 16] = [
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
    AtomicWaker::new(),
];

// Per-channel ISR dispatch table (16 entries; only populated channels matter).
fn dma_int_0() {
    on_dma_int(0);
}
fn dma_int_1() {
    on_dma_int(1);
}
fn dma_int_2() {
    on_dma_int(2);
}
fn dma_int_3() {
    on_dma_int(3);
}
fn dma_int_4() {
    on_dma_int(4);
}
fn dma_int_5() {
    on_dma_int(5);
}
fn dma_int_6() {
    on_dma_int(6);
}
fn dma_int_7() {
    on_dma_int(7);
}
fn dma_int_8() {
    on_dma_int(8);
}
fn dma_int_9() {
    on_dma_int(9);
}
fn dma_int_10() {
    on_dma_int(10);
}
fn dma_int_11() {
    on_dma_int(11);
}
fn dma_int_12() {
    on_dma_int(12);
}
fn dma_int_13() {
    on_dma_int(13);
}
fn dma_int_14() {
    on_dma_int(14);
}
fn dma_int_15() {
    on_dma_int(15);
}

type HandlerFn = fn();
static DMA_INT_HANDLERS: [HandlerFn; 16] = [
    dma_int_0, dma_int_1, dma_int_2, dma_int_3, dma_int_4, dma_int_5, dma_int_6, dma_int_7,
    dma_int_8, dma_int_9, dma_int_10, dma_int_11, dma_int_12, dma_int_13, dma_int_14, dma_int_15,
];

// ── Recurring per-block (descriptor-ring) interrupt support ────────────────────
//
// For a channel running a ring of link descriptors (e.g. the SSI RX block ring),
// each descriptor completion raises DMAINT. Unlike the one-shot path, the ISR must
// clear TC (CHCTRL.CLRTC — *without* SWRST, so the ring keeps running) to deassert
// / re-arm the interrupt, then signal a per-channel "block ready" flag the audio
// loop awaits via [`wait_block`].

/// Per-channel "a block completed" flag, set by the block ISR, consumed by
/// [`wait_block`].
static DMAC_BLOCK_READY: [AtomicBool; 16] = [const { AtomicBool::new(false) }; 16];

/// Block-ring ISR body: clear TC to re-arm (channel keeps running), flag the
/// block, and wake the awaiter.
fn on_block_int(ch: u8) {
    // CHCTRL bits are write-1-to-trigger; write a clean value (not RMW).
    unsafe { ch_reg(ch, OFF_CHCTRL).write(CHCTRL_CLRTC) };
    DMAC_BLOCK_READY[ch as usize].store(true, Ordering::Release);
    DMAC_WAKERS[ch as usize].wake();
}

macro_rules! block_handlers {
    ($($name:ident = $ch:literal),* $(,)?) => {
        $( fn $name() { on_block_int($ch); } )*
        static DMA_BLOCK_HANDLERS: [HandlerFn; 16] = [ $($name),* ];
    };
}
block_handlers!(
    dbi_0 = 0,
    dbi_1 = 1,
    dbi_2 = 2,
    dbi_3 = 3,
    dbi_4 = 4,
    dbi_5 = 5,
    dbi_6 = 6,
    dbi_7 = 7,
    dbi_8 = 8,
    dbi_9 = 9,
    dbi_10 = 10,
    dbi_11 = 11,
    dbi_12 = 12,
    dbi_13 = 13,
    dbi_14 = 14,
    dbi_15 = 15,
);

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    // DMAC_BASE = 0xE820_0000. Channels 0–7 are 64 bytes apart from the base;
    // channels 8–15 sit 0x200 higher (a register gap), i.e. base+0x400 for ch8.
    #[test]
    fn ch_base_layout() {
        assert_eq!(ch_base(0) as usize, 0xE820_0000);
        assert_eq!(ch_base(1) as usize, 0xE820_0040);
        assert_eq!(ch_base(7) as usize, 0xE820_01C0);
        // The 0x200 gap kicks in at channel 8.
        assert_eq!(ch_base(8) as usize, 0xE820_0400);
        assert_eq!(ch_base(9) as usize, 0xE820_0440);
        assert_eq!(ch_base(15) as usize, 0xE820_05C0);
    }

    #[test]
    fn ch_base_stride_is_uniform_within_each_group() {
        for ch in 0..7u8 {
            assert_eq!(ch_base(ch + 1) - ch_base(ch), 64);
        }
        for ch in 8..15u8 {
            assert_eq!(ch_base(ch + 1) - ch_base(ch), 64);
        }
    }

    #[test]
    fn ch_reg_adds_offset() {
        assert_eq!(ch_reg(0, OFF_CHCFG).0, 0xE820_0000 + 0x2C);
        assert_eq!(ch_reg(8, OFF_NXLA).0, 0xE820_0400 + 0x38);
        assert_eq!(ch_reg(5, OFF_CHSTAT).0, ch_base(5) + OFF_CHSTAT);
    }

    #[test]
    fn dctrl_selects_group_register() {
        // Channels 0–7 share one DCTRL; 8–15 share another.
        for ch in 0..8u8 {
            assert_eq!(dctrl_reg(ch).0, 0xE820_0300);
        }
        for ch in 8..16u8 {
            assert_eq!(dctrl_reg(ch).0, 0xE820_0700);
        }
    }

    #[test]
    fn dmars_register_is_shared_per_channel_pair() {
        // DMARS holds two channels' resource selectors per 32-bit word.
        assert_eq!(dmars_reg(0).0, 0xFCFE_1000);
        assert_eq!(dmars_reg(1).0, 0xFCFE_1000);
        assert_eq!(dmars_reg(2).0, 0xFCFE_1004);
        assert_eq!(dmars_reg(3).0, 0xFCFE_1004);
        assert_eq!(dmars_reg(14).0, 0xFCFE_101C);
        assert_eq!(dmars_reg(15).0, 0xFCFE_101C);
    }

    #[test]
    fn chctrl_bits_are_distinct() {
        // Sanity: the start/stop/reset/clear-TC controls don't overlap.
        let all = CHCTRL_SETEN | CHCTRL_CLREN | CHCTRL_SWRST | CHCTRL_CLRTC;
        assert_eq!(all.count_ones(), 4);
    }

    /// `init_with_link_descriptor` (even channel) writes DCTRL=0, CHCFG from
    /// descriptor[4], the DMARS low half, and NXLA = descriptor address.
    #[test]
    fn init_link_descriptor_even_channel() {
        crate::mmio::test::reset();
        // An 8-word link descriptor; word [4] is the CHCFG value.
        let desc: [u32; 8] = [0, 0, 0, 0, 0xABCD_1234, 0, 0, 0];
        let ch = 6u8; // even
        unsafe { init_with_link_descriptor(ch, desc.as_ptr(), 0x00E1) };

        assert_eq!(
            crate::mmio::test::peek32(dctrl_reg(ch).0),
            0,
            "DCTRL cleared"
        );
        assert_eq!(
            crate::mmio::test::peek32(ch_reg(ch, OFF_CHCFG).0),
            0xABCD_1234,
            "CHCFG from desc[4]"
        );
        // Even channel → DMARS resource selector in the low 16 bits.
        assert_eq!(crate::mmio::test::peek32(dmars_reg(ch).0), 0x0000_00E1);
        assert_eq!(
            crate::mmio::test::peek32(ch_reg(ch, OFF_NXLA).0),
            desc.as_ptr() as u32,
            "NXLA = descriptor address"
        );
    }

    /// Odd channels place the DMARS selector in the high 16 bits, preserving
    /// the even sibling's low half (they share one 32-bit DMARS word).
    #[test]
    fn init_link_descriptor_odd_channel_uses_high_dmars_half() {
        crate::mmio::test::reset();
        // Channel 6 (even) and 7 (odd) share dmars_reg.
        let dmars = dmars_reg(7).0;
        crate::mmio::test::poke32(dmars, 0x0000_00E1); // ch6 already set the low half
        let desc = [0u32; 8];
        unsafe { init_with_link_descriptor(7, desc.as_ptr(), 0x00E2) };
        assert_eq!(
            crate::mmio::test::peek32(dmars),
            0x00E2_00E1,
            "odd in high, even preserved"
        );
    }

    /// `stop` must both software-reset the channel and release its DMARS
    /// request route, so a latched peripheral route (e.g. SCIF1-RX = 0x66)
    /// cannot keep claiming its request line after handoff.
    #[test]
    fn stop_clears_chctrl_and_dmars() {
        crate::mmio::test::reset();
        let ch = 5u8;
        // Seed a live channel-control value and a latched SCIF1-RX route in
        // this odd channel's (high) half of DMARS.
        crate::mmio::test::poke32(ch_reg(ch, OFF_CHCTRL).0, CHCTRL_SETEN);
        crate::mmio::test::poke32(dmars_reg(ch).0, 0x0066_0000);
        unsafe { stop(ch) };
        // Last CHCTRL write is the software reset; DMARS is released.
        assert_eq!(
            crate::mmio::test::peek32(ch_reg(ch, OFF_CHCTRL).0),
            CHCTRL_SWRST
        );
        assert_eq!(crate::mmio::test::peek32(dmars_reg(ch).0), 0);
    }

    /// Channels share DMARS in pairs; stopping one must leave its partner's
    /// route in place.
    #[test]
    fn stop_keeps_the_paired_channels_route() {
        crate::mmio::test::reset();
        let dmars = dmars_reg(4).0;
        crate::mmio::test::poke32(dmars, 0x0062_0066); // ch5 = 0x62, ch4 = 0x66
        unsafe { stop(4) };
        assert_eq!(crate::mmio::test::peek32(dmars), 0x0062_0000, "ch5 kept");
        crate::mmio::test::poke32(dmars, 0x0062_0066);
        unsafe { stop(5) };
        assert_eq!(crate::mmio::test::peek32(dmars), 0x0000_0066, "ch4 kept");
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod typed_tests {
    use super::*;
    use crate::interrupt::{Handler, Interrupt};
    use crate::mmio::test;
    use crate::peripherals::{
        DMA_CH0, DMA_CH3, DMA_CH4, DMA_CH5, DMA_CH7, DMA_CH9, DMA_CH10, DMA_CH13, DMA_CH15,
    };
    use core::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::task::{Wake, Waker};

    crate::bind_interrupts!(struct Irqs {
        DMAINT4 => InterruptHandler<DMA_CH4>;
        DMAINT7 => InterruptHandler<DMA_CH7>;
    });

    #[test]
    fn channels_know_their_number_and_interrupt() {
        assert_eq!(<DMA_CH0 as Channel>::NUMBER, 0);
        assert_eq!(<DMA_CH15 as Channel>::NUMBER, 15);
        assert_eq!(<<DMA_CH9 as Channel>::Interrupt as Interrupt>::ID, 41 + 9);
    }

    #[test]
    fn a_channel_is_bound_prioritised_and_enabled_when_built() {
        test::reset();
        let dma = Dma::new(unsafe { DMA_CH4::steal() }, Irqs);
        assert_eq!(dma.number(), 4);
        assert!(crate::gic::handler(45).is_some(), "DMAINT4 has a handler");
        // Priority 13 into IPRIORITYR11 lane 1, then ISENABLER1 bit 13: what
        // raw::register_completion_irq(4) writes.
        assert_eq!(
            test::writes(),
            [(0xE820_142C, (13 << 3) << 8), (0xE820_1104, 1 << 13)]
        );
    }

    #[test]
    fn stopping_an_owned_channel_writes_what_raw_stop_writes() {
        test::reset();
        unsafe { raw::stop(7) };
        let raw_writes = test::writes();

        let mut dma = Dma::new(unsafe { DMA_CH7::steal() }, Irqs);
        test::reset();
        dma.stop();
        assert_eq!(test::writes(), raw_writes);
    }

    struct CountWakes(AtomicU32);
    impl Wake for CountWakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn the_completion_handler_wakes_its_channel() {
        let wakes = Arc::new(CountWakes(AtomicU32::new(0)));
        DMAC_WAKERS[4].register(&Waker::from(wakes.clone()));
        unsafe { <InterruptHandler<DMA_CH4> as Handler<_>>::on_interrupt() };
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn a_stolen_channel_touches_no_register() {
        test::reset();
        let dma = unsafe { Dma::steal::<DMA_CH10>() };
        assert_eq!(dma.number(), 10);
        assert!(test::log().is_empty());
    }

    #[test]
    fn stop_all_stops_every_channel_as_raw_stop_does() {
        test::reset();
        for ch in 0..16 {
            unsafe { raw::stop(ch) };
        }
        let one_by_one = test::writes();
        test::reset();
        unsafe { stop_all() };
        assert_eq!(test::writes(), one_by_one);
    }

    #[test]
    fn an_owned_channel_starts_a_link_descriptor_as_raw_does() {
        let desc: [u32; 8] = [0, 0, 0, 0, 0x8110_0265, 0, 0, 0];
        test::reset();
        unsafe {
            raw::init_with_link_descriptor(13, desc.as_ptr(), 0x62);
            raw::channel_start(13);
        }
        let raw_writes = test::writes();

        let mut dma = unsafe { Dma::steal::<DMA_CH13>() };
        test::reset();
        unsafe {
            dma.init_with_link_descriptor(desc.as_ptr(), 0x62);
            dma.channel_start();
        }
        assert_eq!(test::writes(), raw_writes);
    }

    #[test]
    fn register_mode_setup_goes_through_the_seam() {
        test::reset();
        unsafe { raw::init_register_mode(4, 0x0020_026C, 0xE800_C804, 0x121) };
        // Channel 4's block is at 0xE820_0100: DCTRL_0_7; CHCFG, CHITVL, CHEXT; N0DA; CHCTRL
        // (SWRST | CLRTC); N0DA again; DMARS2's low half.
        assert_eq!(
            test::writes(),
            [
                (0xE820_0300, 0),
                (0xE820_012C, 0x0020_026C),
                (0xE820_0130, 0),
                (0xE820_0134, 0),
                (0xE820_0104, 0xE800_C804),
                (0xE820_0128, (1 << 3) | (1 << 6)),
                (0xE820_0104, 0xE800_C804),
                (0xFCFE_1008, 0x121),
            ]
        );
    }

    crate::bind_interrupts!(struct BlockIrqs {
        DMAINT5 => BlockInterruptHandler<DMA_CH5>;
    });

    #[test]
    fn a_channel_without_interrupt_masks_its_line_and_binds_nothing() {
        test::reset();
        let dma = Dma::new_without_interrupt(unsafe { DMA_CH3::steal() });
        assert_eq!(dma.number(), 3);
        // GICD_ICENABLER1 (0xE820_1184), bit 44 - 32 = 12; nothing set-enabled, no handler.
        assert_eq!(test::writes(), [(0xE820_1184, 1 << 12)]);
        assert!(crate::gic::handler(44).is_none());
    }

    #[test]
    fn a_block_interrupt_clears_tc_and_wakes_the_waiter_once() {
        test::reset();
        let ring = BlockDma::new(unsafe { DMA_CH5::steal() }, BlockIrqs);
        assert_eq!(ring.number(), 5, "derefs to its Dma");
        assert!(crate::gic::handler(46).is_some(), "DMAINT5 bound");
        // Priority 13 into IPRIORITYR11 lane 2, then ISENABLER1 bit 14.
        assert_eq!(
            test::writes(),
            [(0xE820_142C, (13 << 3) << 16), (0xE820_1104, 1 << 14)]
        );

        test::reset();
        unsafe { <BlockInterruptHandler<DMA_CH5> as Handler<_>>::on_interrupt() };
        // CHCTRL (channel 5 at 0xE820_0140 + 0x28) gets CLRTC only: the ring keeps running.
        assert_eq!(test::writes(), [(0xE820_0168, 1 << 6)]);
        embassy_futures::block_on(ring.wait_block());
        // Consumed: a second wait would pend. Poll it once to show it isn't ready.
        let mut second = core::pin::pin!(ring.wait_block());
        let waker = Waker::from(Arc::new(CountWakes(AtomicU32::new(0))));
        let mut cx = core::task::Context::from_waker(&waker);
        assert!(second.as_mut().poll(&mut cx).is_pending());
    }
}
