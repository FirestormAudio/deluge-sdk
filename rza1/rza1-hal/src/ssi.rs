//! Synchronous Serial Interface (SSIF) driver for the RZ/A1L — 44.1 kHz I²S.
//!
//! Implements stereo audio streaming on SSI channel 0 using DMA circular
//! link-descriptor mode.  Only SSI0 is wired to the audio codec on Deluge
//! hardware; the other five channels are unused.
//!
//! ## Clock derivation
//! ```text
//! AUDIO_X1 = 22.5792 MHz  (external crystal on the Deluge board, 512 × 44.1 kHz)
//! ÷ 4  (CKDV = 0b0010)  →  BCLK = 5.6448 MHz
//! one stereo frame = 2 ch × 32-bit system words = 64 BCLK
//! ```
//!
//!
//! ## DMA layout
//! The channels are the board's: the `Dma` handles passed to [`init`]. The Deluge uses:
//!
//! | Channel | Direction       | GIC IRQ |
//! |---------|-----------------|---------|
//! | DMA 6   | SRAM → SSIFTDR  | 47      |
//! | DMA 7   | SSIFRDR → SRAM  | 48      |
//!
//! Both DMA channels run in self-referential circular link-descriptor mode:
//! the `next` pointer in each descriptor points back to itself, so the DMA
//! engine re-arms automatically after each pass through the buffer.
//!
//! ## Buffer sizes
//! | Buffer | Frames  | Stereo samples | Bytes  | Duration |
//! |--------|---------|----------------|--------|----------|
//! | TX     | 1 024   | 2 048          | 8 192  | ~23.2 ms |
//! | RX     | 2 048   | 4 096          | 16 384 | ~46.4 ms |
//!
//! ## Uncached access
//! DMA transfers bypass the CPU cache.  Any CPU code that reads/writes the
//! sample buffers must use the **uncached mirror** alias
//! (`physical address + UNCACHED_MIRROR_OFFSET`) to avoid stale cache lines.
//! [`tx_buf_start`], [`tx_buf_end`], [`rx_buf_start`], [`rx_buf_end`],
//! [`tx_current_ptr`], and [`rx_current_ptr`] all return uncached pointers.

use embassy_sync::once_lock::OnceLock;

use crate::Peri;
use crate::dmac::{BlockDma, Dma};
use crate::pac::cpg::regs::Swrstcr1;
use crate::pac::{CPG, SSIF0};
use crate::peripherals::SSIF0;

// ── Channel / resource constants ────────────────────────────────────────────

/// SSI hardware channel used for audio (SSI0).
pub const SSI_CH: u8 = 0;

/// SSI0's rings, once an `init*` has run: the pointer accessors read their positions.
static TX_DMA: OnceLock<Dma<'static>> = OnceLock::new();
static RX_DMA: OnceLock<Dma<'static>> = OnceLock::new();
/// The RX ring when it runs with per-block interrupts ([`init_block_irq`]).
static RX_BLOCK: OnceLock<BlockDma<'static>> = OnceLock::new();

fn keep<T>(slot: &OnceLock<T>, value: T) {
    if slot.init(value).is_err() {
        unreachable!("SSI0 and its DMA channels are owned once");
    }
}

/// The RX ring, polled or block-interrupt.
fn rx_dma() -> Option<&'static Dma<'static>> {
    RX_DMA
        .try_get()
        .or_else(|| RX_BLOCK.try_get().map(|ring| &**ring))
}

/// DMARS resource-selector value for SSI0 transmit.
const DMARS_SSI0_TX: u32 = 0x00E1;
/// DMARS resource-selector value for SSI0 receive.
const DMARS_SSI0_RX: u32 = 0x00E2;

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
// The Deluge value is 0x002B_C020 (DWL bits [21:19] = 0b101 = 24-bit; DWL=0b111
// is prohibited), held in `deluge_bsp::system::SSICR`.

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
const SWRSTCR1_SSI_ALL: u8 = 0b0111_1000;

/// Software-reset all four SSI channels, ending any transfer a warm start left running.
///
/// # Safety
/// Writes CPG SWRSTCR1. No SSI channel may be in use.
pub(crate) unsafe fn reset_all() {
    let swrstcr = CPG.swrstcr1();
    swrstcr.write_value(Swrstcr1(swrstcr.read().0 | SWRSTCR1_SSI_ALL));
    let _ = swrstcr.read(); // mandatory dummy read
    swrstcr.write_value(Swrstcr1(swrstcr.read().0 & !SWRSTCR1_SSI_ALL));
    let _ = swrstcr.read(); // mandatory dummy read
}

// ── CPG soft-reset ───────────────────────────────────────────────────────────

// ── Uncached mirror ──────────────────────────────────────────────────────────

/// Add this offset to any cached internal-SRAM virtual address to obtain its
/// uncached alias.  DMA transfers go directly to the physical bus, bypassing
/// the CPU cache, so the CPU must access DMA buffers through this alias.

// ── TX / RX sample buffers ───────────────────────────────────────────────────

/// TX buffer depth: stereo frames.
pub const TX_FRAMES: usize = 1024;
/// RX buffer depth: stereo frames.
pub const RX_FRAMES: usize = 2048;

/// Total TX buffer length in `i32` samples (TX_FRAMES × 2 mono channels).
pub const TX_BUF_LEN: usize = TX_FRAMES * 2;
/// Total RX buffer length in `i32` samples (RX_FRAMES × 2 mono channels).
pub const RX_BUF_LEN: usize = RX_FRAMES * 2;

/// Cache-line-aligned wrapper so `repr(align(32))` can apply to a fixed-size
/// array.  The DMA hardware requires descriptors and buffers to start on a
/// 32-byte (cache-line) boundary.
#[repr(align(32))]
struct Aligned32<const N: usize>([i32; N]);

// SAFETY: these buffers are only accessed through the uncached alias during
// normal operation; the DMA engine accesses them on the bus side.
static mut TX_BUF: Aligned32<TX_BUF_LEN> = Aligned32([0i32; TX_BUF_LEN]);
static mut RX_BUF: Aligned32<RX_BUF_LEN> = Aligned32([0i32; RX_BUF_LEN]);

// ── DMA link descriptors ─────────────────────────────────────────────────────

/// An 8-word, cache-line-aligned DMA link descriptor.
///
/// Word layout (matching the Renesas RZ/A1L DMA hardware):
/// | Word | Field       | Description                                |
/// |------|-------------|--------------------------------------------|
/// | 0    | HEADER      | Link-descriptor control flags (0b1101)     |
/// | 1    | N0SA / src  | Source address                             |
/// | 2    | N0DA / dst  | Destination address                        |
/// | 3    | N0TB / size | Transfer byte count                        |
/// | 4    | CHCFG       | Channel config (loaded into CHCFG register)|
/// | 5    | CHITVL      | Interval register (0 = no delay)           |
/// | 6    | CHEXT       | Extension register (0)                     |
/// | 7    | NXLA        | Address of the *next* descriptor (patched  |
/// |      |             | at runtime to point back to self)          |
#[repr(C, align(32))]
struct LinkDesc([u32; 8]);

/// Link-descriptor HEADER word.
///
/// Bit layout (TRM §9.5 link-descriptor format):
///   bit 3 (LDEN): 1 — this channel uses link-descriptor mode
///   bit 2 (NXA):  1 — NXLA field (word\[7\]) holds the next descriptor address
///   bit 1:        0 — reserved
///   bit 0:        1 — descriptor valid
/// Combined: 0b1101 = LDEN | NXA | valid.
const DESC_HEADER: u32 = 0b1101;

/// Mask to align a DMA current-address register down to a stereo-frame boundary.
/// One stereo frame = 2 × i32 = 8 bytes, so the mask clears the low 3 bits.
const STEREO_FRAME_ALIGN_MASK: u32 = !7u32;

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
/// DEM is cleared so DMAINT fires once per full buffer pass (enables interrupt).
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
/// DEM is cleared so DMAINT fires once per full buffer pass (enables interrupt).
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

/// TX link descriptor.  Word[1] (src), word[2] (dst, SSIFTDR), word[4] (CHCFG) and word[7]
/// (NXLA) are patched at runtime by [`init`].
static mut TX_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,                                       // Header: LDEN + NXA + valid
    0,                                                 // src  (→ TX_BUF, set at init)
    0,                                                 // dst  (→ SSIFTDR, set at init)
    (TX_BUF_LEN * core::mem::size_of::<i32>()) as u32, // transfer bytes
    0,                                                 // CHCFG (patched at init)
    0,                                                 // CHITVL (no interval)
    0,                                                 // CHEXT
    0,                                                 // NXLA → &TX_DESC (set at init)
]);

/// RX link descriptor.  Word[1] (src, SSIFRDR), word[2] (dst), word[4] (CHCFG) and word[7]
/// (NXLA) are patched at runtime by [`init`].
static mut RX_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,                                       // Header: LDEN + NXA + valid
    0,                                                 // src  (→ SSIFRDR, set at init)
    0,                                                 // dst  (→ RX_BUF, set at init)
    (RX_BUF_LEN * core::mem::size_of::<i32>()) as u32, // transfer bytes
    0,                                                 // CHCFG (patched at init)
    0,                                                 // CHITVL
    0,                                                 // CHEXT
    0,                                                 // NXLA → &RX_DESC (set at init)
]);

// ── Public API ───────────────────────────────────────────────────────────────

/// Start SSI0 as I²S master at `ssicr` (mode, clock divider, word length), its TX and RX rings
/// on `tx`/`rx`, streaming the TX buffer to the codec and the codec into the RX buffer
/// indefinitely. The rings are polled (see [`tx_current_ptr`]), so their completion interrupts
/// stay masked.
///
/// **Call order:** the SSIF0 module clock is running ([`ModuleClock::Ssif0`](crate::ModuleClock::Ssif0)
/// in [`Config::module_clocks`](crate::Config::module_clocks)) and the board's I²S
/// pins are muxed; the codec is powered after this returns.
pub fn init(_ssi: Peri<'static, SSIF0>, mut tx: Dma<'static>, mut rx: Dma<'static>, ssicr: u32) {
    tx.mask_interrupt();
    rx.mask_interrupt();
    // SAFETY: SSI0 and both channels are owned.
    unsafe { start(ssicr, &mut tx, &mut rx) };
    keep(&TX_DMA, tx);
    keep(&RX_DMA, rx);
}

/// [`init`] with the RX ring split into blocks whose interrupts [`wait_rx_block`] waits on.
pub fn init_block_irq(
    _ssi: Peri<'static, SSIF0>,
    mut tx: Dma<'static>,
    mut rx: BlockDma<'static>,
    ssicr: u32,
) {
    tx.mask_interrupt();
    // SAFETY: SSI0 and both channels are owned; `rx`'s block interrupt is already bound and on.
    unsafe { start_block_irq(ssicr, &mut tx, &mut rx) };
    keep(&TX_DMA, tx);
    keep(&RX_BLOCK, rx);
}

/// Start SSI0 with only its RX ring (TX comes from SCUX).
pub fn init_rx_only(_ssi: Peri<'static, SSIF0>, mut rx: Dma<'static>, ssicr: u32) {
    rx.mask_interrupt();
    // SAFETY: SSI0 and the channel are owned.
    unsafe { start_rx_only(ssicr, &mut rx) };
    keep(&RX_DMA, rx);
}

/// Wait for the next RX block of the ring [`init_block_irq`] started.
///
/// # Panics
/// If [`init_block_irq`] hasn't run.
pub async fn wait_rx_block() {
    RX_BLOCK
        .try_get()
        .expect("ssi::init_block_irq hasn't run")
        .wait_block()
        .await
}

/// Initialise SSI0 and start continuous DMA-driven I²S streaming.
///
/// After this call the DMA engine feeds the TX sample buffer into the codec
/// and captures the codec's output into the RX sample buffer, indefinitely.
///
/// **Call order requirements:**
/// 1. [`init`](crate::init) — starts the SSIF0 module clock ([`ModuleClock::Ssif0`](crate::ModuleClock::Ssif0)).
/// 2. Board audio pin-mux setup (done by `deluge_bsp::audio::init`).
/// 3. `ssi::init()` — this function.
/// 4. 5 ms hardware delay for codec power-on (done by `deluge_bsp::audio::init`).
/// 5. Assert the codec `CODEC_POWER` GPIO (done by `deluge_bsp::audio::init`).
///
/// # Safety
/// Must be called exactly once, from a single-threaded boot context, before
/// any audio task accesses the buffer pointers.
unsafe fn start(ssicr: u32, tx: &mut Dma<'_>, rx: &mut Dma<'_>) {
    unsafe {
        debug!(
            "ssi: init SSI0 (44.1 kHz I\u{00B2}S, DMA ch{}/{})",
            tx.number(),
            rx.number()
        );
        // 1. Patch self-referential fields in the link descriptors.
        //    Write through the uncached alias so the DMAC sees the correct values
        //    without needing a cache flush (DMAC reads physical SRAM directly).
        //    The NXLA pointer must also be the uncached alias address so every
        //    reload of the descriptor by the DMAC is coherent.
        let tx_desc_u = crate::uncached(core::ptr::addr_of!(TX_DESC) as *mut u32);
        let rx_desc_u = crate::uncached(core::ptr::addr_of!(RX_DESC) as *mut u32);

        tx_desc_u
            .add(1)
            .write_volatile(core::ptr::addr_of!(TX_BUF.0[0]) as u32);
        tx_desc_u
            .add(2)
            .write_volatile(SSIF0.ssiftdr().addr() as u32);
        tx_desc_u.add(4).write_volatile(tx_chcfg(tx.number()));
        tx_desc_u.add(7).write_volatile(tx_desc_u as u32);

        rx_desc_u
            .add(2)
            .write_volatile(core::ptr::addr_of!(RX_BUF.0[0]) as u32);
        rx_desc_u
            .add(1)
            .write_volatile(SSIF0.ssifrdr().addr() as u32);
        rx_desc_u.add(4).write_volatile(rx_chcfg(rx.number()));
        rx_desc_u.add(7).write_volatile(rx_desc_u as u32);

        // 2. SSI0 software reset via CPG SWRSTCR1 bit 6.
        let swrstcr = CPG.swrstcr1();
        let reset_bit = 1u8 << (6 - SSI_CH);
        swrstcr.write_value(Swrstcr1(swrstcr.read().0 | reset_bit));
        let _ = swrstcr.read(); // mandatory dummy read
        swrstcr.write_value(Swrstcr1(swrstcr.read().0 & !reset_bit));
        let _ = swrstcr.read(); // mandatory dummy read

        // 3. Configure SSI registers.
        // SSITDMR: CONT=1 (WS continue mode, required for master mode);
        //   TDM=0 (normal stereo, not TDM).
        //   BSP sets SCUX_SSITDMR_CONT_SET when mode_master=true.
        SSIF0.ssitdmr().write_value(SSITDMR_CONT);
        SSIF0.ssicr().write_value(ssicr); // master, 24-bit/32-bit, ÷4
        SSIF0.ssifcr().write_value(SSIFCR_INIT); // FIFOs in reset

        // 4. Initialise and connect DMA channels to the link descriptors.
        //    Pass uncached alias addresses so the DMAC can re-fetch them coherently.
        let tx_desc_u = crate::uncached(core::ptr::addr_of!(TX_DESC) as *mut u32) as *const u32;
        let rx_desc_u = crate::uncached(core::ptr::addr_of!(RX_DESC) as *mut u32) as *const u32;
        tx.init_with_link_descriptor(tx_desc_u, DMARS_SSI0_TX);
        rx.init_with_link_descriptor(rx_desc_u, DMARS_SSI0_RX);

        // 5. Start both DMA channels (software-reset then enable).
        tx.channel_start();
        rx.channel_start();
        debug!(
            "ssi: DMA TX ch{} + RX ch{} started",
            tx.number(),
            rx.number()
        );

        // 6. Release the FIFOs from reset and enable TX/RX.
        //    This mirrors `ssiStart()` from the C firmware exactly.
        let fcr = SSIF0.ssifcr();

        // Release TX FIFO from reset, enable TX DMA trigger
        fcr.write_value(fcr.read() & !SSIFCR_TFRST);
        fcr.write_value(fcr.read() | SSIFCR_TIE);
        // Release RX FIFO from reset, enable RX DMA trigger
        fcr.write_value(fcr.read() & !SSIFCR_RFRST);
        fcr.write_value(fcr.read() | SSIFCR_RIE);

        // Enable transmit and receive
        let cr = SSIF0.ssicr();
        cr.write_value(cr.read() | SSICR_TEN | SSICR_REN);
        debug!("ssi: TX+RX enabled, streaming");
    }
}

/// RX block size (stereo frames) for [`init_block_irq`]: the RX DMA fires its
/// completion interrupt every this-many frames.
pub const RX_IRQ_BLOCK_FRAMES: usize = 128;

/// Number of descriptors in the RX block-IRQ ring (tiles `RX_BUF`).
const RX_RING_DESCS: usize = RX_FRAMES / RX_IRQ_BLOCK_FRAMES;

/// Descriptor ring for the block-IRQ RX path (one per [`RX_IRQ_BLOCK_FRAMES`]).
static mut RX_RING: [LinkDesc; RX_RING_DESCS] = [const { LinkDesc([0u32; 8]) }; RX_RING_DESCS];

/// Like [`init`] but the RX DMA uses a ring of `RX_FRAMES / RX_IRQ_BLOCK_FRAMES` descriptors
/// (each [`RX_IRQ_BLOCK_FRAMES`] long, with the END interrupt **enabled**) that
/// tile the *same* contiguous `RX_BUF`. The RX DMAINT therefore fires once per
/// block instead of once per full buffer, giving an interrupt-driven block clock
/// ([`crate::dmac::BlockDma`] / [`wait_rx_block`]).
///
/// Because the descriptors tile one contiguous buffer, [`rx_current_ptr`] (CRDA)
/// still sweeps linearly, exactly as with [`init`]. TX is the same single
/// self-referential descriptor as [`init`].
///
/// # Safety
/// Same contract as [`init`]; call exactly one of `init` / `init_rx_only` /
/// `init_block_irq`.
unsafe fn start_block_irq(ssicr: u32, tx: &mut Dma<'_>, rx: &mut Dma<'_>) {
    unsafe {
        // ── TX: single self-referential descriptor (as in `init`). ────────────
        let tx_desc_u = crate::uncached(core::ptr::addr_of!(TX_DESC) as *mut u32);
        tx_desc_u
            .add(1)
            .write_volatile(core::ptr::addr_of!(TX_BUF.0[0]) as u32);
        tx_desc_u
            .add(2)
            .write_volatile(SSIF0.ssiftdr().addr() as u32);
        tx_desc_u.add(4).write_volatile(tx_chcfg(tx.number()));
        tx_desc_u.add(7).write_volatile(tx_desc_u as u32);

        // ── RX: ring of block-sized descriptors tiling RX_BUF. ────────────────
        let chunk_slots = RX_IRQ_BLOCK_FRAMES * 2; // i32 slots per block
        let chunk_bytes = (chunk_slots * core::mem::size_of::<i32>()) as u32;
        for i in 0..RX_RING_DESCS {
            let d = crate::uncached(core::ptr::addr_of!(RX_RING[i]) as *mut u32);
            let next =
                crate::uncached(core::ptr::addr_of!(RX_RING[(i + 1) % RX_RING_DESCS]) as *mut u32)
                    as u32;
            d.add(0).write_volatile(DESC_HEADER);
            d.add(1).write_volatile(SSIF0.ssifrdr().addr() as u32); // src = SSIFRDR (fixed)
            d.add(2)
                .write_volatile(core::ptr::addr_of!(RX_BUF.0[i * chunk_slots]) as u32); // dst slice
            d.add(3).write_volatile(chunk_bytes);
            d.add(4).write_volatile(rx_chcfg(rx.number())); // DEM=0 → per-descriptor END IRQ
            d.add(5).write_volatile(0);
            d.add(6).write_volatile(0);
            d.add(7).write_volatile(next); // NXLA → next descriptor (loops)
        }
        let rx_ring_head =
            crate::uncached(core::ptr::addr_of!(RX_RING[0]) as *mut u32) as *const u32;

        // ── SSI0 software reset + register config (identical to `init`). ──────
        let swrstcr = CPG.swrstcr1();
        let reset_bit = 1u8 << (6 - SSI_CH);
        swrstcr.write_value(Swrstcr1(swrstcr.read().0 | reset_bit));
        let _ = swrstcr.read();
        swrstcr.write_value(Swrstcr1(swrstcr.read().0 & !reset_bit));
        let _ = swrstcr.read();

        SSIF0.ssitdmr().write_value(SSITDMR_CONT);
        SSIF0.ssicr().write_value(ssicr);
        SSIF0.ssifcr().write_value(SSIFCR_INIT);

        // ── Connect DMA: TX single descriptor, RX descriptor ring. ───────────
        tx.init_with_link_descriptor(tx_desc_u as *const u32, DMARS_SSI0_TX);
        rx.init_with_link_descriptor(rx_ring_head, DMARS_SSI0_RX);
        tx.channel_start();
        rx.channel_start();

        // ── Release FIFOs + enable TX/RX (identical to `init`). ──────────────
        let fcr = SSIF0.ssifcr();
        fcr.write_value(fcr.read() & !SSIFCR_TFRST);
        fcr.write_value(fcr.read() | SSIFCR_TIE);
        fcr.write_value(fcr.read() & !SSIFCR_RFRST);
        fcr.write_value(fcr.read() | SSIFCR_RIE);
        let cr = SSIF0.ssicr();
        cr.write_value(cr.read() | SSICR_TEN | SSICR_REN);
        debug!("ssi: TX + block-IRQ RX ({} descs) streaming", RX_RING_DESCS);
    }
}

/// Initialise SSI0 **receive only** — used when the SCUX DVU path owns the
/// SSIF0 transmitter.
///
/// Performs the same SSI hardware reset and register setup as [`init`], then
/// starts **only** the RX DMA channel and enables only the RX FIFO and REN.
/// The TX DMA channel, TX FIFO, and TEN bit are left untouched so that the
/// SCUX can drive SSIF0 TX via its own direct-drive path without conflict.
///
/// # Safety
/// Same requirements as [`init`].  Do not call both [`init`] and this
/// function — call exactly one of the two.
unsafe fn start_rx_only(ssicr: u32, rx: &mut Dma<'_>) {
    unsafe {
        debug!(
            "ssi: init SSI0 RX-only (44.1 kHz I\u{00B2}S, DMA ch{} RX, TX owned by SCUX)",
            rx.number()
        );

        // 1. Patch RX link descriptor only.
        let rx_desc_u = crate::uncached(core::ptr::addr_of!(RX_DESC) as *mut u32);
        rx_desc_u
            .add(2)
            .write_volatile(core::ptr::addr_of!(RX_BUF.0[0]) as u32);
        rx_desc_u
            .add(1)
            .write_volatile(SSIF0.ssifrdr().addr() as u32);
        rx_desc_u.add(4).write_volatile(rx_chcfg(rx.number()));
        rx_desc_u.add(7).write_volatile(rx_desc_u as u32);

        // 2. SSI0 software reset.
        let swrstcr = CPG.swrstcr1();
        let reset_bit = 1u8 << (6 - SSI_CH);
        swrstcr.write_value(Swrstcr1(swrstcr.read().0 | reset_bit));
        let _ = swrstcr.read();
        swrstcr.write_value(Swrstcr1(swrstcr.read().0 & !reset_bit));
        let _ = swrstcr.read();

        // 3. Configure SSI registers (same as full init).
        SSIF0.ssitdmr().write_value(SSITDMR_CONT); // WS continue mode (master)
        SSIF0.ssicr().write_value(ssicr);
        SSIF0.ssifcr().write_value(SSIFCR_INIT); // both FIFOs held in reset

        // 4. Initialise and start RX DMA only.
        let rx_desc_u = crate::uncached(core::ptr::addr_of!(RX_DESC) as *mut u32) as *const u32;
        rx.init_with_link_descriptor(rx_desc_u, DMARS_SSI0_RX);
        rx.channel_start();
        debug!(
            "ssi: DMA RX ch{} started (TX skipped — SCUX owns TX)",
            rx.number()
        );

        // 5. Release RX FIFO only; leave TX FIFO in reset (TFRST stays set).
        let fcr = SSIF0.ssifcr();
        fcr.write_value(fcr.read() & !SSIFCR_RFRST);
        fcr.write_value(fcr.read() | SSIFCR_RIE);

        // 6. Enable receive only (TEN stays 0 — SCUX owns TX).
        let cr = SSIF0.ssicr();
        cr.write_value(cr.read() | SSICR_REN);
        debug!("ssi: RX-only enabled, streaming");
    }
}

/// Enable SSI0 transmitter (TEN = bit 1 of SSICR).
///
/// Must be called AFTER [`init_rx_only`] when the SCUX owns the SSI0 TX path.
/// The TRM requires `SSICRn.TEN = 1` when SCUX direct-drive (`SSICTRL.SSI0TX`)
/// is used (SSI012TEN at bit 1 of SSICTRL is for 6-channel mode only and must
/// remain 0 for direct drive).
///
/// Also releases the TX FIFO from reset (clears SSIFCR.TFRST) so that
/// SCUX writes to SSIFTDR can actually propagate into the TX serializer.
/// We clear only TFRST so the existing SSIFCR_RIE (RX DMA trigger) bit is preserved.
///
/// # Safety
/// Read-modify-writes SSI0 SSICR and SSIFCR.
pub unsafe fn enable_tx() {
    let fcr = SSIF0.ssifcr();
    fcr.write_value(fcr.read() & !SSIFCR_TFRST);
    let cr = SSIF0.ssicr();
    cr.write_value(cr.read() | SSICR_TEN);
    debug!("ssi: TX enabled (SCUX direct-drive mode), TX FIFO released");
}

// ── Buffer pointer accessors ─────────────────────────────────────────────────
//
// All pointers are into the *uncached* mirror alias so the CPU never sees
// stale cache lines when reading or writing samples that the DMA has touched.

/// Current DMA read position in the TX buffer (uncached), aligned to one
/// stereo frame (8 bytes).
///
/// Write audio samples *ahead of* this pointer.  Wrap at [`tx_buf_end`].
pub fn tx_current_ptr() -> *mut i32 {
    let crsa = TX_DMA
        .try_get()
        .map_or(unsafe { core::ptr::addr_of!(TX_BUF.0[0]) } as u32, |d| {
            d.current_src()
        });
    // Align down to a stereo frame (2 × i32 = 8 bytes).
    let aligned = crsa & STEREO_FRAME_ALIGN_MASK;
    crate::uncached(aligned as usize as *mut i32)
}

/// Current DMA write position in the RX buffer (uncached), aligned to one
/// stereo frame.
///
/// Read received samples *behind* this pointer.  Wrap at [`rx_buf_start`].
pub fn rx_current_ptr() -> *const i32 {
    let crda = rx_dma().map_or(unsafe { core::ptr::addr_of!(RX_BUF.0[0]) } as u32, |d| {
        d.current_dst()
    });
    let aligned = crda & STEREO_FRAME_ALIGN_MASK;
    crate::uncached(aligned as usize as *mut i32) as *const i32
}

/// Pointer to the first sample in the TX buffer (uncached view).
pub fn tx_buf_start() -> *mut i32 {
    crate::uncached(unsafe { core::ptr::addr_of!(TX_BUF.0[0]) } as *mut i32)
}

/// One-past-the-end pointer for the TX buffer (uncached view).
pub fn tx_buf_end() -> *mut i32 {
    unsafe { tx_buf_start().add(TX_BUF_LEN) }
}

/// Pointer to the first sample in the RX buffer (uncached view).
pub fn rx_buf_start() -> *const i32 {
    crate::uncached(unsafe { core::ptr::addr_of!(RX_BUF.0[0]) } as *mut i32) as *const i32
}

/// One-past-the-end pointer for the RX buffer (uncached view).
pub fn rx_buf_end() -> *const i32 {
    unsafe { rx_buf_start().add(RX_BUF_LEN) }
}

/// Returns the DMA channel number assigned to the SSI RX path.
///
/// Valid after [`init`], [`init_block_irq`] or [`init_rx_only`] has been
/// called.  Used by the firmware to register a GIC completion handler
/// (DMAINT = 41 + ch).
pub fn rx_dma_ch() -> u8 {
    rx_dma().map_or(0, |d| d.number())
}

/// Returns the DMA channel number assigned to the SSI TX path.
///
/// Valid after [`init`] or [`init_block_irq`] has been called.  Used by the
/// firmware to register a GIC completion handler (DMAINT = 41 + ch).
pub fn tx_dma_ch() -> u8 {
    TX_DMA.try_get().map_or(0, |d| d.number())
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

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn buffer_lengths_are_stereo_interleaved() {
        // Buffers hold L+R per frame, so the sample length is 2x the frame count.
        assert_eq!(TX_BUF_LEN, TX_FRAMES * 2);
        assert_eq!(RX_BUF_LEN, RX_FRAMES * 2);
        // RX is double-buffered relative to TX in this design.
        assert_eq!(RX_FRAMES, TX_FRAMES * 2);
    }

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
mod typed_tests {
    use super::*;
    use crate::dmac::{self, BlockDma, Dma};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH6, DMA_CH7, SSIF0};

    crate::bind_interrupts!(struct BlockIrqs {
        DMAINT7 => dmac::BlockInterruptHandler<DMA_CH7>;
    });

    crate::bind_interrupts!(struct CompletionIrqs {
        DMAINT6 => dmac::InterruptHandler<DMA_CH6>;
        DMAINT7 => dmac::InterruptHandler<DMA_CH7>;
    });

    #[test]
    fn ssi_rings_keep_their_completion_interrupts_masked() {
        test::reset();
        // Channels whose completion interrupts are enabled: `init` must mask them itself.
        let tx = Dma::new(unsafe { DMA_CH6::steal() }, CompletionIrqs);
        let rx = Dma::new(unsafe { DMA_CH7::steal() }, CompletionIrqs);
        let built = test::writes().len();
        init(unsafe { SSIF0::steal() }, tx, rx, 0x002B_C020);
        assert_eq!((tx_dma_ch(), rx_dma_ch()), (6, 7));
        // DMAINT6/7 (GIC 47/48) cleared in GICD_ICENABLER1 (0xE820_1184), bits 15 and 16.
        let writes = test::writes();
        let after = &writes[built..];
        assert!(after.contains(&(0xE820_1184, 1 << 15)), "DMAINT6 masked");
        assert!(after.contains(&(0xE820_1184, 1 << 16)), "DMAINT7 masked");
        // SSI0's TX/RX request routes: DMARS3 (channels 6 and 7) holds 0xE1 low, 0xE2 high.
        assert_eq!(test::peek32(0xFCFE_100C), 0x00E2_00E1);
    }

    /// The rings' fixed ends are the FIFO data registers: SSIFTDR for transmit, SSIFRDR for
    /// receive, written into the descriptors at start from the PAC.
    #[test]
    fn the_descriptors_name_the_fifo_data_registers() {
        test::reset();
        let mut tx = Dma::new_without_interrupt(unsafe { DMA_CH6::steal() });
        let mut rx = Dma::new_without_interrupt(unsafe { DMA_CH7::steal() });
        unsafe { start(0x002B_C020, &mut tx, &mut rx) };
        let tx_desc = crate::uncached(core::ptr::addr_of!(TX_DESC) as *mut u32);
        let rx_desc = crate::uncached(core::ptr::addr_of!(RX_DESC) as *mut u32);
        assert_eq!(
            unsafe { tx_desc.add(2).read_volatile() },
            0xE820_B018,
            "TX dst = SSIFTDR"
        );
        assert_eq!(
            unsafe { rx_desc.add(1).read_volatile() },
            0xE820_B01C,
            "RX src = SSIFRDR"
        );
    }

    #[test]
    fn the_block_ring_binds_its_interrupt_before_starting() {
        test::reset();
        let mut tx = Dma::new_without_interrupt(unsafe { DMA_CH6::steal() });
        let mut rx = BlockDma::new(unsafe { DMA_CH7::steal() }, BlockIrqs);
        let enabled_at = test::writes()
            .iter()
            .position(|w| *w == (0xE820_1104, 1 << 16));
        // The private bring-up, not `init_block_irq`: SSI's statics hold one ring set per
        // process, and `ssi_rings_keep_their_completion_interrupts_masked` stores it.
        unsafe { start_block_irq(0x002B_C020, &mut tx, &mut rx) };
        // Channel 7's SETEN (CHCTRL at 0xE820_01E8, value 1) comes after its interrupt is enabled.
        let started_at = test::writes().iter().rposition(|w| *w == (0xE820_01E8, 1));
        assert!(enabled_at.is_some() && started_at.is_some() && enabled_at < started_at);
        assert!(crate::gic::handler(48).is_some(), "DMAINT7 bound");
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod config_tests {
    use super::*;

    #[test]
    fn the_deluge_codec_config_encodes_0x002b_c020() {
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
