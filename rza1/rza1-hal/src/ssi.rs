//! Serial Sound Interface (SSIF): I²S over lent DMA rings.
//!
//! [`Ssi`] builds an owned driver on SSIF0, SSIF1 or SSIF3 from the channel, its pins, a
//! [`RingBuf`] per direction and a [`Config`]:
//! - [`Ssi::new`]: full duplex, an [`SsiTx`] ([`WritableRing`]) and an [`SsiRx`]
//!   ([`ReadableRing`]);
//! - [`Ssi::new_rx_only`]: receive only;
//! - [`Ssi::new_with_scux_tx`]: receive, with transmit fed by a SCUX path
//!   ([`crate::scux::Channel::into_ssi_path`]) through a [`ScuxOutput`].
//!
//! Frames are stereo, one 32-bit word per channel. The rings are the caller's (uncached memory,
//! checked by [`crate::dma_buf::UncachedMut`]); each DMA block ends with an interrupt, which the
//! ring's [`BlockInterruptHandler`](crate::dmac::BlockInterruptHandler) binding services.
//!
//! The data pins use the plain alternate function (PIPC = 1), which runs on hardware; HW manual
//! Table 41.7 asks PIPC = 0 for SSITxD0/1/3, which no bench run has tried yet.
//!
//! The master clock pin (AUDIO_XOUT) and a codec's power are the board's. SSIF2, which shares
//! one data pin and one DMA request between directions, isn't supported yet.
//!
//! ```ignore
//! let (tx, rx) = ssi::Ssi::new(
//!     p.SSIF0, p.P6_8, p.P6_9, p.P6_10, p.P6_11,
//!     RingBuf { dma: BlockDma::new(p.DMA_CH6, Irqs), buf: tx_ring, blocks: 8 },
//!     RingBuf { dma: BlockDma::new(p.DMA_CH7, Irqs), buf: rx_ring, blocks: 16 },
//!     ssi::Config::default(),
//! );
//! ```

use core::marker::PhantomData;

use crate::Peri;
use crate::pac::CPG;
use crate::pac::cpg::regs::Swrstcr1;

// ── CHCFG register field constants (shared with scux; defined in dmac) ───────
use crate::dmac::{
    CHCFG_AM_BURST, CHCFG_DAD, CHCFG_DDS_32BIT, CHCFG_DMS, CHCFG_HIEN, CHCFG_LVL, CHCFG_REQD,
    CHCFG_SAD, CHCFG_SDS_32BIT,
};

// ── SSI0 register map ────────────────────────────────────────────────────────

// SSICR initialisation value — from `src/RZA1/ssi/drv_ssif_user.h`
// `SSI_SSICR0_USER_INIT_VALUE`:
//   - CKS=0  (bit 30)   : AUDIO_X1 clock source
//   - CHNL=00 (23:22)  : 1 channel pair (stereo)
//   - DWL=101 (21:19)  : 24-bit data word  [SSI_SSICR0_DWL_VALUE = 0x00280000]
//   - SWL=011 (18:16)  : 32-bit system word [SSI_SSICR0_SWL_VALUE = 0x00030000]
//   - SCKD=1  (bit 15) : RZ/A1L generates BCLK (master)
//   - SWSD=1  (bit 14) : RZ/A1L generates LRCK (master)
//   - SCKP=SWSP=SPDP=SDTA=PDTA=DEL=0 (all defaults)
//   - CKDV=0010 (7:4)  : AUDIO_X1 ÷ 4 = 5.6448 MHz BCLK
//
// [`Config::default`] encodes 0x002B_C020 (DWL bits [21:19] = 0b101 = 24-bit; DWL=0b111 is
// prohibited).

/// SSIFCR bit positions (verified against vendor/rbsp ssif.h SSIF_FCR_SHIFT_*).
const SSIFCR_RFRST: u32 = 1 << 0; // RX FIFO reset (active high)
const SSIFCR_TFRST: u32 = 1 << 1; // TX FIFO reset (active high)
const SSIFCR_RIE: u32 = 1 << 2; // RX interrupt enable (→ RXI DMA trigger)
const SSIFCR_TIE: u32 = 1 << 3; // TX interrupt enable (→ TXI DMA trigger)
// RTRG[1:0] = bits 5:4 — RX trigger level
// TTRG[1:0] = bits 7:6 — TX trigger level

/// SSICR bit positions (verified against vendor/rbsp ssif.h SSIF_CR_SHIFT_*).
const SSICR_REN: u32 = 1 << 0; // Receive enable
const SSICR_TEN: u32 = 1 << 1; // Transmit enable

/// SSITDMR bit 8: WS continue mode.
/// BSP (scux_dev.c): set for master mode — keeps WS clock running between frames.
/// Without this the WS line idles low between frames and the codec loses sync.
/// BSP: SCUX_SSITDMR_CONT_SET = (1U << 8)  (ssif.h: SSIF_TDMR_BIT_CONT)
const SSITDMR_CONT: u32 = 1 << 8;

/// SSIFCR bits [7:6]: TTRG — TX trigger level (0b10 = assert DMA request when ≥ 4 TX FIFO
/// stages are empty).  BSP: `SSI_SSIFCR_TTRG_INIT_VALUE = 0x000000A0`.
const SSIFCR_TTRG_4: u32 = 0b10 << 6; // = 0x80
/// SSIFCR bits [5:4]: RTRG — RX trigger level (0b10 = assert DMA request when ≥ 4 RX FIFO
/// stages have been filled).
const SSIFCR_RTRG_4: u32 = 0b10 << 4; // = 0x20

/// SSIFCR initialisation value: both FIFOs held in reset, TX/RX triggers set to 4
/// stages, interrupts disabled.  Mirrors `SSI_SSIFCR_BASE_INIT_VALUE` from `drv_ssif.h`.
///   TFRST=1 | RFRST=1 | TTRG=0b10 (bits [7:6]) | RTRG=0b10 (bits [5:4]) | TIE=0 | RIE=0
///   = 0x02 | 0x01 | 0x80 | 0x20 = 0xA3
const SSIFCR_INIT: u32 = SSIFCR_TFRST | SSIFCR_RFRST | SSIFCR_TTRG_4 | SSIFCR_RTRG_4;

/// CPG SWRSTCR1 bits 6 to 3: the software resets of SSIF0 to SSIF3.
#[cfg(any(target_os = "none", test))]
const SWRSTCR1_SSI_ALL: u8 = 0b0111_1000;

/// Software-reset all four SSI channels, ending any transfer a warm start left running.
///
/// # Safety
/// Writes CPG SWRSTCR1. No SSI channel may be in use.
#[cfg(any(target_os = "none", test))]
pub(crate) unsafe fn reset_all() {
    let swrstcr = CPG.swrstcr1();
    swrstcr.write_value(Swrstcr1(swrstcr.read().0 | SWRSTCR1_SSI_ALL));
    let _ = swrstcr.read(); // mandatory dummy read
    swrstcr.write_value(Swrstcr1(swrstcr.read().0 & !SWRSTCR1_SSI_ALL));
    let _ = swrstcr.read(); // mandatory dummy read
}

// CHCFG values for the two SSI DMA channels, composed from named field constants.
// For reference, the C BSP `drivers/ssi/ssi.c` link-descriptor initialisers
// (which also set DEM, masking the end interrupt; `tx_chcfg`/`rx_chcfg` leave
// it clear):
//
//   TX: DMS | DEM | DAD | DDS_32BIT | SDS_32BIT | AM_BURST | HIEN | REQD | LVL | sel
//       = 0x8000_0000 | 0x0100_0000 | 0x0020_0000 | 0x0002_0000 | 0x0000_2000
//         | 0x0000_0200 | 0x0000_0020 | 0x0000_0008 | 0x0000_0040 | ch
//       = 0x8122_2268 | ch  (ch=6 → 0x8122_226E)
//   RX: DMS | DEM | SAD | DDS_32BIT | SDS_32BIT | AM_BURST | HIEN | LVL | sel
//       = 0x8000_0000 | 0x0100_0000 | 0x0010_0000 | 0x0002_0000 | 0x0000_2000
//         | 0x0000_0200 | 0x0000_0020 | 0x0000_0040 | ch
//       = 0x8112_2260 | ch  (ch=7 → 0x8112_2267)

/// Compute CHCFG word for an SSI TX DMA channel (SRAM → SSIFTDR).
/// Source address increments (SAD=0); destination is fixed SSIFTDR (DAD=1).
/// Trigger: destination-select (REQD=1) — SSI TX FIFO drives the request.
/// DEM is cleared, so DMAINT fires once per ring block.
#[inline(always)]
const fn tx_chcfg(dma_ch: u8) -> u32 {
    CHCFG_DMS
        | CHCFG_DAD
        | CHCFG_DDS_32BIT
        | CHCFG_SDS_32BIT
        | CHCFG_AM_BURST
        | CHCFG_HIEN
        | CHCFG_REQD
        | CHCFG_LVL
        | (dma_ch as u32 & 7)
}

/// Compute CHCFG word for an SSI RX DMA channel (SSIFRDR → SRAM).
/// Source is fixed SSIFRDR (SAD=1); destination address increments (DAD=0).
/// Trigger: source-select (REQD=0) — SSI RX FIFO drives the request.
/// DEM is cleared, so DMAINT fires once per ring block.
#[inline(always)]
const fn rx_chcfg(dma_ch: u8) -> u32 {
    CHCFG_DMS
        | CHCFG_SAD
        | CHCFG_DDS_32BIT
        | CHCFG_SDS_32BIT
        | CHCFG_AM_BURST
        | CHCFG_HIEN
        | CHCFG_LVL
        | (dma_ch as u32 & 7)
}

// ── Owned driver: configuration, instances, pins ─────────────────────────────

mod sealed {
    pub trait Sealed {}
}

/// An SSIF channel with separate transmit and receive data pins: `peripherals::SSIF0`, `SSIF1`
/// or `SSIF3`. (SSIF2 shares one data pin and one DMA request between directions and isn't
/// supported yet.)
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The channel number.
    const INDEX: u8;
    /// Its transmit DMA request (DMARS: MID << 2 | RID).
    const DMARS_TX: u32;
    /// Its receive DMA request.
    const DMARS_RX: u32;
}

macro_rules! instances {
    ($($ssi:ident = $n:literal, $tx:literal, $rx:literal;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$ssi {}
        impl Instance for crate::peripherals::$ssi {
            const INDEX: u8 = $n;
            const DMARS_TX: u32 = $tx;
            const DMARS_RX: u32 = $rx;
        }
    )*};
}

// DMARS = MID << 2 | RID: MID 0x38 + n, RID 01 transmit, 10 receive (HW manual Table 9.4).
instances! {
    SSIF0 = 0, 0xE1, 0xE2;
    SSIF1 = 1, 0xE5, 0xE6;
    SSIF3 = 3, 0xED, 0xEE;
}

/// SSIF channel `n`'s registers.
pub(crate) fn regs(n: u8) -> crate::pac::ssif::Ssif {
    [crate::pac::SSIF0, crate::pac::SSIF1, crate::pac::SSIF2, crate::pac::SSIF3][n as usize]
}

pin_trait!(
    /// A pin that can carry SSI `T`'s bit clock (SSISCKn).
    SckPin, Instance
);
pin_trait!(
    /// A pin that can carry SSI `T`'s word select (SSIWSn).
    WsPin, Instance
);
pin_trait!(
    /// A pin that can carry SSI `T`'s transmit data (SSITxDn).
    TxdPin, Instance
);
pin_trait!(
    /// A pin that can carry SSI `T`'s receive data (SSIRxDn).
    RxdPin, Instance
);

/// The master clock divider (SSICR.CKDV): the bit clock is the master clock divided by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum ClockDiv {
    Div1,
    Div2,
    Div4,
    Div8,
    Div16,
    Div32,
    Div64,
    Div128,
    Div6,
    Div12,
    Div24,
    Div48,
    Div96,
}

/// Who drives the bit clock and word select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Role {
    /// This SSI drives them, from the master clock divided by the divider.
    Master(ClockDiv),
    /// The codec drives them.
    Slave,
}

/// Data bits per sample (SSICR.DWL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[allow(missing_docs)]
pub enum DataBits {
    B8,
    B16,
    B18,
    B20,
    B22,
    B24,
    B32,
}

/// Bits per system word, the slot a sample sits in (SSICR.SWL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[allow(missing_docs)]
pub enum SystemWordBits {
    B8,
    B16,
    B24,
    B32,
    B48,
    B64,
    B128,
    B256,
}

/// The master clock (SSICR.CKS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ClockSource {
    /// The AUDIO_X1 oscillator input.
    AudioX1,
    /// The AUDIO_CLK pin.
    AudioClk,
}

/// An SSI's I²S settings: stereo, one-clock data delay, both polarities as I²S specifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub struct Config {
    /// Who drives the clocks.
    pub role: Role,
    /// Data bits per sample.
    pub data_bits: DataBits,
    /// Bits per system word.
    pub system_word_bits: SystemWordBits,
    /// The master clock.
    pub clock_source: ClockSource,
}

impl Default for Config {
    /// Master from AUDIO_X1 ÷ 4, 24-bit samples in 32-bit words.
    fn default() -> Self {
        Self {
            role: Role::Master(ClockDiv::Div4),
            data_bits: DataBits::B24,
            system_word_bits: SystemWordBits::B32,
            clock_source: ClockSource::AudioX1,
        }
    }
}

/// SSICR for `config`, with TEN and REN clear.
#[doc(hidden)]
pub fn ssicr(config: &Config) -> u32 {
    let cks = match config.clock_source {
        ClockSource::AudioX1 => 0,
        ClockSource::AudioClk => 1 << 30,
    };
    let dwl = config.data_bits as u32;
    let swl = config.system_word_bits as u32;
    let role = match config.role {
        // SCKD and SWSD: this SSI outputs the bit clock and word select.
        Role::Master(div) => (1 << 15) | (1 << 14) | ((div as u32) << 4),
        Role::Slave => 0,
    };
    cks | (dwl << 19) | (swl << 16) | role
}

// ── Owned driver ──────────────────────────────────────────────────────────────

use crate::dmac::ring::{Dir, RingDma};
use crate::ring_buffer::{ReadableRing, RingBuf, WritableRing};

/// Read-modify-write SSIF `n`'s SSICR with interrupts masked: the two directions share it.
fn ssicr_update(n: u8, f: impl FnOnce(u32) -> u32) {
    critical_section::with(|_| {
        let cr = regs(n).ssicr();
        cr.write_value(f(cr.read()));
    });
}

/// Read-modify-write SSIF `n`'s SSIFCR with interrupts masked.
fn ssifcr_update(n: u8, f: impl FnOnce(u32) -> u32) {
    critical_section::with(|_| {
        let fcr = regs(n).ssifcr();
        fcr.write_value(f(fcr.read()));
    });
}

/// Pulse SSIF `n`'s software reset (CPG SWRSTCR1 bit 6 − n), then program its modes: continuous
/// transmit, `config`, both FIFOs held in reset with 4-word triggers.
fn reset_and_configure(n: u8, config: &Config) {
    let swrstcr = CPG.swrstcr1();
    let bit = 1u8 << (6 - n);
    swrstcr.write_value(Swrstcr1(swrstcr.read().0 | bit));
    let _ = swrstcr.read();
    swrstcr.write_value(Swrstcr1(swrstcr.read().0 & !bit));
    let _ = swrstcr.read();
    let r = regs(n);
    r.ssitdmr().write_value(SSITDMR_CONT);
    r.ssicr().write_value(ssicr(config));
    r.ssifcr().write_value(SSIFCR_INIT);
}

/// Start SSIF `T`'s transmit ring on `tx` (SSIFTDR as the destination).
fn start_tx<T: Instance>(tx: RingBuf<'_>) -> RingDma<'_> {
    let ch = tx.dma.number();
    let dst = regs(T::INDEX).ssiftdr().addr() as u32;
    RingDma::start(tx.dma, tx.buf, tx.blocks, 2, tx_chcfg(ch), Dir::MemToPeriph { dst }, T::DMARS_TX)
}

/// Start SSIF `T`'s receive ring on `rx` (SSIFRDR as the source).
fn start_rx<T: Instance>(rx: RingBuf<'_>) -> RingDma<'_> {
    let ch = rx.dma.number();
    let src = regs(T::INDEX).ssifrdr().addr() as u32;
    RingDma::start(rx.dma, rx.buf, rx.blocks, 2, rx_chcfg(ch), Dir::PeriphToMem { src }, T::DMARS_RX)
}

/// Release the transmit FIFO and route its request to the DMAC.
fn enable_tx_fifo(n: u8) {
    ssifcr_update(n, |v| v & !SSIFCR_TFRST);
    ssifcr_update(n, |v| v | SSIFCR_TIE);
}

/// Release the receive FIFO and route its request to the DMAC.
fn enable_rx_fifo(n: u8) {
    ssifcr_update(n, |v| v & !SSIFCR_RFRST);
    ssifcr_update(n, |v| v | SSIFCR_RIE);
}

/// The SSI constructors. Each takes an SSIF channel, its pins (put on the SSI's functions here),
/// a [`RingBuf`] per DMA direction and a [`Config`]; frames are stereo, two words each.
///
/// The master clock pin (AUDIO_XOUT) and the codec's power are the board's.
pub struct Ssi;

impl Ssi {
    /// Full duplex: transmit from `tx`, receive into `rx`, started together.
    #[allow(clippy::too_many_arguments)]
    pub fn new<'d, T: Instance>(
        _ssi: Peri<'d, T>,
        sck: Peri<'d, impl SckPin<T>>,
        ws: Peri<'d, impl WsPin<T>>,
        txd: Peri<'d, impl TxdPin<T>>,
        rxd: Peri<'d, impl RxdPin<T>>,
        tx: RingBuf<'d>,
        rx: RingBuf<'d>,
        config: Config,
    ) -> (SsiTx<'d>, SsiRx<'d>) {
        mux_clocks(sck, ws);
        let txd_alt = txd.alternate();
        crate::gpio::mux(txd, txd_alt);
        let rxd_alt = rxd.alternate();
        crate::gpio::mux(rxd, rxd_alt);
        let n = T::INDEX;
        reset_and_configure(n, &config);
        let tx = start_tx::<T>(tx);
        let rx = start_rx::<T>(rx);
        enable_tx_fifo(n);
        enable_rx_fifo(n);
        ssicr_update(n, |v| v | SSICR_TEN | SSICR_REN);
        debug!("ssi{}: full duplex streaming", n);
        (SsiTx { ring: WritableRing::new(tx), n }, SsiRx { ring: ReadableRing::new(rx), n })
    }

    /// Receive only, into `rx`.
    pub fn new_rx_only<'d, T: Instance>(
        _ssi: Peri<'d, T>,
        sck: Peri<'d, impl SckPin<T>>,
        ws: Peri<'d, impl WsPin<T>>,
        rxd: Peri<'d, impl RxdPin<T>>,
        rx: RingBuf<'d>,
        config: Config,
    ) -> SsiRx<'d> {
        mux_clocks(sck, ws);
        let rxd_alt = rxd.alternate();
        crate::gpio::mux(rxd, rxd_alt);
        let n = T::INDEX;
        reset_and_configure(n, &config);
        let rx = start_rx::<T>(rx);
        enable_rx_fifo(n);
        ssicr_update(n, |v| v | SSICR_REN);
        SsiRx { ring: ReadableRing::new(rx), n }
    }

    /// Receive into `rx`; transmit is fed by a SCUX path, which takes the [`ScuxOutput`] and
    /// enables it once its route is up.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_scux_tx<'d, T: Instance>(
        ssi: Peri<'d, T>,
        sck: Peri<'d, impl SckPin<T>>,
        ws: Peri<'d, impl WsPin<T>>,
        txd: Peri<'d, impl TxdPin<T>>,
        rxd: Peri<'d, impl RxdPin<T>>,
        rx: RingBuf<'d>,
        config: Config,
    ) -> (ScuxOutput<'d, T>, SsiRx<'d>) {
        let txd_alt = txd.alternate();
        crate::gpio::mux(txd, txd_alt);
        let rx = Self::new_rx_only(ssi, sck, ws, rxd, rx, config);
        (ScuxOutput { _ssi: PhantomData }, rx)
    }
}

/// Put the bit clock and word select pins on their functions.
fn mux_clocks<T: Instance>(sck: Peri<'_, impl SckPin<T>>, ws: Peri<'_, impl WsPin<T>>) {
    let (sck_alt, ws_alt) = (sck.alternate(), ws.alternate());
    crate::gpio::mux(sck, sck_alt);
    crate::gpio::mux(ws, ws_alt);
}

/// An SSI's transmit ring: a [`WritableRing`] (it derefs to one). Dropping it stops transmit
/// (TEN), then halts its DMA.
pub struct SsiTx<'d> {
    ring: WritableRing<'d>,
    n: u8,
}

/// An SSI's receive ring: a [`ReadableRing`] (it derefs to one). Dropping it stops receive (REN),
/// then halts its DMA.
pub struct SsiRx<'d> {
    ring: ReadableRing<'d>,
    n: u8,
}

impl<'d> core::ops::Deref for SsiTx<'d> {
    type Target = WritableRing<'d>;
    fn deref(&self) -> &WritableRing<'d> {
        &self.ring
    }
}

impl<'d> core::ops::DerefMut for SsiTx<'d> {
    fn deref_mut(&mut self) -> &mut WritableRing<'d> {
        &mut self.ring
    }
}

impl<'d> core::ops::Deref for SsiRx<'d> {
    type Target = ReadableRing<'d>;
    fn deref(&self) -> &ReadableRing<'d> {
        &self.ring
    }
}

impl<'d> core::ops::DerefMut for SsiRx<'d> {
    fn deref_mut(&mut self) -> &mut ReadableRing<'d> {
        &mut self.ring
    }
}

impl Drop for SsiTx<'_> {
    fn drop(&mut self) {
        // The ring (a field) halts its DMA after this.
        ssicr_update(self.n, |v| v & !SSICR_TEN);
    }
}

impl Drop for SsiRx<'_> {
    fn drop(&mut self) {
        ssicr_update(self.n, |v| v & !SSICR_REN);
    }
}

/// Stop SSIF `n`'s transmit (TEN): what a SCUX path's control does when it drops.
pub(crate) fn stop_tx(n: u8) {
    ssicr_update(n, |v| v & !SSICR_TEN);
}

/// SSI `T`'s transmit, fed by a SCUX path instead of a DMA ring. Dropping it stops transmit.
pub struct ScuxOutput<'d, T: Instance> {
    _ssi: PhantomData<(&'d mut (), T)>,
}

impl<T: Instance> ScuxOutput<'_, T> {
    /// Release the transmit FIFO and start transmitting: the SCUX path's route must be up.
    pub(crate) fn enable(&self) {
        ssifcr_update(T::INDEX, |v| v & !SSIFCR_TFRST);
        ssicr_update(T::INDEX, |v| v | SSICR_TEN);
    }
}

impl<T: Instance> Drop for ScuxOutput<'_, T> {
    fn drop(&mut self) {
        ssicr_update(T::INDEX, |v| v & !SSICR_TEN);
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn fifo_init_composes_reset_and_trigger_bits() {
        // INIT resets both FIFOs and sets the 4-stage trigger thresholds.
        assert_eq!(
            SSIFCR_INIT,
            SSIFCR_TFRST | SSIFCR_RFRST | SSIFCR_TTRG_4 | SSIFCR_RTRG_4
        );
        assert_eq!(SSIFCR_TTRG_4, 0x80);
        assert_eq!(SSIFCR_RTRG_4, 0x20);
        assert_eq!(SSIFCR_INIT, 0xA3);
    }

    #[test]
    fn control_enable_bits_are_distinct() {
        assert_ne!(SSICR_REN, SSICR_TEN);
        assert_eq!(SSICR_REN | SSICR_TEN, 0b11);
    }

    #[test]
    fn tx_chcfg_encodes_channel_and_direction() {
        // The DMA channel lands in the low 3 bits; everything else is fixed.
        assert_eq!(tx_chcfg(6) & 7, 6, "dma channel in low 3 bits");
        assert_eq!(tx_chcfg(7), tx_chcfg(0) | 7);
        // TX: destination fixed (DAD) and destination-triggered (REQD).
        assert_eq!(tx_chcfg(0) & CHCFG_DAD, CHCFG_DAD);
        assert_eq!(tx_chcfg(0) & CHCFG_REQD, CHCFG_REQD);
        assert_eq!(tx_chcfg(0) & CHCFG_SAD, 0, "TX source increments");
    }

    #[test]
    fn rx_chcfg_is_source_fixed_and_source_triggered() {
        // RX: source fixed (SAD), source-triggered (REQD clear), dest increments.
        assert_eq!(rx_chcfg(0) & CHCFG_SAD, CHCFG_SAD);
        assert_eq!(rx_chcfg(0) & CHCFG_REQD, 0, "RX is source-triggered");
        assert_eq!(rx_chcfg(0) & CHCFG_DAD, 0, "RX destination increments");
        assert_eq!(rx_chcfg(5) & 7, 5);
    }

    #[test]
    fn tx_and_rx_chcfg_share_format_but_differ_in_direction() {
        // Both 32-bit burst transfers triggered by a high-level DMA request (HIEN + LVL).
        let common = CHCFG_DDS_32BIT | CHCFG_SDS_32BIT | CHCFG_AM_BURST | CHCFG_HIEN | CHCFG_LVL;
        assert_eq!(tx_chcfg(0) & common, common);
        assert_eq!(rx_chcfg(0) & common, common);
        assert_ne!(tx_chcfg(0), rx_chcfg(0), "direction bits differ");
    }

    #[test]
    fn the_pac_names_the_same_ssi_registers() {
        use crate::pac::{CPG, SSIF0};
        let b = 0xE820_B000;
        assert_eq!((SSIF0.ssicr().addr(), SSIF0.ssicr().width()), (b, 4));
        assert_eq!(SSIF0.ssisr().addr(), b + 0x04);
        assert_eq!(SSIF0.ssifcr().addr(), b + 0x10);
        assert_eq!(SSIF0.ssifsr().addr(), b + 0x14);
        assert_eq!(SSIF0.ssiftdr().addr(), b + 0x18);
        assert_eq!(SSIF0.ssifrdr().addr(), b + 0x1C);
        assert_eq!(SSIF0.ssitdmr().addr(), b + 0x20);
        assert_eq!(
            (CPG.swrstcr1().addr(), CPG.swrstcr1().width()),
            (0xFCFE_0460, 1)
        );
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod config_tests {
    use super::*;

    #[test]
    fn the_default_config_encodes_0x002b_c020() {
        assert_eq!(ssicr(&Config::default()), 0x002B_C020);
    }

    #[test]
    fn slave_mode_clears_sckd_and_swsd() {
        let mut c = Config::default();
        c.role = Role::Slave;
        assert_eq!(ssicr(&c) & (3 << 14), 0);
        assert_eq!(ssicr(&c) & (0xF << 4), 0, "no divider without a clock to divide");
    }

    #[test]
    fn data_and_word_bits_land_in_dwl_and_swl() {
        let mut c = Config::default();
        c.data_bits = DataBits::B16;
        c.system_word_bits = SystemWordBits::B16;
        assert_eq!((ssicr(&c) >> 19) & 0b111, 0b001);
        assert_eq!((ssicr(&c) >> 16) & 0b111, 0b001);
        c.clock_source = ClockSource::AudioClk;
        assert_eq!(ssicr(&c) & (1 << 30), 1 << 30);
    }

    #[test]
    fn clock_dividers_follow_ckdv() {
        let mut c = Config::default();
        c.role = Role::Master(ClockDiv::Div96);
        assert_eq!((ssicr(&c) >> 4) & 0xF, 0b1100);
        c.role = Role::Master(ClockDiv::Div1);
        assert_eq!((ssicr(&c) >> 4) & 0xF, 0);
    }

    #[test]
    fn ssi_instances_know_their_dma_requests() {
        use crate::peripherals::{SSIF0, SSIF1, SSIF3};
        assert_eq!((SSIF0::INDEX, SSIF0::DMARS_TX, SSIF0::DMARS_RX), (0, 0xE1, 0xE2));
        assert_eq!((SSIF1::INDEX, SSIF1::DMARS_TX, SSIF1::DMARS_RX), (1, 0xE5, 0xE6));
        assert_eq!((SSIF3::INDEX, SSIF3::DMARS_TX, SSIF3::DMARS_RX), (3, 0xED, 0xEE));
        assert_eq!(regs(3).ssicr().addr(), 0xE820_C800);
    }

    #[cfg(feature = "rza1l")]
    #[test]
    fn ssi_pin_functions_are_generated() {
        use crate::peripherals::{P6_8, P6_9, P6_10, P6_11, SSIF0};
        fn sck<P: SckPin<SSIF0>>() {}
        fn ws<P: WsPin<SSIF0>>() {}
        fn txd<P: TxdPin<SSIF0>>() {}
        fn rxd<P: RxdPin<SSIF0>>() {}
        sck::<P6_8>();
        ws::<P6_9>();
        txd::<P6_10>();
        rxd::<P6_11>();
    }
}

#[cfg(feature = "rza1l")]
#[cfg(all(test, not(target_os = "none")))]
mod driver_tests {
    use super::*;
    use crate::dma_buf::UncachedMut;
    use crate::dmac::{BlockDma, BlockInterruptHandler};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH6, DMA_CH7, P6_8, P6_9, P6_10, P6_11};
    use crate::ring_buffer::RingBuf;

    crate::bind_interrupts!(struct Irqs {
        DMAINT6 => BlockInterruptHandler<DMA_CH6>;
        DMAINT7 => BlockInterruptHandler<DMA_CH7>;
    });

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ring_on<C: crate::dmac::Channel>(ch: crate::Peri<'static, C>) -> RingBuf<'static>
    where
        Irqs: crate::interrupt::Binding<C::Interrupt, BlockInterruptHandler<C>>,
    {
        RingBuf {
            dma: BlockDma::new(ch, Irqs),
            buf: UncachedMut::new(std::vec![0u32; 256].leak()).unwrap(),
            blocks: 4,
        }
    }

    fn duplex() -> (SsiTx<'static>, SsiRx<'static>) {
        test::reset();
        unsafe {
            Ssi::new(
                crate::peripherals::SSIF0::steal(),
                P6_8::steal(),
                P6_9::steal(),
                P6_10::steal(),
                P6_11::steal(),
                ring_on(DMA_CH6::steal()),
                ring_on(DMA_CH7::steal()),
                Config::default(),
            )
        }
    }

    fn ssicr_addr() -> usize {
        crate::pac::SSIF0.ssicr().addr()
    }

    #[test]
    fn ssi_new_programs_the_codec_config_and_starts_both_rings() {
        let _serial = serial();
        let (_tx, _rx) = duplex();
        assert_eq!(test::peek32(ssicr_addr()), 0x002B_C020 | SSICR_TEN | SSICR_REN);
        assert_eq!(test::peek32(0xFCFE_100C), 0x00E2_00E1, "DMARS3: ch7 receive, ch6 transmit");
        let fcr = test::peek32(crate::pac::SSIF0.ssifcr().addr());
        assert_eq!(fcr & (SSIFCR_TFRST | SSIFCR_RFRST), 0, "FIFOs out of reset");
        assert_eq!(fcr & (SSIFCR_TIE | SSIFCR_RIE), SSIFCR_TIE | SSIFCR_RIE);
        let log = test::log();
        let cr = log.iter().position(|a| a.write && a.addr == ssicr_addr()).unwrap();
        for ch in [6, 7] {
            let chctrl = crate::pac::DMAC.ch(ch).chctrl().addr();
            let seten = log.iter().position(|a| a.write && a.addr == chctrl && a.val & 1 != 0);
            assert!(matches!(seten, Some(s) if s > cr), "ch{ch} started after SSICR");
        }
    }

    #[test]
    fn rx_only_leaves_ten_clear() {
        let _serial = serial();
        test::reset();
        let _rx = unsafe {
            Ssi::new_rx_only(
                crate::peripherals::SSIF0::steal(),
                P6_8::steal(),
                P6_9::steal(),
                P6_11::steal(),
                ring_on(DMA_CH7::steal()),
                Config::default(),
            )
        };
        assert_eq!(test::peek32(ssicr_addr()) & (SSICR_TEN | SSICR_REN), SSICR_REN);
    }

    #[test]
    fn dropping_ssi_tx_clears_ten_then_halts() {
        let _serial = serial();
        let (tx, _rx) = duplex();
        let start = test::log().len();
        drop(tx);
        let log = test::log()[start..].to_vec();
        let ten_off = log
            .iter()
            .position(|a| a.write && a.addr == ssicr_addr() && a.val & SSICR_TEN == 0)
            .expect("TEN cleared");
        let chctrl6 = crate::pac::DMAC.ch(6).chctrl().addr();
        let clren = log.iter().position(|a| a.write && a.addr == chctrl6 && a.val & 2 != 0);
        assert!(matches!(clren, Some(c) if c > ten_off), "TEN off, then the channel halts");
        assert_eq!(test::peek32(ssicr_addr()) & SSICR_REN, SSICR_REN, "RX keeps running");
    }

    #[test]
    fn ssi_tx_is_a_writable_ring() {
        let _serial = serial();
        let (tx, rx) = duplex();
        assert_eq!((tx.len(), tx.frame_words()), (256, 2));
        assert_eq!(rx.len(), 256);
    }

    #[test]
    fn the_scux_tx_variant_starts_rx_only() {
        let _serial = serial();
        test::reset();
        let (out, _rx) = unsafe {
            Ssi::new_with_scux_tx(
                crate::peripherals::SSIF0::steal(),
                P6_8::steal(),
                P6_9::steal(),
                P6_10::steal(),
                P6_11::steal(),
                ring_on(DMA_CH7::steal()),
                Config::default(),
            )
        };
        assert_eq!(test::peek32(ssicr_addr()) & SSICR_TEN, 0);
        out.enable();
        assert_eq!(test::peek32(ssicr_addr()) & SSICR_TEN, SSICR_TEN);
        drop(out);
        assert_eq!(test::peek32(ssicr_addr()) & SSICR_TEN, 0);
    }
}
