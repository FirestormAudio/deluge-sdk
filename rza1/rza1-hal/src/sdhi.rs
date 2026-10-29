//! Renesas RZ/A1L SD Host Interface (SDHI) register-level driver.
//!
//! Two SDHI controllers are present on RZ/A1L:
//!   - SDHI0 at 0xE804_E000 (not used on Deluge)
//!   - SDHI1 at 0xE804_E800 (Deluge SD card — port 1)
//!
//! All registers are 16-bit with byte-granular offsets per TRM table 38.2.
//! The data FIFO (SD_BUF0) is at offset 0x30, accessed as 32-bit words.
//!
//! GIC interrupt IDs for SDHI1:
//!   305 = INTC_ID_SDHI1_3  (card-detect / status-change)
//!   306 = INTC_ID_SDHI1_0  (command / data complete)
//!   307 = INTC_ID_SDHI1_1  (SDIO interrupt — left masked; SDIO is not used)
//!
//! ## Async model
//!
//! Each port has one [`embassy_sync::waitqueue::AtomicWaker`].  The GIC
//! handler captures INFO1/INFO2 into atomics, then wakes the waker.
//! `send_cmd`, `read_blocks_sw`, and `write_blocks_sw` use the shared wait
//! helpers (`wait_resp`, `wait_buf_ready`, `wait_access_end`) to sleep until
//! the expected interrupt bits arrive.
//!
//! ## Usage
//!
//! ```ignore
//! rza1_hal::bind_interrupts!(struct Irqs {
//!     SDHI1_STATUS => sdhi::StatusInterruptHandler<SDHI1>;
//!     SDHI1_OP => sdhi::OperationInterruptHandler<SDHI1>;
//!     DMAINT14 => dmac::InterruptHandler<DMA_CH14>;
//! });
//! let tx = dmac::Dma::new_without_interrupt(p.DMA_CH11);
//! let rx = dmac::Dma::new(p.DMA_CH14, Irqs);
//! let sdhi = sdhi::Sdhi::new(
//!     p.SDHI1, p.P7_4, p.P7_5, p.P7_3, p.P7_2, p.P7_7, p.P7_6, p.P7_0, p.P7_1, tx, rx, Irqs, sd_option,
//! );
//! // In an async task — SD card protocol:
//! sdhi.set_arg(0);
//! sdhi.send_cmd(0 /* CMD0: reset */).await?;
//! let resp = sdhi.read_r1();
//! ```

use crate::dma_buf::{Uncached, UncachedMut};
use embassy_hal_internal::drop::OnDrop;
use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use core::task::Poll;

use embassy_sync::waitqueue::AtomicWaker;

use crate::pac::CPG;
use crate::pac::cpg::regs::Stbcr12;

// ---------------------------------------------------------------------------
// Registers: `crate::pac::SDHI0`/`SDHI1` (TRM table 38.2), through `sdhi(port)`.
// All registers are 16-bit except SD_BUF0, which is 32-bit.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SD_INFO1 bit masks
// ---------------------------------------------------------------------------

/// Response end (CMD complete, response received).
pub const INFO1_RESP: u16 = 0x0001;
/// Data transfer end (all blocks transferred).
pub const INFO1_DATA_TRNS: u16 = 0x0004;
/// Card remove via DAT3 detect.
pub const INFO1_REM_DAT3: u16 = 0x0100;
/// Card insert  via DAT3 detect.
pub const INFO1_INS_DAT3: u16 = 0x0200;
/// Card remove  via CD pin.
pub const INFO1_REM_CD: u16 = 0x0008;
/// Card insert  via CD pin (latched event; cleared by writing 0).
pub const INFO1_INS_CD: u16 = 0x0010;
/// Card detect change (insert OR remove) — CD pin (latched event mask).
pub const INFO1_DET_CD: u16 = 0x0018;
/// Card currently present — SD_CD level (1 = SD_CD held low = card inserted).
/// Use this for steady-state presence queries; INFO1_INS_CD is only for edge events.
pub const INFO1_CD_LEVEL: u16 = 0x0020;
/// Write-protect — SD_WP level (INFO7).  Per the TRM the bit reflects the pin:
/// 1 = SD_WP held low, 0 = SD_WP high.  The mapping from that level to "write
/// protected" is board-specific — on the Deluge it is inverted (see
/// [`Sdhi::card_write_protected`]).
pub const INFO1_WP: u16 = 0x0080;

// ---------------------------------------------------------------------------
// SD_INFO2 bit masks
// ---------------------------------------------------------------------------

/// CMD error (command index mismatch in response).
pub const INFO2_ERR0: u16 = 0x0001;
/// CRC error (response CRC, read data CRC, or CRC status).
pub const INFO2_ERR1: u16 = 0x0002;
/// END error (end-bit missing in response, read data, or CRC status).
pub const INFO2_ERR2: u16 = 0x0004;
/// Data timeout (busy too long, R1b busy, write CRC, or read data not arriving).
pub const INFO2_ERR3: u16 = 0x0008;
/// SD_BUF illegal write access (write when not in data-write state or buffer full).
pub const INFO2_ERR4: u16 = 0x0010;
/// SD_BUF illegal read access (read when buffer empty or data has CRC/END error).
pub const INFO2_ERR5: u16 = 0x0020;
/// Response timeout (no response within 640 SD_CLK cycles).
pub const INFO2_ERR6: u16 = 0x0040;
/// Buffer Read Enable (FIFO has data to read).
pub const INFO2_BRE: u16 = 0x0100;
/// Buffer Write Enable (FIFO is ready to accept data).
pub const INFO2_BWE: u16 = 0x0200;
/// Command type register busy.
pub const INFO2_CBSY: u16 = 0x4000;
/// Illegal access (register out-of-sequence).
pub const INFO2_ILA: u16 = 0x8000;
/// Bus clock divider change complete.
pub const INFO2_SCLKDIVEN: u16 = 0x2000;

/// All error bits in INFO2.
pub const INFO2_ERR_ALL: u16 = 0x807F;

// ---------------------------------------------------------------------------
// CC_EXT_MODE bits
// ---------------------------------------------------------------------------
const CC_EXT_MODE_DMASDRW: u16 = 1 << 1; // Enable SDHI DMA read/write

// ---------------------------------------------------------------------------
// DMA resource selectors (DMARS) for SDHI (BSP: dma_if.h DMA_RS_SDHI_*)
// Encoding: (MID[6:0] << 2) | RID[1:0]
// ---------------------------------------------------------------------------

/// DMARS for SDHI0 transmit (memory → SD_BUF0). MID=0x30, RID=01.
const DMARS_SDHI0_TX: u32 = 0x00C1;
/// DMARS for SDHI0 receive  (SD_BUF0 → memory). MID=0x30, RID=02.
const DMARS_SDHI0_RX: u32 = 0x00C2;
/// DMARS for SDHI1 transmit (memory → SD_BUF0). MID=0x31, RID=01.
const DMARS_SDHI1_TX: u32 = 0x00C5;
/// DMARS for SDHI1 receive  (SD_BUF0 → memory). MID=0x31, RID=02.
const DMARS_SDHI1_RX: u32 = 0x00C6;

use crate::dmac::{
    CHCFG_AM_BURST, CHCFG_DAD, CHCFG_DDS_32BIT, CHCFG_HIEN, CHCFG_LVL, CHCFG_REQD, CHCFG_SAD,
    CHCFG_SDS_32BIT,
};

/// Compute CHCFG for an SDHI DMA TX channel (memory → SD_BUF0).
/// Source increments (SAD=0); destination is fixed SD_BUF0 (DAD=1, REQD=1).
/// Register mode (DMS=0), DEM=0 (completion interrupt enabled).
#[inline(always)]
const fn chcfg_sdhi_tx(dma_ch: u8) -> u32 {
    CHCFG_DAD
        | CHCFG_DDS_32BIT
        | CHCFG_SDS_32BIT
        | CHCFG_AM_BURST
        | CHCFG_HIEN
        | CHCFG_LVL
        | CHCFG_REQD
        | (dma_ch as u32 & 7)
}

/// Compute CHCFG for an SDHI DMA RX channel (SD_BUF0 → memory).
/// Source is fixed SD_BUF0 (SAD=1); destination increments (DAD=0, REQD=0).
/// Register mode (DMS=0), DEM=0 (completion interrupt enabled).
#[inline(always)]
const fn chcfg_sdhi_rx(dma_ch: u8) -> u32 {
    CHCFG_SAD
        | CHCFG_DDS_32BIT
        | CHCFG_SDS_32BIT
        | CHCFG_AM_BURST
        | CHCFG_HIEN
        | CHCFG_LVL
        | (dma_ch as u32 & 7)
}

// ---------------------------------------------------------------------------
// Clock divider constants (SD_CLK_CTRL bits[7:0], P1φ input ≈ 66.6 MHz)
//
// TRM table (one-hot encoding — only one bit may be set; 0x00 = P1/2):
//   0x80 = P1/512 ≈  130 kHz   (identification / card-detect)
//   0x40 = P1/256 ≈  260 kHz
//   0x20 = P1/128 ≈  521 kHz
//   0x10 = P1/64  ≈  1.0 MHz
//   0x08 = P1/32  ≈  2.1 MHz
//   0x04 = P1/16  ≈  4.2 MHz
//   0x02 = P1/8   ≈  8.3 MHz
//   0x01 = P1/4   ≈ 16.7 MHz   (fast mode after init)
//   0x00 = P1/2   ≈ 33.3 MHz
// ---------------------------------------------------------------------------
const CLK_DIV_512: u16 = 0x80; // P1/512 ≈ 130 kHz  (identification clock)
/// P1/4 ≈ 16.7 MHz — default-speed data clock (any card).
pub const CLK_DIV_4: u16 = 0x01;
/// P1/2 ≈ 33.3 MHz — requires the card in High-Speed mode (CMD6) first.
/// The sole legal non-one-hot divider encoding (TRM §38.2.10: 0x00 = P1/2).
pub const CLK_DIV_2: u16 = 0x00;
const CLK_ENABLE: u16 = 1 << 8; // Bit 8 (SCLKEN) = clock output enable

// ---------------------------------------------------------------------------
// Per-port state
// ---------------------------------------------------------------------------

const NUM_PORTS: usize = 2;

struct SdhiState {
    waker: AtomicWaker,
    /// Accumulated INFO1 bits from ISR (ANDed with active mask at capture time).
    info1: AtomicU16,
    /// Accumulated INFO2 bits from ISR.
    info2: AtomicU16,
    /// Card-detect removal edge seen by [`interrupt_handler`], for
    /// [`take_card_detect_events`] to consume.
    ///
    /// The ISR clears the whole latched INFO1 word in hardware and `clear_info`
    /// wipes the [`Self::info1`] accumulator at the start of every transfer, so a
    /// card-detect edge that happened to be latched when an unrelated SDHI
    /// interrupt fired would otherwise be destroyed before anyone read it. These
    /// two flags are the ISR's hand-off for exactly that case, and nothing but
    /// [`take_card_detect_events`] consumes them.
    cd_removed: AtomicBool,
    /// Card-detect insertion edge seen by [`interrupt_handler`]. See
    /// [`Self::cd_removed`].
    cd_inserted: AtomicBool,
}

unsafe impl Sync for SdhiState {}

static STATE: [SdhiState; NUM_PORTS] = [
    SdhiState {
        waker: AtomicWaker::new(),
        info1: AtomicU16::new(0),
        info2: AtomicU16::new(0),
        cd_removed: AtomicBool::new(false),
        cd_inserted: AtomicBool::new(false),
    },
    SdhiState {
        waker: AtomicWaker::new(),
        info1: AtomicU16::new(0),
        info2: AtomicU16::new(0),
        cd_removed: AtomicBool::new(false),
        cd_inserted: AtomicBool::new(false),
    },
];

// ---------------------------------------------------------------------------
// GIC IRQ IDs
// ---------------------------------------------------------------------------

/// GIC IDs for SDHI0 and SDHI1: status-change, operation, SDIO. (The bindings name them; the
/// tests pin the numbers against the manual.)
#[cfg(test)]
const SDHI0_IRQS: [u16; 3] = [302, 303, 304];
#[cfg(test)]
const SDHI1_IRQS: [u16; 3] = [305, 306, 307];

// ---------------------------------------------------------------------------
// STBCR12 clock-gate bit masks
// ---------------------------------------------------------------------------

/// STBCR12 bits 3+2: SDHI00+SDHI01 — clock-gate mask for SDHI port 0.
const STBCR12_SDHI0_MASK: u8 = 0x0C;
/// STBCR12 bits 1+0: SDHI10+SDHI11 — clock-gate mask for SDHI port 1.
const STBCR12_SDHI1_MASK: u8 = 0x03;

// ---------------------------------------------------------------------------
// Interrupt-mask initialisation values (sd_init.c SD_INFO*_MASK constants)
// ---------------------------------------------------------------------------

/// INFO1_MASK init value: mask all INFO1 interrupts except card-detect.
/// BSP: `SD_INFO1_MASK = 0x031D` (sd_init.c).
const INFO1_MASK_INIT: u16 = 0x031D;
/// INFO2_MASK init value: mask all INFO2 error/status interrupts.
/// BSP: `SD_INFO2_MASK = 0x8B7F` (sd_init.c).
const INFO2_MASK_INIT: u16 = 0x8B7F;
/// SDIO_INFO1_MASK init value: mask all SDIO status interrupts.
const SDIO_INFO1_MASK_INIT: u16 = 0xC007;

// ---------------------------------------------------------------------------
// Soft-reset sequence values (SOFT_RST register)
// ---------------------------------------------------------------------------

/// SD_STOP.STP: stop the data transfer in progress.
const SD_STOP_STP: u16 = 0x0001;
/// SOFT_RST value to assert reset (bit 1 set, bit 0 clear).
/// BSP: written first in the RZ/A1 two-step soft-reset sequence.
const SOFT_RST_ASSERT: u16 = 0x0006;
/// SOFT_RST value to deassert reset (bit 1 set, bit 0 set).
/// BSP: written second to release the core from reset while keeping CMD/DAT active.
const SOFT_RST_DEASSERT: u16 = 0x0007;

// ---------------------------------------------------------------------------
// Register helpers
// ---------------------------------------------------------------------------

/// SDHI port `port`'s registers.
fn sdhi(port: u8) -> crate::pac::sdhi::Sdhi {
    if port == 0 {
        crate::pac::SDHI0
    } else {
        crate::pac::SDHI1
    }
}

// ---------------------------------------------------------------------------
// Public API: hardware initialization
// ---------------------------------------------------------------------------

/// Initialize SDHI hardware for `port` (0 or 1).
///
/// Enables the module clock (STBCR12), releases soft-reset, configures
/// interrupt masks, and sets the SD timing option register.
///
/// Called by [`Sdhi::new`], and by [`Sdhi::reset`] before each re-identification.
///
/// # Safety
/// Writes to memory-mapped peripheral registers.
unsafe fn init(port: u8, sd_option: u16) {
    // ---- enable SDHI module clock (STBCR12) ----
    // STBCR12 layout (bit 7..0): [1][1][1][1][SDHI00][SDHI01][SDHI10][SDHI11]
    //   SDHI0 (port 0): bits 3 (SDHI00) + 2 (SDHI01) — mask 0x0C
    //   SDHI1 (port 1): bits 1 (SDHI10) + 0 (SDHI11) — mask 0x03
    // Use RMW to enable only the clocks for the requested port; preserve
    // the other port's stop-bits so unused clocks stay gated.
    let stbcr12 = CPG.stbcr12();
    let bits: u8 = if port == 0 {
        STBCR12_SDHI0_MASK
    } else {
        STBCR12_SDHI1_MASK
    };
    stbcr12.write_value(Stbcr12(stbcr12.read().0 & !bits));
    let _ = stbcr12.read(); // dummy read (required per RZ/A1 HW manual)

    let s = sdhi(port);

    // ---- mask all meaningful interrupts (0 = enabled, 1 = masked) ----
    // Values match sd_init.c: SD_INFO1_MASK=0x031D, SD_INFO2_MASK=0x8B7F (RZA1)
    s.sd_info1_mask().write_value(INFO1_MASK_INIT);
    s.sd_info2_mask().write_value(INFO2_MASK_INIT);
    s.sdio_info1_mask().write_value(SDIO_INFO1_MASK_INIT);

    // ---- clear SDIO mode ----
    s.sdio_mode().write_value(0x0000);

    // ---- clear status registers ----
    // Clear RESP(bit0) and DATA_TRNS(bit2) only, per sd_init.c: info1 & ~0x0005
    let info1 = s.sd_info1().read();
    s.sd_info1()
        .write_value(info1 & !(INFO1_RESP | INFO1_DATA_TRNS));
    s.sd_info2().write_value(0x0000);
    s.sdio_info1().write_value(0x0000);

    // ---- soft reset (RZ/A1 variant: SOFT_RST_ASSERT → SOFT_RST_DEASSERT) ----
    s.soft_rst().write_value(SOFT_RST_ASSERT);
    s.soft_rst().write_value(SOFT_RST_DEASSERT);

    // ---- SD_OPTION: bus width, data timeout and card-detect time ----
    // Board-specific; see the caller's `sd_option`.
    s.sd_option().write_value(sd_option);

    // ---- EXT_SWAP: no byte-swap ----
    s.ext_swap().write_value(0x0000);

    // ---- CC_EXT_MODE: software transfer mode (no DMA yet) ----
    s.cc_ext_mode().write_value(0x0000);

    // ---- block size = 512 bytes ----
    s.sd_size().write_value(512);

    // ---- start at identification clock (~130 kHz, P1/512) ----
    s.sd_clk_ctrl().write_value(CLK_DIV_512 | CLK_ENABLE);
}

/// Set the SD clock divider (`CLK_DIV_*` constant).
///
/// Waits for `SCLKDIVEN` before and after the change per TRM §38.2.10
/// ("Do not write to SD_CLK_CTRL while the SCLKDIVEN bit in SD_INFO2 is 0").
///
/// `CLK_DIV_2` (33.3 MHz) exceeds the 25 MHz default-speed limit — only set
/// it after the card has been switched to High-Speed mode via CMD6.
///
/// # Safety
/// Writes to memory-mapped SDHI register.
unsafe fn set_clock_div(port: u8, div: u16) {
    let s = sdhi(port);
    // Wait for clock divider change to settle (SCLKDIVEN = 1 means ready).
    let mut ok = false;
    for _ in 0..10_000u32 {
        if s.sd_info2().read() & INFO2_SCLKDIVEN != 0 {
            ok = true;
            break;
        }
    }
    if !ok {
        warn!("sdhi{}: set_clock_div: SCLKDIVEN pre-change timeout", port);
    }
    s.sd_clk_ctrl().write_value(div | CLK_ENABLE);
    ok = false;
    for _ in 0..10_000u32 {
        if s.sd_info2().read() & INFO2_SCLKDIVEN != 0 {
            ok = true;
            break;
        }
    }
    if !ok {
        warn!("sdhi{}: set_clock_div: SCLKDIVEN post-change timeout", port);
    }
}

/// Switch the SD clock to the default-speed data clock (~16.7 MHz, P1/4).
///
/// Call after successful card initialization.
///
/// # Safety
/// Writes to memory-mapped SDHI register.
unsafe fn set_clock_fast(port: u8) {
    unsafe { set_clock_div(port, CLK_DIV_4) }
}

// ---------------------------------------------------------------------------
// Interrupt handlers (called by GIC dispatch)
// ---------------------------------------------------------------------------

/// Core interrupt handler: capture INFO1/INFO2, accumulate, wake waiter.
///
/// # Safety
/// Reads and writes memory-mapped SDHI registers.
unsafe fn interrupt_handler(port: u8) {
    let s = sdhi(port);

    // Read status
    let info1 = s.sd_info1().read();
    let info2 = s.sd_info2().read();

    // Clear bits by writing 0 (write-0-to-clear per RZ/A1 HW manual).
    // Only clear the bits we just read to avoid losing a racing edge.
    s.sd_info1().write_value(!info1);
    s.sd_info2().write_value(!info2);

    // Also clear SDIO status to prevent spurious SDIO interrupts.
    let sdio = s.sdio_info1().read();
    s.sdio_info1().write_value(!sdio);

    let st = &STATE[port as usize];
    st.info1.fetch_or(info1, Ordering::Release);
    st.info2.fetch_or(info2, Ordering::Release);
    // The clear above took the card-detect edge latches down with everything
    // else, and `clear_info` wipes the accumulator at each transfer start —
    // so hand any edge over to `take_card_detect_events` separately.
    if info1 & INFO1_REM_CD != 0 {
        st.cd_removed.store(true, Ordering::Release);
    }
    if info1 & INFO1_INS_CD != 0 {
        st.cd_inserted.store(true, Ordering::Release);
    }
    st.waker.wake();
}

// ---------------------------------------------------------------------------
// Async waiting primitives
// ---------------------------------------------------------------------------

/// Take (atomically consume) accumulated INFO1/INFO2 bits.
#[allow(dead_code)]
fn take_info(port: u8) -> (u16, u16) {
    let st = &STATE[port as usize];
    let i1 = st.info1.swap(0, Ordering::Acquire);
    let i2 = st.info2.swap(0, Ordering::Acquire);
    (i1, i2)
}

/// Peek at INFO1/INFO2 without consuming.
#[allow(dead_code)]
fn peek_info(port: u8) -> (u16, u16) {
    let st = &STATE[port as usize];
    let i1 = st.info1.load(Ordering::Acquire);
    let i2 = st.info2.load(Ordering::Acquire);
    (i1, i2)
}

/// Clear accumulated state.
fn clear_info(port: u8) {
    let st = &STATE[port as usize];
    st.info1.store(0, Ordering::Release);
    st.info2.store(0, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Shared async wait primitives
// ---------------------------------------------------------------------------
//
// Each helper is the interrupt-driven wait used by the command/data paths:
// check accumulated state, register the waker, then re-check (closing the
// race where the interrupt fires between the load and the register).

/// Await response end (`INFO1_RESP`), surfacing INFO2 errors.
async fn wait_resp(port: u8) -> Result<(), SdhiError> {
    poll_fn(|cx| {
        let st = &STATE[port as usize];
        let i1 = st.info1.load(Ordering::Acquire);
        let i2 = st.info2.load(Ordering::Acquire);
        if let Err(e) = check_info2_errors(i2) {
            return Poll::Ready(Err(e));
        }
        if i1 & INFO1_RESP != 0 {
            return Poll::Ready(Ok(()));
        }
        st.waker.register(cx.waker());
        // Re-check in case the interrupt fired between the load and register.
        let i1 = st.info1.load(Ordering::Acquire);
        let i2 = st.info2.load(Ordering::Acquire);
        if let Err(e) = check_info2_errors(i2) {
            return Poll::Ready(Err(e));
        }
        if i1 & INFO1_RESP != 0 {
            return Poll::Ready(Ok(()));
        }
        Poll::Pending
    })
    .await
}

/// Await a FIFO-ready bit (`INFO2_BRE` or `INFO2_BWE`) and consume it.
async fn wait_buf_ready(port: u8, ready_bit: u16) -> Result<(), SdhiError> {
    poll_fn(|cx| {
        let st = &STATE[port as usize];
        let i2 = st.info2.load(Ordering::Acquire);
        if i2 & ready_bit != 0 {
            st.info2.fetch_and(!ready_bit, Ordering::AcqRel);
            return Poll::Ready(Ok::<(), SdhiError>(()));
        }
        if let Err(e) = check_info2_errors(i2) {
            return Poll::Ready(Err(e));
        }
        st.waker.register(cx.waker());
        // Re-check in case the interrupt fired between the load and register.
        let i2 = st.info2.load(Ordering::Acquire);
        if i2 & ready_bit != 0 {
            st.info2.fetch_and(!ready_bit, Ordering::AcqRel);
            return Poll::Ready(Ok(()));
        }
        if let Err(e) = check_info2_errors(i2) {
            return Poll::Ready(Err(e));
        }
        Poll::Pending
    })
    .await
}

/// Await access end (`INFO1_DATA_TRNS`), surfacing INFO2 errors.
async fn wait_access_end(port: u8) -> Result<(), SdhiError> {
    poll_fn(|cx| {
        let st = &STATE[port as usize];
        let i1 = st.info1.load(Ordering::Acquire);
        let i2 = st.info2.load(Ordering::Acquire);
        if let Err(e) = check_info2_errors(i2) {
            return Poll::Ready(Err(e));
        }
        if i1 & INFO1_DATA_TRNS != 0 {
            return Poll::Ready(Ok(()));
        }
        st.waker.register(cx.waker());
        // Re-check in case the interrupt fired between the load and register.
        let i1 = st.info1.load(Ordering::Acquire);
        let i2 = st.info2.load(Ordering::Acquire);
        if let Err(e) = check_info2_errors(i2) {
            return Poll::Ready(Err(e));
        }
        if i1 & INFO1_DATA_TRNS != 0 {
            return Poll::Ready(Ok(()));
        }
        Poll::Pending
    })
    .await
}

// ---------------------------------------------------------------------------
// Command/argument helpers
// ---------------------------------------------------------------------------

/// Set the 32-bit command argument (split into two 16-bit registers).
///
/// # Safety
/// Writes to SDHI registers.
#[inline]
unsafe fn set_arg(port: u8, arg: u32) {
    let s = sdhi(port);
    s.sd_arg0().write_value((arg & 0xFFFF) as u16);
    s.sd_arg1().write_value((arg >> 16) as u16);
}

/// Wait for the clock divider register to be ready (SCLKDIVEN bit).
///
/// # Safety
/// Reads SDHI INFO2 register.
unsafe fn wait_clk_stable(port: u8) {
    let s = sdhi(port);
    for _ in 0..100_000u32 {
        if s.sd_info2().read() & INFO2_SCLKDIVEN != 0 {
            return;
        }
    }
    // Timeout — continue anyway; hardware may still work.
}

// ---------------------------------------------------------------------------
// Async command issue
// ---------------------------------------------------------------------------

/// An SD host error: what went wrong with a command or a data transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum SdhiError {
    /// Response timeout (no response received in time).
    ResponseTimeout,
    /// Response CRC error.
    ResponseCrc,
    /// Data CRC error.
    DataCrc,
    /// Data timeout.
    DataTimeout,
    /// SDHI command busy timeout.
    CmdBusy,
    /// Illegal register access.
    IllegalAccess,
    /// Generic hardware error.
    HardwareError,
    /// The buffer is empty or not a whole number of 512-byte blocks.
    BufferLength,
}

/// The number of 512-byte blocks in a `len`-byte buffer, or [`SdhiError::BufferLength`] if it is
/// empty or not a whole number of blocks.
fn block_count(len: usize) -> Result<u32, SdhiError> {
    if len == 0 || len % 512 != 0 {
        return Err(SdhiError::BufferLength);
    }
    u32::try_from(len / 512).map_err(|_| SdhiError::BufferLength)
}

impl From<SdhiError> for &'static str {
    fn from(e: SdhiError) -> &'static str {
        match e {
            SdhiError::ResponseTimeout => "SDHI: response timeout",
            SdhiError::ResponseCrc => "SDHI: response CRC error",
            SdhiError::DataCrc => "SDHI: data CRC error",
            SdhiError::DataTimeout => "SDHI: data timeout",
            SdhiError::CmdBusy => "SDHI: command busy timeout",
            SdhiError::IllegalAccess => "SDHI: illegal register access",
            SdhiError::HardwareError => "SDHI: hardware error",
            SdhiError::BufferLength => "SDHI: buffer is not a whole number of 512-byte blocks",
        }
    }
}

fn check_info2_errors(info2: u16) -> Result<(), SdhiError> {
    if info2 & INFO2_ILA != 0 {
        return Err(SdhiError::IllegalAccess);
    }
    if info2 & INFO2_ERR6 != 0 {
        return Err(SdhiError::ResponseTimeout);
    }
    if info2 & INFO2_ERR5 != 0 {
        // SD_BUF illegal read — treat as hardware error
        return Err(SdhiError::HardwareError);
    }
    if info2 & INFO2_ERR4 != 0 {
        // SD_BUF illegal write — treat as hardware error
        return Err(SdhiError::HardwareError);
    }
    if info2 & INFO2_ERR0 != 0 {
        // Command index mismatch
        return Err(SdhiError::HardwareError);
    }
    if info2 & INFO2_ERR1 != 0 {
        return Err(SdhiError::ResponseCrc);
    }
    if info2 & INFO2_ERR2 != 0 {
        // END error (end-bit missing) — can occur in response or data
        return Err(SdhiError::HardwareError);
    }
    if info2 & INFO2_ERR3 != 0 {
        return Err(SdhiError::DataTimeout);
    }
    Ok(())
}

/// Issue an SD command and wait asynchronously for the response end.
///
/// `cmd_val` is written directly to the SD_CMD register.
/// The argument should have been set via [`set_arg`] immediately before.
///
/// Returns `Ok(())` when INFO1_RESP is set; `Err` on hardware error.
///
/// # Safety
/// Reads/writes SDHI peripheral registers. Must not be called concurrently
/// for the same port.
async unsafe fn send_cmd(port: u8, cmd_val: u16) -> Result<(), SdhiError> {
    unsafe {
        let s = sdhi(port);

        // Wait for clock to be stable
        wait_clk_stable(port);

        // Clear any stale interrupt state
        clear_info(port);

        // Enable response-end and error interrupts.
        // Hardware polarity: 0 = enabled, 1 = masked → write bitwise complement.
        // INFO1: enable RESP only  → ~0x0001 = 0xFFFE
        // INFO2: enable ERR_ALL (includes ILA) → ~INFO2_ERR_ALL = 0x7F80
        s.sd_info1_mask().write_value(!INFO1_RESP);
        s.sd_info2_mask().write_value(!INFO2_ERR_ALL);

        // Issue command
        s.sd_cmd().write_value(cmd_val);

        // Async wait
        let result = wait_resp(port).await;

        // Disable all interrupts (0 = enabled, 1 = masked → 0xFFFF masks all)
        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);

        // Clear accumulated state
        clear_info(port);

        result
    }
}

// ---------------------------------------------------------------------------
// Response reading
// ---------------------------------------------------------------------------

/// Read the 32-bit R1/R3/R6/R7 response (RESP0 + RESP1 combined).
///
/// # Safety
/// Reads SDHI response registers.
unsafe fn read_r1(port: u8) -> u32 {
    let s = sdhi(port);
    let lo = s.sd_rsp00().read() as u32;
    let hi = s.sd_rsp01().read() as u32;
    (hi << 16) | lo
}

/// Read the full 128-bit R2 response (CID or CSD register).
///
/// Returns `[word0, word1, word2, word3]` where word0 contains bits `[127:96]`.
///
/// # Safety
/// Reads SDHI response registers.
unsafe fn read_r2(port: u8) -> [u32; 4] {
    let s = sdhi(port);
    [
        ((s.sd_rsp07().read() as u32) << 16) | s.sd_rsp06().read() as u32,
        ((s.sd_rsp05().read() as u32) << 16) | s.sd_rsp04().read() as u32,
        ((s.sd_rsp03().read() as u32) << 16) | s.sd_rsp02().read() as u32,
        ((s.sd_rsp01().read() as u32) << 16) | s.sd_rsp00().read() as u32,
    ]
}

// ---------------------------------------------------------------------------
// Software (FIFO) data transfer
// ---------------------------------------------------------------------------

/// Read `count` 512-byte blocks from the card into `buf`.
///
/// Caller must have already:
///  1. Set block count: [`set_block_count`]
///  2. Issued CMD17 or CMD18 via [`send_cmd`]
///
/// Transfer is interrupt-driven: the hardware raises INFO2_BRE when the FIFO
/// has 512 bytes ready; we drain it, then repeat for each block.  After all
/// blocks arrive, we wait for INFO1_DATA_TRNS (access end).
///
/// # Safety
/// Reads/writes SDHI registers and the provided buffer.
async unsafe fn read_blocks_sw(port: u8, buf: *mut u8, count: u32) -> Result<(), SdhiError> {
    unsafe {
        let s = sdhi(port);

        // Enable DATA_TRNS (access end), BRE, and all errors.
        // Hardware polarity: 0 = enabled, 1 = masked → write bitwise complement.
        // INFO1: enable DATA_TRNS only  → ~INFO1_DATA_TRNS = 0xFFFB
        // INFO2: enable ERR_ALL + BRE   → ~(INFO2_ERR_ALL | INFO2_BRE) = 0x7E80
        s.sd_info1_mask().write_value(!INFO1_DATA_TRNS);
        s.sd_info2_mask().write_value(!(INFO2_ERR_ALL | INFO2_BRE));

        let mut dst = buf;
        for _block in 0..count {
            // Wait for Buffer Read Enable
            wait_buf_ready(port, INFO2_BRE).await?;

            // Drain 512 bytes from 32-bit FIFO (128 reads)
            let fifo = s.sd_buf0();
            for w in 0..128usize {
                let word = fifo.read();
                let p = dst.add(w * 4) as *mut u32;
                p.write_unaligned(word);
            }
            dst = dst.add(512);
        }

        // Wait for access end (DATA_TRNS)
        wait_access_end(port).await?;

        // Disable all interrupts (0xFFFF = all masked)
        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);
        clear_info(port);

        Ok(())
    }
}

/// Write `count` 512-byte blocks from `buf` to the card.
///
/// Caller must have already set block count and issued CMD24 or CMD25.
///
/// # Safety
/// Reads/writes SDHI registers and the provided buffer.
async unsafe fn write_blocks_sw(port: u8, buf: *const u8, count: u32) -> Result<(), SdhiError> {
    unsafe {
        let s = sdhi(port);

        // Enable DATA_TRNS, BWE, and all errors.
        // Hardware polarity: 0 = enabled, 1 = masked → write bitwise complement.
        // INFO1: enable DATA_TRNS only  → ~INFO1_DATA_TRNS = 0xFFFB
        // INFO2: enable ERR_ALL + BWE   → ~(INFO2_ERR_ALL | INFO2_BWE) = 0x7D80
        s.sd_info1_mask().write_value(!INFO1_DATA_TRNS);
        s.sd_info2_mask().write_value(!(INFO2_ERR_ALL | INFO2_BWE));

        let mut src = buf;
        for _block in 0..count {
            // Wait for Buffer Write Enable
            wait_buf_ready(port, INFO2_BWE).await?;

            // Fill 512 bytes into 32-bit FIFO (128 writes)
            let fifo = s.sd_buf0();
            for w in 0..128usize {
                let p = src.add(w * 4) as *const u32;
                fifo.write_value(p.read_unaligned());
            }
            src = src.add(512);
        }

        // Wait for access end
        wait_access_end(port).await?;

        // No separate card-busy wait is needed here: per the RZ/A1 TRM (§38.2.6,
        // SD_INFO1 Access End), the Access End flag we awaited above is only set
        // for a write *after reception of the busy state following the CRC status*
        // completes. So once INFO1_DATA_TRNS is observed the card has already
        // released D0/CBSY, and the next command can be issued safely.
        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);
        clear_info(port);

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Short (64-byte) status-block transfer — CMD6 switch-function status
// ---------------------------------------------------------------------------

/// Issue an extended-mode single-block read command whose data phase is the
/// 64-byte switch-function status block (CMD6), and read that block.
///
/// Unlike [`read_blocks_sw`], this issues the command itself: `SD_SIZE` must
/// be programmed to 64 *before* the command starts the data transfer.  It is
/// restored to 512 afterwards — on the error path too.
///
/// `cmd_val` must be an extended-mode single-block-read encoding: for CMD6
/// this is `0x1C06` (TRM table 38.7 — `MD[2:0]`=100 extended/R1, MD3=with-data,
/// MD4=read, MD5=0 single-block).  `arg` is the CMD6 mode/function argument.
///
/// # Safety
/// Reads/writes SDHI peripheral registers.  Must not be called concurrently
/// for the same port.
async unsafe fn read_status_block_sw(
    port: u8,
    cmd_val: u16,
    arg: u32,
    buf: &mut [u8; 64],
) -> Result<(), SdhiError> {
    unsafe {
        let s = sdhi(port);

        // SD_SIZE ← 64 before the command (TRM §38.2.11; single-block
        // transfers accept 1–512 bytes).  Never while SCLKDIVEN = 0.
        wait_clk_stable(port);
        s.sd_size().write_value(64);
        // Single block: SEC (block-count enable) off.
        s.sd_stop().write_value(0x0000);
        s.sd_seccnt().write_value(1);

        set_arg(port, arg);
        let result = read_status_block_inner(port, cmd_val, buf).await;

        // Quiesce interrupts and restore the 512-byte sector block size —
        // error path included, so a failed CMD6 can't poison later sector IO.
        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);
        clear_info(port);
        wait_clk_stable(port);
        s.sd_size().write_value(512);

        result
    }
}

/// Body of [`read_status_block_sw`]: command issue + 64-byte FIFO drain.
/// Split out so the caller can restore `SD_SIZE` on every exit path.
async unsafe fn read_status_block_inner(
    port: u8,
    cmd_val: u16,
    buf: &mut [u8; 64],
) -> Result<(), SdhiError> {
    unsafe {
        let s = sdhi(port);

        // Issue the command and await the R1 response.
        send_cmd(port, cmd_val).await?;

        // Enable DATA_TRNS (access end), BRE, and all errors.
        // Hardware polarity: 0 = enabled, 1 = masked → write bitwise complement.
        s.sd_info1_mask().write_value(!INFO1_DATA_TRNS);
        s.sd_info2_mask().write_value(!(INFO2_ERR_ALL | INFO2_BRE));

        // Wait for FIFO data (BRE), drain 64 bytes (16 × 32-bit reads).
        wait_buf_ready(port, INFO2_BRE).await?;
        let fifo = s.sd_buf0();
        for w in 0..16usize {
            let word = fifo.read();
            let p = buf.as_mut_ptr().add(w * 4) as *mut u32;
            p.write_unaligned(word);
        }

        // Wait for access end.
        wait_access_end(port).await
    }
}

// ---------------------------------------------------------------------------
// DMA data transfer
// ---------------------------------------------------------------------------

/// Read `count` 512-byte blocks from the card into `buf` using DMAC.
///
/// Handles the complete DMA transfer sequence per TRM §38.4.9:
/// 1. Enables SDHI DMA mode (`CC_EXT_MODE = 0x0002`) before issuing the command.
/// 2. Issues `cmd_val` (CMD17 or CMD18) and waits for response end.
/// 3. Configures and starts the DMAC (SD_BUF0 → `buf`).
/// 4. Waits for `INFO1_DATA_TRNS` via the SDHI interrupt.
///
/// Caller must have already called [`set_block_count`] and [`set_arg`].
///
/// # Safety
/// * `buf` must point to `count × 512` bytes of valid **uncached** memory
///   (virtual address in the uncached OCRAM mirror, i.e. physical +
///   `rza1::UNCACHED_MIRROR_OFFSET`).  Using a cached buffer results in
///   undefined cache coherence behaviour.
/// * Must not be called concurrently on the same port.
async unsafe fn read_blocks_dma(
    port: u8,
    cmd_val: u16,
    buf: *mut u8,
    count: u32,
    dma: &mut crate::dmac::Dma<'_>,
) -> Result<(), SdhiError> {
    unsafe {
        let s = sdhi(port);
        let dmars_rx = if port == 0 {
            DMARS_SDHI0_RX
        } else {
            DMARS_SDHI1_RX
        };
        let sd_buf0 = s.sd_buf0().addr() as u32;

        // 1. Enable DMA mode BEFORE issuing the command (TRM §38.4.9).
        s.cc_ext_mode().write_value(CC_EXT_MODE_DMASDRW);

        // 2. Issue command and wait for response end (mirrors send_cmd).
        wait_clk_stable(port);
        clear_info(port);
        s.sd_info1_mask().write_value(!INFO1_RESP);
        s.sd_info2_mask().write_value(!INFO2_ERR_ALL);
        s.sd_cmd().write_value(cmd_val);

        let cmd_result = wait_resp(port).await;

        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);
        clear_info(port);

        if let Err(e) = cmd_result {
            s.cc_ext_mode().write_value(0x0000);
            return Err(e);
        }

        // 3. Configure DMAC: SD_BUF0 (fixed source) → buf (incrementing dest).
        dma.init_register_mode_rx(chcfg_sdhi_rx(dma.number()), sd_buf0, dmars_rx);

        // 4. Enable DATA_TRNS + error interrupts, then kick the DMAC.
        s.sd_info1_mask().write_value(!INFO1_DATA_TRNS);
        s.sd_info2_mask().write_value(!INFO2_ERR_ALL);
        dma.start_transfer_rx(buf as u32, count * 512);

        // 5. Wait for transfer complete (DATA_TRNS via SDHI interrupt).
        let data_result = wait_access_end(port).await;

        // On an error the SD_BUF DMA request stays asserted (HW manual ch. 38, DMA transfer): stop
        // the DMAC before returning, or it keeps writing into the caller's buffer.
        if data_result.is_err() && !crate::dmac::halt_channel(dma.number()) {
            warn!("sdhi{}: DMA channel {} didn't stop after a read error", port, dma.number());
        }

        // 5b. For RX: wait for the DMAC to drain its final burst into memory.
        // DATA_TRNS fires when the SD-bus transfer ends; the DMAC may still
        // be writing the last burst to the destination buffer.  Reading the
        // buffer before CHSTAT.TC would observe stale data.
        // (`dma` has its completion interrupt: built with `Dma::new`.)
        if data_result.is_ok() {
            dma.wait_transfer_complete().await;
        }

        // 6. Restore software transfer mode.
        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);
        clear_info(port);
        s.cc_ext_mode().write_value(0x0000);

        data_result
    }
}

/// Write `count` 512-byte blocks from `buf` to the card using DMAC.
///
/// Handles the complete DMA transfer sequence per TRM §38.4.9:
/// 1. Enables SDHI DMA mode (`CC_EXT_MODE = 0x0002`) before issuing the command.
/// 2. Issues `cmd_val` (CMD24 or CMD25) and waits for response end.
/// 3. Configures and starts the DMAC (`buf` → SD_BUF0).
/// 4. Waits for `INFO1_DATA_TRNS` via the SDHI interrupt.
///
/// Caller must have already called [`set_block_count`] and [`set_arg`].
///
/// # Safety
/// * `buf` must point to `count × 512` bytes of valid **uncached** memory
///   so the DMAC reads the CPU-written bytes without stale cache lines.
/// * Must not be called concurrently on the same port.
async unsafe fn write_blocks_dma(
    port: u8,
    cmd_val: u16,
    buf: *const u8,
    count: u32,
    dma: &mut crate::dmac::Dma<'_>,
) -> Result<(), SdhiError> {
    unsafe {
        let s = sdhi(port);
        let dmars_tx = if port == 0 {
            DMARS_SDHI0_TX
        } else {
            DMARS_SDHI1_TX
        };
        let sd_buf0 = s.sd_buf0().addr() as u32;

        // 1. Enable DMA mode BEFORE issuing the command (TRM §38.4.9).
        s.cc_ext_mode().write_value(CC_EXT_MODE_DMASDRW);

        // 2. Issue command and wait for response end.
        wait_clk_stable(port);
        clear_info(port);
        s.sd_info1_mask().write_value(!INFO1_RESP);
        s.sd_info2_mask().write_value(!INFO2_ERR_ALL);
        s.sd_cmd().write_value(cmd_val);

        let cmd_result = wait_resp(port).await;

        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);
        clear_info(port);

        if let Err(e) = cmd_result {
            s.cc_ext_mode().write_value(0x0000);
            return Err(e);
        }

        // 3. Configure DMAC: buf (incrementing source) → SD_BUF0 (fixed dest).
        dma.init_register_mode(chcfg_sdhi_tx(dma.number()), sd_buf0, dmars_tx);

        // 4. Enable DATA_TRNS + error interrupts, then kick the DMAC.
        s.sd_info1_mask().write_value(!INFO1_DATA_TRNS);
        s.sd_info2_mask().write_value(!INFO2_ERR_ALL);
        dma.start_transfer(buf as u32, count * 512);

        // 5. Wait for transfer complete (DATA_TRNS via SDHI interrupt).
        let data_result = wait_access_end(port).await;

        // On an error the SD_BUF DMA request stays asserted (HW manual ch. 38, DMA transfer): stop
        // the DMAC before returning.
        if data_result.is_err() && !crate::dmac::halt_channel(dma.number()) {
            warn!("sdhi{}: DMA channel {} didn't stop after a write error", port, dma.number());
        }

        // No separate card-busy wait is needed here: per the RZ/A1 TRM (§38.2.6,
        // SD_INFO1 Access End), the Access End flag we awaited above (DATA_TRNS) is
        // only set for a write *after reception of the busy state following the CRC
        // status of the last block* completes (or, with auto-CMD12, after the CMD12
        // response-busy completes). So once DATA_TRNS is observed the card has
        // already released D0/CBSY and the next command can be issued safely.

        // 6. Restore software transfer mode.
        s.sd_info1_mask().write_value(0xFFFF);
        s.sd_info2_mask().write_value(0xFFFF);
        clear_info(port);
        s.cc_ext_mode().write_value(0x0000);

        data_result
    }
}

// ---------------------------------------------------------------------------
// Block-count / stop-transfer helpers
// ---------------------------------------------------------------------------

/// Set the number of blocks for a multi-block transfer.
///
/// Programs SD_SECCNT and the SD_STOP block-count-enable (SEC) bit.  Note SEC
/// only bounds how many blocks the *controller* clocks in (so DATA_TRNS fires
/// at the right count) — it does **not** issue CMD12 to the card.  Whether
/// CMD12 is sent is governed by the SD_CMD mode bits: a command in extended
/// mode with `[15:14]`=01 (e.g. `CMD18 | 0x7C00`) suppresses the auto-CMD12, so
/// the caller must issue [`stop_transfer`] manually after the transfer.
///
/// # Safety
/// Writes SDHI registers.
unsafe fn set_block_count(port: u8, count: u32) {
    let s = sdhi(port);
    s.sd_seccnt().write_value(count as u16);
    // Block-count enable (SEC, bit 8) when count > 1; otherwise disabled.
    if count > 1 {
        s.sd_stop().write_value(0x0100);
    } else {
        s.sd_stop().write_value(0x0000);
    }
}

/// Issue CMD12 (STOP_TRANSMISSION) to abort a multi-block transfer.
///
/// # Safety
/// Writes SDHI registers.
async unsafe fn stop_transfer(port: u8) -> Result<(), SdhiError> {
    unsafe {
        set_arg(port, 0);
        send_cmd(port, 12u16).await // CMD12
    }
}

// ---------------------------------------------------------------------------
// Card detect
// ---------------------------------------------------------------------------

/// Return `true` if a card is physically inserted (CD pin low = card present).
///
/// Reads INFO1 bit 5 (INFO5 = current SD_CD level) directly from hardware.
/// Bit 5 = 1 means SD_CD has been held low for at least Ncycle → card present.
///
/// Do NOT use INFO1_INS_CD (bit 4) here — that is an edge-triggered latch
/// (set on insertion, must be cleared) not the steady-state level.
///
/// # Safety
/// Reads SDHI INFO1 register.
unsafe fn card_inserted(port: u8) -> bool {
    let s = sdhi(port);
    let info1 = s.sd_info1().read();
    // INFO1_CD_LEVEL (bit 5 = INFO5): 1 = SD_CD held low = card present.
    info1 & INFO1_CD_LEVEL != 0
}

/// Return `true` if the card's write-protect (lock) tab is engaged (read-only).
///
/// Reads SD_INFO1 bit 7 (INFO7 = SD_WP level).  **The Deluge socket inverts the
/// usual polarity**: per the RZ/A1 manual INFO7 = 1 means the `SD_WP` pin is low,
/// which a standard pulled-up socket asserts when the lock tab is engaged — but
/// on the Deluge board the engaged tab drives `SD_WP` *high* (INFO7 = 0), so here
/// a **clear** INFO7 means write-protected (verified on hardware with known
/// locked and unlocked cards).
///
/// # Safety
/// Reads SDHI INFO1 register.
unsafe fn card_write_protected(port: u8) -> bool {
    let s = sdhi(port);
    s.sd_info1().read() & INFO1_WP == 0
}

/// Consume the latched card-detect *edges* for `port`, as `(removed, inserted)`.
///
/// Unlike [`card_inserted`], which reads the steady-state SD_CD level, these are
/// hardware edge latches — so a card removed and a different one inserted entirely
/// between two calls still reports `(true, true)`, where two level reads would
/// compare equal and look like nothing happened. Callers must handle both being
/// set: it means the card present now is not the one present before, and anything
/// cached about the old card (RCA, capacity, mounted filesystem) is stale.
///
/// Both sources of an edge are drained:
/// - the hardware INFO1 latch, which is the usual one — card-detect interrupts sit
///   masked almost all the time (every transfer restores `INFO1_MASK = 0xFFFF`), so
///   normally no ISR runs for these and the bits just accumulate in the register;
/// - the sticky flags [`interrupt_handler`] sets when an unrelated SDHI interrupt
///   happened to clear the latch first.
///
/// # Safety
/// Reads and writes the SDHI INFO1 register.
unsafe fn take_card_detect_events(port: u8) -> (bool, bool) {
    let s = sdhi(port);
    let latched = s.sd_info1().read() & INFO1_DET_CD;
    if latched != 0 {
        // Write-0-to-clear: `!latched` writes 0 to exactly the bits we read as
        // set and 1 (= leave alone) everywhere else, so a response or
        // transfer-end flag that gets set between the read and this write is
        // not cleared out from under the waiter blocked on it.
        s.sd_info1().write_value(!latched);
    }
    let st = &STATE[port as usize];
    // Swap both before combining: `||` would short-circuit past the second
    // swap and leave a stale sticky flag set for the next caller to re-report.
    let sticky_removed = st.cd_removed.swap(false, Ordering::AcqRel);
    let sticky_inserted = st.cd_inserted.swap(false, Ordering::AcqRel);
    (
        latched & INFO1_REM_CD != 0 || sticky_removed,
        latched & INFO1_INS_CD != 0 || sticky_inserted,
    )
}

// ---------------------------------------------------------------------------
// Card capacity (set by deluge-bsp after CSD decode)
// ---------------------------------------------------------------------------

/// Total card capacity in 512-byte blocks, per port.
///
/// Populated by the BSP layer via [`set_card_blocks`] during card init so
/// that the polling `BlockDevice` impl can return a reliable block count.
static CARD_BLOCKS: [AtomicU32; NUM_PORTS] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Store the card capacity (in 512-byte blocks) for `port`.
///
/// Called by the BSP once the CSD register has been decoded.
fn set_card_blocks(port: u8, blocks: u32) {
    CARD_BLOCKS[port as usize].store(blocks, Ordering::Release);
}

/// Return the stored card capacity (in 512-byte blocks) for `port`.
///
/// Returns 0 if [`set_card_blocks`] has not been called yet.
fn card_size_blocks(port: u8) -> u32 {
    CARD_BLOCKS[port as usize].load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Owned typed wrapper
// ---------------------------------------------------------------------------

// ── Owned driver ─────────────────────────────────────────────────────────────

use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use crate::Peri;
use crate::dmac::Dma;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

pin_trait!(
    /// A pin that can carry SD port `T`'s clock (SD_CLK_n).
    ClkPin, Instance
);
pin_trait!(
    /// A pin that can carry SD port `T`'s command line (SD_CMD_n).
    CmdPin, Instance
);
pin_trait!(
    /// A pin that can carry SD port `T`'s data bit 0 (SD_D0_n).
    D0Pin, Instance
);
pin_trait!(
    /// A pin that can carry SD port `T`'s data bit 1 (SD_D1_n).
    D1Pin, Instance
);
pin_trait!(
    /// A pin that can carry SD port `T`'s data bit 2 (SD_D2_n).
    D2Pin, Instance
);
pin_trait!(
    /// A pin that can carry SD port `T`'s data bit 3 (SD_D3_n).
    D3Pin, Instance
);
pin_trait!(
    /// A pin that can carry SD port `T`'s card detect (SD_CD_n).
    CdPin, Instance
);
pin_trait!(
    /// A pin that can carry SD port `T`'s write protect (SD_WP_n).
    WpPin, Instance
);

/// An SDHI channel singleton (`peripherals::SDHI0`, `SDHI1`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The port number, 0 or 1.
    const PORT: u8;
    /// Its status (card-detect) interrupt.
    type StatusInterrupt: Interrupt;
    /// Its operation (command and transfer) interrupt.
    type OperationInterrupt: Interrupt;
}

macro_rules! instances {
    ($($sdhi:ident = $n:literal, $status:ident, $op:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$sdhi {}
        impl Instance for crate::peripherals::$sdhi {
            const PORT: u8 = $n;
            type StatusInterrupt = crate::interrupt::typelevel::$status;
            type OperationInterrupt = crate::interrupt::typelevel::$op;
        }
    )*};
}

instances! {
    SDHI0 = 0, SDHI0_STATUS, SDHI0_OP;
    SDHI1 = 1, SDHI1_STATUS, SDHI1_OP;
}

/// SDHI `T`'s status interrupt (card insert/remove): latches the status and wakes the waiter.
pub struct StatusInterruptHandler<T: Instance> {
    _sdhi: PhantomData<T>,
}

impl<T: Instance> Handler<T::StatusInterrupt> for StatusInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: the GIC dispatches this for T's status interrupt only.
        unsafe { interrupt_handler(T::PORT) }
    }
}

/// SDHI `T`'s operation interrupt (response, transfer end, errors).
pub struct OperationInterruptHandler<T: Instance> {
    _sdhi: PhantomData<T>,
}

impl<T: Instance> Handler<T::OperationInterrupt> for OperationInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: as above, for T's operation interrupt.
        unsafe { interrupt_handler(T::PORT) }
    }
}

/// An SD host controller, with its DMA channels.
///
/// Its methods take `&self`, so firmware can keep it in a static. The card protocol is a
/// sequence: issue one command (and its transfer) at a time, serialised by the caller. A command
/// or transfer dropped mid-flight resets the whole controller ([`take_aborted`](Self::take_aborted)),
/// so one from another task running alongside it would never complete.
pub struct Sdhi<'d> {
    port: u8,
    /// Whether a command or transfer was dropped mid-flight ([`take_aborted`](Self::take_aborted)).
    aborted: AtomicBool,
    /// The board's SD_OPTION, which [`reset`](Self::reset) restores.
    sd_option: u16,
    /// The DMA channels, each held by one block transfer at a time across its awaits.
    tx_dma: Mutex<CriticalSectionRawMutex, Dma<'d>>,
    rx_dma: Mutex<CriticalSectionRawMutex, Dma<'d>>,
    _sdhi: PhantomData<&'d mut ()>,
}

impl<'d> Sdhi<'d> {
    /// Bring SDHI `T` up on its eight pins (put on the SDHI's function first): module clock, soft
    /// reset, interrupt masks, `sd_option` (bus width and timeouts), 512-byte blocks,
    /// identification clock. Binds and enables its status and
    /// operation interrupts; SDIO stays masked.
    ///
    /// `rx_dma` must have its completion interrupt (a block read waits for its last burst);
    /// `tx_dma` needs none.
    #[allow(clippy::too_many_arguments)]
    pub fn new<T: Instance>(
        _sdhi: Peri<'d, T>,
        clk: Peri<'d, impl ClkPin<T>>,
        cmd: Peri<'d, impl CmdPin<T>>,
        d0: Peri<'d, impl D0Pin<T>>,
        d1: Peri<'d, impl D1Pin<T>>,
        d2: Peri<'d, impl D2Pin<T>>,
        d3: Peri<'d, impl D3Pin<T>>,
        cd: Peri<'d, impl CdPin<T>>,
        wp: Peri<'d, impl WpPin<T>>,
        mut tx_dma: Dma<'d>,
        rx_dma: Dma<'d>,
        irq: impl Binding<T::StatusInterrupt, StatusInterruptHandler<T>>
        + Binding<T::OperationInterrupt, OperationInterruptHandler<T>>
        + Copy
        + 'd,
        sd_option: u16,
    ) -> Self {
        let alts = [
            clk.alternate(),
            cmd.alternate(),
            d0.alternate(),
            d1.alternate(),
            d2.alternate(),
            d3.alternate(),
            cd.alternate(),
            wp.alternate(),
        ];
        crate::gpio::mux(clk, alts[0]);
        crate::gpio::mux(cmd, alts[1]);
        crate::gpio::mux(d0, alts[2]);
        crate::gpio::mux(d1, alts[3]);
        crate::gpio::mux(d2, alts[4]);
        crate::gpio::mux(d3, alts[5]);
        crate::gpio::mux(cd, alts[6]);
        crate::gpio::mux(wp, alts[7]);
        // A write never waits for its DMA completion (DATA_TRNS ends it), so its line stays off.
        tx_dma.mask_interrupt();
        // SAFETY: SDHI T is owned.
        unsafe { init(T::PORT, sd_option) };
        interrupt::bind::<T::StatusInterrupt, StatusInterruptHandler<T>>(irq);
        interrupt::bind::<T::OperationInterrupt, OperationInterruptHandler<T>>(irq);
        // SAFETY: T's own interrupts; their priorities are the application's.
        unsafe {
            T::StatusInterrupt::enable();
            T::OperationInterrupt::enable();
        }
        Self {
            port: T::PORT,
            aborted: AtomicBool::new(false),
            sd_option,
            tx_dma: Mutex::new(tx_dma),
            rx_dma: Mutex::new(rx_dma),
            _sdhi: PhantomData,
        }
    }

    /// Return the controller to the state [`new`](Self::new) left it in, ready to identify a
    /// card: soft reset, interrupt masks, `sd_option`, 512-byte blocks and the ~130 kHz
    /// identification clock. Bindings and GIC enables are kept.
    ///
    /// Run it before re-identifying a card: the previous card may have left the clock at
    /// 16.7 or 33.3 MHz, far above identification mode's 400 kHz, or the controller wedged
    /// by a pull mid-transfer. No command or transfer may be in flight.
    pub fn reset(&self) {
        // SAFETY: the owned controller; nothing is in flight (this method's contract).
        unsafe { init(self.port, self.sd_option) }
    }

    /// Whether a command or transfer was dropped mid-flight since the last call. If so the
    /// controller has been stopped and reset to identification mode, and the card must be
    /// re-identified before it is used again.
    ///
    /// The controller runs one command or transfer at a time, and the caller serialises them (as
    /// `deluge-bsp`'s SD layer does with one lock): an abort resets the whole controller, so an
    /// operation another task had in flight on it would never complete.
    pub fn take_aborted(&self) -> bool {
        self.aborted.swap(false, Ordering::AcqRel)
    }

    /// A guard that, if dropped before it is defused, aborts the command or transfer in flight:
    /// the DMA channel `dma` (if any) halted, the SD transfer stopped (SD_STOP.STP), the
    /// controller soft-reset to identification mode, and the abort recorded for
    /// [`take_aborted`](Self::take_aborted). Synchronous: nothing runs on after the future is gone.
    fn abort_on_drop(&self, dma: Option<u8>) -> OnDrop<impl FnOnce() + '_> {
        OnDrop::new(move || {
            if let Some(ch) = dma
                && !crate::dmac::halt_channel(ch)
            {
                warn!("sdhi{}: DMA channel {} didn't stop on abort", self.port, ch);
            }
            // STP stays set: the soft reset in `init` clears it and ends any CMD12 sequence
            // (HW manual §38.2.3 note 3 forbids clearing STP while one may still run).
            sdhi(self.port).sd_stop().write_value(SD_STOP_STP);
            // SAFETY: the owned controller; the transfer in flight is stopped above.
            unsafe { init(self.port, self.sd_option) };
            self.aborted.store(true, Ordering::Release);
        })
    }

    /// Set the SD clock divider (`CLK_DIV_*`). `CLK_DIV_2` needs a High-Speed card.
    pub fn set_clock_div(&self, div: u16) {
        // SAFETY: the owned controller.
        unsafe { set_clock_div(self.port, div) }
    }

    /// The default-speed clock (P1/4, ~16.7 MHz).
    pub fn set_clock_fast(&self) {
        // SAFETY: the owned controller.
        unsafe { set_clock_fast(self.port) }
    }

    /// The next command's argument.
    pub fn set_arg(&self, arg: u32) {
        // SAFETY: the owned controller.
        unsafe { set_arg(self.port, arg) }
    }

    /// Issue a command and wait for its response.
    pub async fn send_cmd(&self, cmd_val: u16) -> Result<(), SdhiError> {
        let guard = self.abort_on_drop(None);
        // SAFETY: the owned controller.
        let r = unsafe { send_cmd(self.port, cmd_val).await };
        guard.defuse();
        r
    }

    /// The short (R1) response.
    pub fn read_r1(&self) -> u32 {
        // SAFETY: a read of the owned controller.
        unsafe { read_r1(self.port) }
    }

    /// The long (R2) response.
    pub fn read_r2(&self) -> [u32; 4] {
        // SAFETY: a read of the owned controller.
        unsafe { read_r2(self.port) }
    }

    /// The next transfer's block count.
    pub fn set_block_count(&self, count: u32) {
        // SAFETY: the owned controller.
        unsafe { set_block_count(self.port, count) }
    }

    /// End a multi-block transfer (CMD12).
    pub async fn stop_transfer(&self) -> Result<(), SdhiError> {
        let guard = self.abort_on_drop(None);
        // SAFETY: the owned controller.
        let r = unsafe { stop_transfer(self.port).await };
        guard.defuse();
        r
    }

    /// Whether a card is in the slot.
    pub fn card_inserted(&self) -> bool {
        // SAFETY: a read of the owned controller.
        unsafe { card_inserted(self.port) }
    }

    /// Whether the card's write-protect switch is on.
    pub fn card_write_protected(&self) -> bool {
        // SAFETY: a read of the owned controller.
        unsafe { card_write_protected(self.port) }
    }

    /// Card removal and insertion edges since the last call, as `(removed, inserted)`.
    pub fn take_card_detect_events(&self) -> (bool, bool) {
        // SAFETY: the owned controller.
        unsafe { take_card_detect_events(self.port) }
    }

    /// Record the card's size in 512-byte blocks.
    pub fn set_card_blocks(&self, blocks: u32) {
        set_card_blocks(self.port, blocks)
    }

    /// The card's size in 512-byte blocks, as recorded.
    pub fn card_size_blocks(&self) -> u32 {
        card_size_blocks(self.port)
    }

    /// Issue `cmd_val` with `arg` and read its 64-byte data block by PIO (CMD6 status).
    pub async fn read_status_block_sw(
        &self,
        cmd_val: u16,
        arg: u32,
        buf: &mut [u8; 64],
    ) -> Result<(), SdhiError> {
        let guard = self.abort_on_drop(None);
        // SAFETY: the owned controller; `buf` is a live 64-byte buffer.
        let r = unsafe { read_status_block_sw(self.port, cmd_val, arg, buf).await };
        guard.defuse();
        r
    }

    /// Read `buf.len() / 512` blocks into `buf` by PIO (any memory: the CPU copies). The command
    /// has already been issued.
    pub async fn read_blocks_sw(&self, buf: &mut [u8]) -> Result<(), SdhiError> {
        let count = block_count(buf.len())?;
        let guard = self.abort_on_drop(None);
        // SAFETY: `buf` holds `count * 512` writable bytes, borrowed for the whole transfer.
        let r = unsafe { read_blocks_sw(self.port, buf.as_mut_ptr(), count).await };
        guard.defuse();
        r
    }

    /// Write `buf.len() / 512` blocks from `buf` by PIO. The command has already been issued.
    pub async fn write_blocks_sw(&self, buf: &[u8]) -> Result<(), SdhiError> {
        let count = block_count(buf.len())?;
        let guard = self.abort_on_drop(None);
        // SAFETY: `buf` holds `count * 512` readable bytes, borrowed for the whole transfer.
        let r = unsafe { write_blocks_sw(self.port, buf.as_ptr(), count).await };
        guard.defuse();
        r
    }

    /// Issue `cmd_val` and read `buf.len() / 512` blocks into `buf` by DMA.
    pub async fn read_blocks_dma(
        &self,
        cmd_val: u16,
        mut buf: UncachedMut<'_>,
    ) -> Result<(), SdhiError> {
        let count = block_count(buf.len())?;
        let mut rx = self.rx_dma.lock().await;
        let guard = self.abort_on_drop(Some(rx.number()));
        // SAFETY: `buf` is `count * 512` bytes of uncached memory, borrowed for the transfer (and
        // the guard stops the DMAC before the borrow ends if the transfer is dropped).
        let r = unsafe {
            read_blocks_dma(self.port, cmd_val, buf.as_mut_slice().as_mut_ptr(), count, &mut rx)
                .await
        };
        guard.defuse();
        r
    }

    /// Issue `cmd_val` and write `buf.len() / 512` blocks from `buf` by DMA.
    pub async fn write_blocks_dma(&self, cmd_val: u16, buf: Uncached<'_>) -> Result<(), SdhiError> {
        let count = block_count(buf.len())?;
        let mut tx = self.tx_dma.lock().await;
        let guard = self.abort_on_drop(Some(tx.number()));
        // SAFETY: `buf` is `count * 512` bytes of uncached memory, borrowed for the transfer (and
        // the guard stops the DMAC before the borrow ends if the transfer is dropped).
        let r = unsafe {
            write_blocks_dma(self.port, cmd_val, buf.as_slice().as_ptr(), count, &mut tx).await
        };
        guard.defuse();
        r
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn the_pac_names_the_same_sdhi_registers() {
        for (port, base) in [(0u8, 0xE804_E000usize), (1, 0xE804_E800)] {
            let s = sdhi(port);
            let got = [
                (s.sd_cmd().addr(), s.sd_cmd().width()),
                (s.sd_arg0().addr(), s.sd_arg0().width()),
                (s.sd_arg1().addr(), s.sd_arg1().width()),
                (s.sd_stop().addr(), s.sd_stop().width()),
                (s.sd_seccnt().addr(), s.sd_seccnt().width()),
                (s.sd_rsp00().addr(), s.sd_rsp00().width()),
                (s.sd_rsp07().addr(), s.sd_rsp07().width()),
                (s.sd_info1().addr(), s.sd_info1().width()),
                (s.sd_info2().addr(), s.sd_info2().width()),
                (s.sd_info1_mask().addr(), s.sd_info1_mask().width()),
                (s.sd_info2_mask().addr(), s.sd_info2_mask().width()),
                (s.sd_clk_ctrl().addr(), s.sd_clk_ctrl().width()),
                (s.sd_size().addr(), s.sd_size().width()),
                (s.sd_option().addr(), s.sd_option().width()),
                (s.sd_err_sts1().addr(), s.sd_err_sts1().width()),
                (s.sd_err_sts2().addr(), s.sd_err_sts2().width()),
                (s.sd_buf0().addr(), s.sd_buf0().width()),
                (s.sdio_mode().addr(), s.sdio_mode().width()),
                (s.sdio_info1().addr(), s.sdio_info1().width()),
                (s.sdio_info1_mask().addr(), s.sdio_info1_mask().width()),
                (s.cc_ext_mode().addr(), s.cc_ext_mode().width()),
                (s.soft_rst().addr(), s.soft_rst().width()),
                (s.ext_swap().addr(), s.ext_swap().width()),
            ];
            let want = [
                (base, 2),
                (base + 0x04, 2),
                (base + 0x06, 2),
                (base + 0x08, 2),
                (base + 0x0A, 2),
                (base + 0x0C, 2),
                (base + 0x1A, 2),
                (base + 0x1C, 2),
                (base + 0x1E, 2),
                (base + 0x20, 2),
                (base + 0x22, 2),
                (base + 0x24, 2),
                (base + 0x26, 2),
                (base + 0x28, 2),
                (base + 0x2C, 2),
                (base + 0x2E, 2),
                (base + 0x30, 4),
                (base + 0x34, 2),
                (base + 0x36, 2),
                (base + 0x38, 2),
                (base + 0xD8, 2),
                (base + 0xE0, 2),
                (base + 0xF0, 2),
            ];
            assert_eq!(got, want, "SDHI{port}");
        }
        assert_eq!(crate::pac::CPG.stbcr12().addr(), 0xFCFE_0444);
    }

    #[test]
    fn info_bit_masks() {
        assert_eq!(INFO1_RESP, 0x0001);
        assert_eq!(INFO1_DATA_TRNS, 0x0004);
        assert_eq!(INFO2_BRE, 0x0100);
        assert_eq!(INFO2_BWE, 0x0200);
        assert_eq!(INFO2_CBSY, 0x4000);
    }

    #[test]
    fn gic_ids_for_sdhi1() {
        assert_eq!(SDHI1_IRQS, [305, 306, 307]);
    }

    #[test]
    fn gic_ids_for_sdhi0() {
        assert_eq!(SDHI0_IRQS, [302, 303, 304]);
    }

    #[test]
    fn dma_chcfg_direction_encoding() {
        // TX (mem -> SD_BUF0): destination fixed + destination-triggered.
        assert_eq!(chcfg_sdhi_tx(0) & CHCFG_DAD, CHCFG_DAD);
        assert_eq!(chcfg_sdhi_tx(0) & CHCFG_REQD, CHCFG_REQD);
        assert_eq!(chcfg_sdhi_tx(0) & CHCFG_SAD, 0);
        // RX (SD_BUF0 -> mem): source fixed, source-triggered (REQD clear).
        assert_eq!(chcfg_sdhi_rx(0) & CHCFG_SAD, CHCFG_SAD);
        assert_eq!(chcfg_sdhi_rx(0) & CHCFG_REQD, 0);
        assert_eq!(chcfg_sdhi_rx(0) & CHCFG_DAD, 0);
        // DMA channel encoded in the low 3 bits.
        assert_eq!(chcfg_sdhi_tx(11) & 7, 11 & 7);
        assert_eq!(chcfg_sdhi_rx(14) & 7, 14 & 7);
    }

    #[test]
    fn clock_dividers_match_trm() {
        // SD_CLK_CTRL[7:0] divider select (TRM ch.38 §38.2.10).
        assert_eq!(CLK_DIV_512, 0x80); // ~130 kHz identification clock
        assert_eq!(CLK_DIV_4, 0x01); //  ~16.7 MHz default-speed clock
        assert_eq!(CLK_DIV_2, 0x00); //  ~33.3 MHz high-speed clock
        // Non-zero encodings are one-hot; 0x00 (= P1/2) is the sole legal
        // all-zero encoding.
        assert_eq!(CLK_DIV_512.count_ones(), 1);
        assert_eq!(CLK_DIV_4.count_ones(), 1);
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod typed_tests {
    use super::*;
    use crate::dmac::{self, Dma};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH14, SDHI1};

    crate::bind_interrupts!(struct Irqs {
        SDHI1_STATUS => StatusInterruptHandler<SDHI1>;
        SDHI1_OP => OperationInterruptHandler<SDHI1>;
        DMAINT14 => dmac::InterruptHandler<DMA_CH14>;
    });

    fn sdhi1() -> Sdhi<'static> {
        let tx = Dma::new_without_interrupt(unsafe { DMA_CH11::steal() });
        let rx = Dma::new(unsafe { DMA_CH14::steal() }, Irqs);
        use crate::peripherals::*;
        unsafe {
            Sdhi::new(
                SDHI1::steal(),
                P7_4::steal(),
                P7_5::steal(),
                P7_3::steal(),
                P7_2::steal(),
                P7_7::steal(),
                P7_6::steal(),
                P7_0::steal(),
                P7_1::steal(),
                tx,
                rx,
                Irqs,
                0x00BD,
            )
        }
    }

    /// The tests here share SDHI1's interrupt state (`STATE[1]`, a process-wide static), and test
    /// threads run in parallel: each holds this lock so they don't race on it.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Poll `f` once, without a waker that does anything.
    fn poll_once<F: core::future::Future>(f: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        f.poll(&mut core::task::Context::from_waker(core::task::Waker::noop()))
    }

    /// The response-end and access-end flags the operation interrupt would record.
    fn interrupt_saw(info1: u16) {
        STATE[1].info1.fetch_or(info1, Ordering::Release);
    }

    fn abort_writes() -> (bool, bool, bool) {
        let s = sdhi(1);
        let dma = crate::pac::DMAC.ch(14).chctrl().addr();
        let w = test::writes();
        (
            w.iter().any(|&(a, v)| a == s.sd_stop().addr() && v & 1 != 0),
            w.iter().any(|&(a, v)| a == s.soft_rst().addr() && v == SOFT_RST_DEASSERT as u32),
            w.iter().any(|&(a, v)| a == dma && v & (1 << 1) != 0),
        )
    }

    #[test]
    fn a_read_dropped_in_its_command_phase_aborts() {
        let _serial = serial();
        let sdhi = sdhi1();
        let mut storage = [0u8; 512];
        test::reset();
        {
            let read = sdhi.read_blocks_dma(17, crate::dma_buf::UncachedMut::new(&mut storage).unwrap());
            let mut read = core::pin::pin!(read);
            assert!(poll_once(read.as_mut()).is_pending(), "waits for the response");
        }
        let (stp, reset, _) = abort_writes();
        assert!(stp && reset, "SD_STOP.STP and a soft reset");
        // STP stays set until the soft reset (HW manual §38.2.3 note 3: don't clear STP while a
        // CMD12 sequence may still run); SDRST itself ends the sequence.
        let stop = super::sdhi(1).sd_stop().addr();
        let last_stop = test::writes().into_iter().filter(|&(a, _)| a == stop).last();
        assert_eq!(last_stop.map(|(_, v)| v), Some(SD_STOP_STP as u32), "STP not cleared by hand");
        assert!(sdhi.take_aborted());
        assert!(!sdhi.take_aborted(), "taken once");
    }

    #[test]
    fn a_read_dropped_in_its_data_phase_halts_the_dma() {
        let _serial = serial();
        let sdhi = sdhi1();
        let mut storage = [0u8; 1024];
        test::reset();
        {
            let read = sdhi.read_blocks_dma(18, crate::dma_buf::UncachedMut::new(&mut storage).unwrap());
            let mut read = core::pin::pin!(read);
            assert!(poll_once(read.as_mut()).is_pending());
            interrupt_saw(INFO1_RESP);
            assert!(poll_once(read.as_mut()).is_pending(), "waits for the data");
        }
        let (stp, reset, dma_halted) = abort_writes();
        assert!(stp && reset && dma_halted, "STP {stp}, reset {reset}, DMA halted {dma_halted}");
        assert!(sdhi.take_aborted());
    }

    #[test]
    fn a_write_dropped_mid_flight_aborts() {
        let _serial = serial();
        let sdhi = sdhi1();
        let storage = [0u8; 512];
        test::reset();
        {
            let write = sdhi.write_blocks_dma(24, crate::dma_buf::Uncached::new(&storage).unwrap());
            let mut write = core::pin::pin!(write);
            assert!(poll_once(write.as_mut()).is_pending());
            interrupt_saw(INFO1_RESP);
            assert!(poll_once(write.as_mut()).is_pending());
        }
        let (stp, reset, _) = abort_writes();
        assert!(stp && reset);
        assert!(sdhi.take_aborted());
    }

    #[test]
    fn a_read_that_fails_in_its_data_phase_stops_the_dma() {
        // On an SD error the SD_BUF DMA request stays asserted (HW manual ch. 38): the DMAC must
        // be stopped before the read returns, or it keeps writing into the caller's buffer.
        let _serial = serial();
        let sdhi = sdhi1();
        let mut storage = [0u8; 512];
        test::reset();
        {
            let read = sdhi.read_blocks_dma(17, crate::dma_buf::UncachedMut::new(&mut storage).unwrap());
            let mut read = core::pin::pin!(read);
            assert!(poll_once(read.as_mut()).is_pending());
            interrupt_saw(INFO1_RESP);
            assert!(poll_once(read.as_mut()).is_pending());
            STATE[1].info2.fetch_or(INFO2_ERR3, Ordering::Release); // a data error
            assert!(matches!(poll_once(read.as_mut()), Poll::Ready(Err(_))));
        }
        let (_, _, dma_halted) = abort_writes();
        assert!(dma_halted, "the DMA channel was stopped");
        assert!(!sdhi.take_aborted(), "an error is not an abort");
    }

    #[test]
    fn a_completed_read_never_aborts() {
        let _serial = serial();
        let sdhi = sdhi1();
        let mut storage = [0u8; 512];
        test::reset();
        {
            let read = sdhi.read_blocks_dma(17, crate::dma_buf::UncachedMut::new(&mut storage).unwrap());
            let mut read = core::pin::pin!(read);
            assert!(poll_once(read.as_mut()).is_pending());
            interrupt_saw(INFO1_RESP);
            assert!(poll_once(read.as_mut()).is_pending());
            interrupt_saw(INFO1_DATA_TRNS);
            // The DMAC's last burst landed: CHSTAT.TC.
            test::poke32(crate::pac::DMAC.ch(14).chstat().addr(), crate::dmac::CHSTAT_TC);
            assert!(poll_once(read.as_mut()).is_ready(), "completes");
        }
        let (stp, reset, _) = abort_writes();
        assert!(!stp && !reset, "no abort after a completed transfer");
        assert!(!sdhi.take_aborted());
    }

    #[test]
    fn a_buffer_that_is_not_whole_blocks_is_refused() {
        let _serial = serial();
        let sdhi = sdhi1();
        test::reset();
        let mut odd = [0u8; 513];
        let mut empty = [0u8; 0];
        assert_eq!(
            embassy_futures::block_on(sdhi.read_blocks_sw(&mut odd)),
            Err(SdhiError::BufferLength)
        );
        assert_eq!(
            embassy_futures::block_on(sdhi.write_blocks_sw(&empty)),
            Err(SdhiError::BufferLength)
        );
        assert_eq!(
            embassy_futures::block_on(
                sdhi.read_blocks_dma(17, crate::dma_buf::UncachedMut::new(&mut odd).unwrap())
            ),
            Err(SdhiError::BufferLength)
        );
        assert_eq!(
            embassy_futures::block_on(
                sdhi.write_blocks_dma(24, crate::dma_buf::Uncached::new(&odd).unwrap())
            ),
            Err(SdhiError::BufferLength)
        );
        let _ = &mut empty;
        assert!(test::writes().is_empty(), "no command went out");
    }

    #[test]
    fn a_controller_comes_up_clocked_reset_and_bound_with_sdio_masked() {
        let _serial = serial();
        test::reset();
        test::poke8(0xFCFE_0444, 0xFF);
        let _sdhi = sdhi1();
        // SD port 1's eight pins (P7_0–P7_7) on alternative 3: PFCE7 set, PFC7/PFCAE7 clear.
        assert_eq!(test::peek16(0xFCFE_361C) & 0xFF, 0xFF, "PFCE7");
        assert_eq!(test::peek16(0xFCFE_351C) & 0xFF, 0, "PFC7");
        assert_eq!(test::peek16(0xFCFE_3A1C) & 0xFF, 0, "PFCAE7");
        // SDHI1's clocks (STBCR12 bits 1:0) run; SDHI0's stay stopped.
        assert_eq!(test::peek8(0xFCFE_0444), 0xFF & !STBCR12_SDHI1_MASK);
        assert_eq!(test::peek16(0xE804_E828), 0x00BD);
        assert_eq!(test::peek16(0xE804_E826), 512);
        assert!(crate::gic::handler(305).is_some(), "status bound");
        assert!(crate::gic::handler(306).is_some(), "operation bound");
        // SDIO (307) is neither bound nor enabled: GICD_ISENABLER9 (0xE820_1124) bit 307-288=19.
        assert!(crate::gic::handler(307).is_none());
        assert_eq!(test::peek32(0xE820_1124) & (1 << 19), 0);
    }

    #[test]
    fn card_detect_reads_info1_of_its_own_port() {
        let _serial = serial();
        test::reset();
        let sdhi = sdhi1();
        test::poke16(0xE804_E81C, INFO1_CD_LEVEL);
        assert!(sdhi.card_inserted());
        test::poke16(0xE804_E81C, 0);
        assert!(!sdhi.card_inserted());
    }
    #[test]
    fn a_reset_returns_the_controller_to_the_identification_clock() {
        let _serial = serial();
        test::reset();
        let sdhi = sdhi1();
        // The previous card ran at the High-Speed clock with an odd block size.
        sdhi.set_clock_div(CLK_DIV_2);
        test::poke16(0xE804_E826, 64);
        test::poke16(0xE804_E828, 0);
        sdhi.reset();
        assert_eq!(
            test::peek16(0xE804_E824),
            CLK_DIV_512 | CLK_ENABLE,
            "re-identification runs at P1/512"
        );
        assert_eq!(test::peek16(0xE804_E826), 512);
        assert_eq!(test::peek16(0xE804_E828), 0x00BD);
        assert_eq!(test::peek16(0xE804_E8E0), SOFT_RST_DEASSERT);
    }
}
