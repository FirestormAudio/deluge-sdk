//! Sample Rate Conversion Unit (SCUX) driver for the RZ/A1L.
//!
//! Hardware reference: RZ/A1L Group User's Manual: Hardware,
//! **Chapter 37 — SCUX** (R01UH0437EJ0700 Rev.7.00, Sep 2024).
//! Primary sections used:
//! - §37.3   Register Descriptions (§37.3.1 – §37.3.73)
//! - §37.4.1 Initial Setting Procedure (Fig. 37.6 – 37.8)
//! - §37.4.2 Transfer Start / Stop Procedure (Fig. 37.9 – 37.11)
//! - §37.4.5 Data Transfer Routes
//! - §37.4.6 Input/Output Timing Signals
//! - §37.4.7 2SRC Block (async/sync mode, INTIFS formula)
//! - §37.4.8 DVU Block (volume, ramp, zero-cross mute)
//! - §37.4.9 MIX Block
//!
//! The SCUX contains five functional blocks that can be combined into various
//! audio processing pipelines:
//!
//! ```text
//!                                                              ┌──► FFU0-3 ──► CPU/DMA
//! CPU/DMA ──► FFD0-3 ──► IPC0-3 ──► 2SRC0-3 ──► OPC0-3 ──────┤
//!                                                              └──► DVU0-3 ──► MIX ──► SSIF
//! ```
//!
//! ## Block summary
//! | Block | Instances | Purpose |
//! |-------|-----------|---------|
//! | FFD   | 4         | FIFO download — DMA from SRAM into SCUX input path |
//! | IPC   | 4         | Input path control — routing and format selection |
//! | 2SRC  | 2 units × 2 ch = 4 paths | Asynchronous / synchronous sample-rate conversion |
//! | DVU   | 4         | Digital volume unit — per-channel gain, ramp, zero-cross mute |
//! | MIX   | 1         | Mixer — combines up to 4 DVU outputs |
//! | OPC   | 4         | Output path control — routing to FFU or SSIF |
//! | FFU   | 4         | FIFO upload — DMA from SCUX output path to SRAM |
//! | CIM   | 1         | Common interface — DMA routing, SSIF clocking, reset |
//!
//! ## Base addresses
//! | Block | Base       |
//! |-------|------------|
//! | SCUX  | 0xE820_8000 |
//! | IPC   | 0xE820_8000 (stride 0x100, ×4) |
//! | OPC   | 0xE820_8400 (stride 0x100, ×4) |
//! | FFD   | 0xE820_8800 (stride 0x100, ×4) |
//! | FFU   | 0xE820_8C00 (stride 0x100, ×4) |
//! | 2SRC  | 0xE820_9000 (2 units, each 2×SRC at stride 0x34) |
//! | DVU   | 0xE820_9200 (stride 0x100, ×4) |
//! | MIX   | 0xE820_9600 |
//! | CIM   | 0xE820_9700 |
//!
//! ## DMA resource selectors (DMARS)
//! | Channel | DMARS value | Description |
//! |---------|-------------|-------------|
//! | FFD0_0  | 0x0101      | SCUTXI0 — CPU → SCUX input FIFO 0 |
//! | FFD0_1  | 0x0105      | SCUTXI1 — CPU → SCUX input FIFO 1 |
//! | FFD0_2  | 0x0109      | SCUTXI2 — CPU → SCUX input FIFO 2 |
//! | FFD0_3  | 0x010D      | SCUTXI3 — CPU → SCUX input FIFO 3 |
//! | FFU0_0  | 0x0102      | SCURXI0 — SCUX output FIFO 0 → CPU |
//! | FFU0_1  | 0x0106      | SCURXI1 — SCUX output FIFO 1 → CPU |
//! | FFU0_2  | 0x010A      | SCURXI2 — SCUX output FIFO 2 → CPU |
//! | FFU0_3  | 0x010E      | SCURXI3 — SCUX output FIFO 3 → CPU |
//!
//! ## DMA channel allocation (Deluge)
//! Channels are passed in by the board crate ([`init_ffd_dma`] / [`init_ffu_dma`]).
//!
//! | DMA ch | Direction        | SCUX path    | Max ch |
//! |--------|-----------------|--------------|--------|
//! | 0      | SRAM → FFD0_0   | DMATD0_CIM   | 8      |
//! | 1      | FFU0_0 → SRAM   | DMATU0_CIM   | 8      |
//! | 2      | SRAM → FFD0_2   | DMATD2_CIM   | 2      |
//! | 3      | SRAM → FFD0_1   | DMATD1_CIM   | 8      |
//! | 4      | SRAM → FFD0_3   | DMATD3_CIM   | 2      |
//! | 5      | FFU0_1 → SRAM   | DMATU1_CIM   | 8      |
//! | 8      | FFU0_2 → SRAM   | DMATU2_CIM   | 2      |
//! | 9      | FFU0_3 → SRAM   | DMATU3_CIM   | 2      |

use core::marker::PhantomData;

use crate::Peri;
use crate::dmac::Dma;
use crate::pac::SCUX;

// ── Uncached mirror ───────────────────────────────────────────────────────────

/// Add to any internal-SRAM cached VA to get its uncached alias.
use crate::UNCACHED_MIRROR_OFFSET;

mod regs;

/// CPU → SCUX DVU → SSI0 output path (hardware volume, ramps, zero-cross mute).
pub mod dvu_path;
/// CPU → SCUX async SRC → CPU path, for bridging two clock domains.
pub mod src_path;
/// Engine ↔ USB-rate async SRC path (2SRC1/0).
pub mod usb_tx_path;
use regs::*;
// Re-export the public SCUX register/API constants as `scux::NAME`.
pub use regs::{
    DMARS_SCURXI0, DMARS_SCURXI1, DMARS_SCURXI2, DMARS_SCURXI3, DMARS_SCUTXI0, DMARS_SCUTXI1,
    DMARS_SCUTXI2, DMARS_SCUTXI3, FDTSEL_DIVEN, FDTSEL_SCKSEL_SSIF0_WS, INTIFS_44100_TO_44100,
    INTIFS_44100_TO_48000, INTIFS_48000_TO_44100, INTIFS_48000_TO_48000, INTIFS_88200_TO_44100,
    INTIFS_96000_TO_44100, INTIFS_96000_TO_48000, SSICTRL_SSI0TX, intifs,
};

// ── Configuration types ───────────────────────────────────────────────────────

/// Bit depth of audio samples.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BitDepth {
    /// 24-bit samples (OTBL = 0b00000).
    B24,
    /// 16-bit samples (OTBL = 0b01000).
    B16,
}

impl BitDepth {
    /// OTBL field value (written to bits [20:16] of SADIR/VADIR).
    /// BSP: SADIR_2SRC0_OTBL_SET_24BIT = (0x0U << 16), OTBL_SET_16BIT = (0x8U << 16)
    fn otbl(self) -> u32 {
        match self {
            BitDepth::B24 => 0x0, // = SADIR_2SRC0_OTBL_SET_24BIT value
            BitDepth::B16 => 0x8, // = SADIR_2SRC0_OTBL_SET_16BIT value
        }
    }
}

/// Audio channel/bit-depth descriptor written into FDAIR / FUAIR / SADIR / VADIR / MADIR.
///
/// Encodes as: `channels | (otbl << 16)` per the BSP convention.
/// CHNUM is the actual channel count (1=mono, 2=stereo), not count−1.
/// OTBL field occupies bits `[20:16]` in SADIR/VADIR (FDAIR/FUAIR/MADIR have no OTBL field).
/// For 24-bit audio, OTBL = 0 so the shift position is moot, but is kept correct.
#[derive(Copy, Clone, Debug)]
pub struct AudioInfo {
    /// Number of audio channels (1–8).
    pub channels: u8,
    /// Sample bit depth.
    pub depth: BitDepth,
}

impl AudioInfo {
    /// Stereo (2-channel) 24-bit audio — the most common configuration.
    pub const STEREO_24: AudioInfo = AudioInfo {
        channels: 2,
        depth: BitDepth::B24,
    };

    fn to_reg(self) -> u32 {
        (self.depth.otbl() << 16) | (self.channels as u32)
    }
}

/// 2SRC operating mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SrcMode {
    /// Asynchronous — SCUX generates an internal reference; input and output
    /// clocks are independent.  Use for CPU→SRAM pipelines.
    Async,
    /// Synchronous — SCUX uses an external (SSI) clock reference.  The
    /// interpolation ratio is computed from INTIFS.
    Sync,
}

/// Configuration for one 2SRC path.
#[derive(Copy, Clone, Debug)]
pub struct SrcConfig {
    /// Async or sync mode.
    pub mode: SrcMode,
    /// Audio input format fed into the SRC.
    pub audio: AudioInfo,
    /// If true, bypass the rate converter entirely — audio passes through
    /// unchanged with no clock-domain dependency.  When true, `mode`,
    /// `intifs`, `mnfsr`, and `buf_size` are ignored.
    pub bypass: bool,
    /// Interpolation ratio (INTIFS).  Use the [`intifs`] function or the
    /// `INTIFS_*` constants.  For sync passthrough use [`INTIFS_44100_TO_44100`].
    pub intifs: u32,
    /// Minimum frequency register (MNFSR).  Set to `intifs * 2 >> 1` for
    /// safety margin; 0 disables the minimum frequency check.
    pub mnfsr: u32,
    /// Buffer size select (BFSSR bits `[1:0]`).  0 = 256 samples (recommended
    /// for most use cases).
    pub buf_size: u32,
}

/// Volume ramp configuration for the DVU hardware fade engine.
#[derive(Copy, Clone, Debug)]
pub struct RampConfig {
    /// Ramp speed in VRPDR units (hardware step interval register value).
    /// See TRM §37.3.37.  A value of 0x0F corresponds to ~1 ms per step.
    pub vrpdr: u32,
    /// Ramp step size in VRDBR units (dB-per-step register value).
    /// 0 = minimum step (≈ 0.0078 dB), 0xFF = maximum step.
    pub vrdbr: u32,
}

/// Configuration for one DVU instance.
#[derive(Copy, Clone, Debug)]
pub struct DvuConfig {
    /// Audio format passing through this DVU instance.
    pub audio: AudioInfo,
    /// If true, all DVU processing is bypassed (audio passes unchanged).
    /// When bypassed, `volumes` and `ramp` are ignored.
    pub bypass: bool,
    /// Per-audio-channel volume in hardware units.
    ///
    /// 0 = mute, 0x0010_0000 = 0 dB (unity = 1.0 in 4.20 fixed-point), values above unity boost up to
    /// approximately +18 dB.  Applies only when `bypass == false`.
    pub volumes: [u32; 8],
    /// Hardware volume ramp configuration.  `None` = instant volume change.
    pub ramp: Option<RampConfig>,
    /// Enable zero-cross mute suppression (eliminates clicks on abrupt mutes).
    pub zero_cross_mute: bool,
}

impl DvuConfig {
    /// Bypass mode — audio passes through unchanged.
    pub const BYPASS: DvuConfig = DvuConfig {
        audio: AudioInfo::STEREO_24,
        bypass: true,
        volumes: [0x0010_0000; 8],
        ramp: None,
        zero_cross_mute: false,
    };
}

/// IPC input path selection (written to IPSLR_IPC0_n, TRM §37.3.2).
///
/// Encodes the `IPC_PASS_SEL` field (bits 2:0):
/// - `000` = no operation
/// - `001` = SSI (external) → IPC → SRC (async)
/// - `011` = FFD → IPC → SRC (async) — CPU supplies audio via DMA ← usual
/// - `100` = FFD → IPC → SRC (sync) — clock from SSI
/// - `101–111` = no operation
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum IpcSel {
    /// Input path disabled.
    None = 0b000,
    /// SSI → 2SRC (async clock domain).
    SsiToSrcAsync = 0b001,
    /// FFD → 2SRC (async, CPU supplies audio via DMA).
    FfdToSrcAsync = 0b011,
    /// FFD → 2SRC (sync, SCUX uses SSI clock reference).
    FfdToSrcSync = 0b100,
}

/// OPC output path selection (written to OPSLR_OPC0_n, TRM §37.3.4).
///
/// Encodes the `OPC_PASS_SEL` field (bits 2:0):
/// - `000` = no operation
/// - `001` = SRC (async) → OPC → DVU (or SSIF direct with DVU/MIX inline)
/// - `011` = SRC (async) → OPC → FFU (CPU receives converted audio via DMA)
/// - `100` = SRC (sync) → OPC → FFU
/// - `101–111` = no operation
///
/// The Renesas BSP uses 0b001 for any SSIF output route (SRC direct,
/// SRC+DVU, or SRC+DVU+MIX). DVU/MIX routing is configured separately.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum OpcSel {
    /// Output path disabled.
    None = 0b000,
    /// Async path → SSIF direct (works with or without DVU/MIX in chain).
    ToSsi = 0b001,
    /// Async path → FFU (CPU receives converted audio via DMA).
    ToFfu = 0b011,
}

/// MIX configuration.
#[derive(Copy, Clone, Debug)]
pub struct MixConfig {
    /// Audio format for MIX output.
    pub audio: AudioInfo,
    /// If true, the mixer is bypassed (first connected source passes through).
    pub bypass: bool,
}

// ── Link descriptors ──────────────────────────────────────────────────────────

/// Cache-line-aligned 8-word DMA link descriptor (same layout as SSI).
#[repr(C, align(32))]
struct LinkDesc([u32; 8]);

/// Each FIFO's DMA channel, once its path's `init` has handed it over: `start` starts the
/// channels a bitmask names, and the paths read their positions.
type FifoChannels = embassy_sync::blocking_mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    core::cell::RefCell<[Option<Dma<'static>>; 4]>,
>;
static FFD_DMA: FifoChannels =
    embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new([None, None, None, None]));
static FFU_DMA: FifoChannels =
    embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new([None, None, None, None]));

/// Run `f` on FIFO `n`'s channel in `table`.
///
/// # Panics
/// If no path has handed that FIFO a channel: reading a DMA position before `init` would read a
/// channel that isn't there.
fn with_fifo<R>(table: &FifoChannels, n: usize, f: impl FnOnce(&mut Dma<'static>) -> R) -> R {
    table.lock(|t| {
        f(t.borrow_mut()[n]
            .as_mut()
            .expect("scux path used before init"))
    })
}

/// 2SRC unit `unit`'s path `pair`: the PAC numbers the four paths `2 * unit + pair`.
fn src(unit: u8, pair: u8) -> crate::pac::scux::Src {
    SCUX.src(2 * unit as usize + pair as usize)
}

/// FFD `n`'s DMA read position (CRSA).
pub(crate) fn ffd_current_src(n: usize) -> u32 {
    with_fifo(&FFD_DMA, n, |d| d.current_src())
}

/// FFU `n`'s DMA write position (CRDA).
pub(crate) fn ffu_current_dst(n: usize) -> u32 {
    with_fifo(&FFU_DMA, n, |d| d.current_dst())
}

static mut FFD0_DESC: LinkDesc = LinkDesc([
    DESC_HEADER, // LDEN + NXA + valid (see DESC_HEADER)
    0,           // src  → FFD0 buffer (set at init)
    0,           // dst = DMATD0_CIM data register (set at init)
    0,           // byte count (set at init)
    0,           // CHCFG (patched at init from dma_ch param)
    0,           // CHITVL
    0,           // CHEXT
    0,           // NXLA → &FFD0_DESC (set at init)
]);

static mut FFD1_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,
    0,
    0, // dst = DMATDn_CIM data register (set at init)
    0,
    0, // CHCFG (patched at init)
    0,
    0,
    0,
]);

static mut FFU0_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,
    0, // src = DMATU0_CIM data register (set at init)
    0, // dst → FFU0 buffer (set at init)
    0, // byte count (set at init)
    0, // CHCFG (patched at init)
    0,
    0,
    0,
]);

static mut FFU1_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,
    0, // src = DMATUn_CIM data register (set at init)
    0,
    0,
    0, // CHCFG (patched at init)
    0,
    0,
    0,
]);

static mut FFD2_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,
    0,
    0, // dst = DMATDn_CIM data register (set at init)
    0,
    0, // CHCFG (patched at init)
    0,
    0,
    0,
]);

static mut FFD3_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,
    0,
    0, // dst = DMATDn_CIM data register (set at init)
    0,
    0, // CHCFG (patched at init)
    0,
    0,
    0,
]);

static mut FFU2_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,
    0, // src = DMATUn_CIM data register (set at init)
    0,
    0,
    0, // CHCFG (patched at init)
    0,
    0,
    0,
]);

static mut FFU3_DESC: LinkDesc = LinkDesc([
    DESC_HEADER,
    0, // src = DMATUn_CIM data register (set at init)
    0,
    0,
    0, // CHCFG (patched at init)
    0,
    0,
    0,
]);

// ── Software reset ─────────────────────────────────────────────────────────────

/// Assert and then deassert the SCUX block-level software reset.
///
/// Writes 0 then 1 to SWRSR_CIM.SWRST (TRM §37.3.63).  After this call all
/// INIT bits in FFD/FFU/SRC/DVU/MIX/IPC/OPC are 1 (held in initialisation
/// state), which is the required starting condition before configuring any
/// sub-block (TRM §37.4.1, Fig. 37.6 step <1>).
///
/// # Safety
/// Writes to CIM memory-mapped registers.  Call from a single-threaded init
/// context after [`init`](crate::init) has started the SCUX module clock.
pub unsafe fn reset() {
    SCUX.swrsr_cim().write_value(SWRSR_RESET);
    let _ = SCUX.swrsr_cim().read(); // mandatory dummy read
    SCUX.swrsr_cim().write_value(SWRSR_RUN);
    let _ = SCUX.swrsr_cim().read();
}

// ── Sub-block configuration ────────────────────────────────────────────────────

/// Configure one IPC (input path control) channel.
///
/// Writes `sel` to IPSLR_IPC0_n (TRM §37.3.2).  Must be called while the
/// IPC channel INIT bit is 1, i.e. before [`start`] or [`start_path`]
/// clears it (TRM §37.4.1, Fig. 37.6 step <2>).
///
/// # Safety
/// Writes to IPC memory-mapped registers.
pub unsafe fn configure_ipc(ch: u8, sel: IpcSel) {
    SCUX.ipc(ch as usize).ipslr().write_value(sel as u32);
}

/// Configure one OPC (output path control) channel.
///
/// Writes `sel` to OPSLR_OPC0_n (TRM §37.3.4).  Must be called while the
/// OPC channel INIT bit is 1 (TRM §37.4.1, Fig. 37.6 step <2>).
///
/// # Safety
/// Writes to OPC memory-mapped registers.
pub unsafe fn configure_opc(ch: u8, sel: OpcSel) {
    SCUX.opc(ch as usize).opslr().write_value(sel as u32);
}

/// Configure one FFD (FIFO download / CPU→SCUX) channel.
///
/// Writes to FDAIR, DRQSR, DEVMR, and FFDPR (TRM §§37.3.6–37.3.8, 37.3.10).
/// Must be called while FFDIR_FFD0_n.INIT = 1 (TRM §37.4.1, Fig. 37.7 step <5>).
///
/// `dma_size` is the DRQSR DMA request threshold in stereo samples (typically
/// 8 for 16-byte bursts or 16 for 32-byte bursts).
///
/// # Safety
/// Writes to FFD memory-mapped registers.
pub unsafe fn configure_ffd(ch: u8, audio: AudioInfo, dma_size: u8) {
    SCUX.ffd(ch as usize).fdair().write_value(audio.to_reg());
    SCUX.ffd(ch as usize).drqsr().write_value(dma_size as u32);
    SCUX.ffd(ch as usize).devmr().write_value(0); // all event flags cleared = DMA mode
    // FFDPR.PASS_ASYNC: enable async data path CIM (DMATD) → FFD → IPC.
    // Without this the audio data written to DMATD0_CIM goes nowhere.
    SCUX.ffd(ch as usize).ffdpr().write_value(FFDPR_PASS_ASYNC);
}

/// Configure one FFU (FIFO upload / SCUX→CPU) channel.
///
/// Writes to FUAIR, URQSR, UEVMR, and FFUPR (TRM §§37.3.13–37.3.16).
/// Must be called while FFUIR_FFU0_n.INIT = 1 (TRM §37.4.1, Fig. 37.7 step <6>).
///
/// # Safety
/// Writes to FFU memory-mapped registers.
pub unsafe fn configure_ffu(ch: u8, audio: AudioInfo, dma_size: u8) {
    SCUX.ffu(ch as usize).fuair().write_value(audio.to_reg());
    SCUX.ffu(ch as usize).urqsr().write_value(dma_size as u32);
    SCUX.ffu(ch as usize).uevmr().write_value(0); // all event flags cleared = DMA mode
    // FFUPR.PASS_ASYNC: enable async data path OPC → FFU → CIM (DMATU).
    SCUX.ffu(ch as usize).ffupr().write_value(FFUPR_PASS_ASYNC);
}

/// Configure one 2SRC path (identified by `unit` 0–1 and `pair` 0–1).
///
/// Writes to SADIRp, SRCBRp, IFSCRp, IFSVRp, SRCCRp, MNFSRp, BFSSRp
/// (TRM §§37.3.18–37.3.25).  Must be called while SRCIRp.INIT = 1
/// (TRM §37.4.1, Fig. 37.7 step <7>).
///
/// SRCCR must-be-1 bits (16, 8, 4) are always written per TRM §37.3.23 Note 1;
/// they are cleared by SWRSR and must be restored before the SRC will function
/// even in bypass mode.
///
/// # Safety
/// Writes to 2SRC memory-mapped registers.
pub unsafe fn configure_src(unit: u8, pair: u8, cfg: SrcConfig) {
    // Audio information — channel count and bit depth.
    src(unit, pair).sadir().write_value(cfg.audio.to_reg());

    // BSP (SCUX_InitHw) always writes SRCCR = BASE_VALUE (0x00010110) regardless
    // of bypass mode.  After scux::reset() (SWRSR) these must-be-1 bits are 0;
    // skip them and the SRC may malfunction even in bypass mode.
    src(unit, pair).srccr().write_value(SRCCR_MBZ1);

    if cfg.bypass {
        src(unit, pair).srcbr().write_value(SRCBR_BYPASS);
        return;
    }

    // Bypass disabled (0 = active conversion)
    src(unit, pair).srcbr().write_value(0);

    // IFSCR_INTIFSEN=1: use INTIFS ratio (async mode); 0 = sync mode.
    let ifscr_val: u32 = match cfg.mode {
        SrcMode::Async => IFSCR_INTIFSEN,
        SrcMode::Sync => 0,
    };
    src(unit, pair).ifscr().write_value(ifscr_val);

    // Interpolation ratio
    src(unit, pair).ifsvr().write_value(cfg.intifs);

    // Common control: must-be-1 bits | mode bit
    let srccr_val = SRCCR_MBZ1
        | match cfg.mode {
            SrcMode::Sync => SRCCR_SYNC,
            SrcMode::Async => 0,
        };
    src(unit, pair).srccr().write_value(srccr_val);

    // Minimum frequency
    src(unit, pair).mnfsr().write_value(cfg.mnfsr);

    // Buffer size
    src(unit, pair).bfssr().write_value(cfg.buf_size & 0b11);
}

/// Update the INTIFS ratio of a running 2SRC path.
///
/// Writes the new ratio to IFSVRp_2SRC0_m (TRM §37.3.22), then reloads it by
/// briefly asserting SRCIRR_2SRC0_m.INIT (TRM §37.3.30).  The SRC hardware
/// picks up the new rate within one output sample period without glitching.
///
/// Valid while the SRC is active (TRM §37.4.7).  The path must be running.
///
/// # Safety
/// Writes to 2SRC live registers; the path must be running.
pub unsafe fn src_update_intifs(unit: u8, pair: u8, new_intifs: u32) {
    src(unit, pair).ifsvr().write_value(new_intifs);
    // Reload the input-rate by toggling SRCIRR.INIT (unit-level register)
    SCUX.srcirr(unit as usize).write_value(1); // assert
    SCUX.srcirr(unit as usize).write_value(0); // release
}

/// Configure one DVU instance (phase 1 — safe while DVUIR.INIT = 1).
///
/// Writes VADIR_DVU0_n (TRM §37.3.32) and DVUBR_DVU0_n (TRM §37.3.33),
/// which are the only DVU registers that accept writes while DVUIR.INIT = 1
/// (TRM §37.4.8).  All other DVU registers (VOLxR, VRCTR, DVUCR, DVUER, …)
/// are continuously forced to their reset values while INIT = 1 and must be
/// written after INIT is cleared — see [`apply_dvu_after_init`].
///
/// Must be called during initial configuration before [`start`]
/// (TRM §37.4.1, Fig. 37.7 step <8>).
///
/// # Safety
/// Writes to DVU memory-mapped registers.
pub unsafe fn configure_dvu(ch: u8, cfg: DvuConfig) {
    // VADIR and DVUBR can be written while INIT=1 (BSP SetupDvu does so).
    SCUX.dvu(ch as usize)
        .vadir()
        .write_value(cfg.audio.to_reg());
    if cfg.bypass {
        SCUX.dvu(ch as usize).dvubr().write_value(DVUBR_BYPASS);
    } else {
        SCUX.dvu(ch as usize).dvubr().write_value(0); // active (not bypassed)
    }
    // All other DVU registers (VOLxR, VRCTR, DVUCR, DVUER, …) must be
    // written AFTER DVUIR.INIT is cleared — see apply_dvu_after_init().
}

/// Apply DVU volume / ramp settings after DVUIR.INIT has been cleared.
///
/// **Must be called immediately after [`start`]** clears `DVUIR.INIT` for
/// this DVU channel.  Writes ZCMCR (§37.3.35), VOL0R–VOL7R (§§37.3.40–47),
/// VRCTR (§37.3.36), VRPDR (§37.3.37), VRDBR (§37.3.38), VRWTR (§37.3.39),
/// DVUCR (§37.3.34), and finally DVUER.DVUEN (§37.3.48) — mirroring the BSP
/// `SCUX_SetupDvuVolume` sequence (TRM §37.4.8).
///
/// Has no effect (returns immediately) when `cfg.bypass == true`.
///
/// # Safety
/// Writes to DVU memory-mapped registers.  DVUIR.INIT must be 0.
pub unsafe fn apply_dvu_after_init(ch: u8, cfg: DvuConfig) {
    if cfg.bypass {
        return; // bypass: no volume processing, nothing to set
    }

    // ZCMCR: zero-cross mute per active channel.
    let zcmcr_val = if cfg.zero_cross_mute {
        (1u32 << cfg.audio.channels as u32) - 1
    } else {
        0
    };
    SCUX.dvu(ch as usize)
        .zcmcr()
        .write_value(zcmcr_val * ZCMCR_ZCEN);

    // VOLxR: per-channel volume.
    // Format: 4.20 fixed-point. Unity (0 dB) = 0x0010_0000 (= 1.0).
    // WARNING: bit 23 is the sign bit — 0x00FF_FFFF has sign=1 → MUTE!
    for i in 0..cfg.audio.channels {
        SCUX.dvu(ch as usize)
            .volr(i as usize)
            .write_value(cfg.volumes[i as usize]);
    }

    // DVUCR: VRMD (bit 4) + VVMD (bit 8).
    // VRMD mandatory when DVU enabled; VVMD enables digital volume path.
    // BSP: DVUCR |= DVUCR_DVU0_VVMD_SET (SetDigiVolRegister)
    //      DVUCR |= DVUCR_DVU0_VRMD_SET (SetupDvuVolume)
    SCUX.dvu(ch as usize)
        .dvucr()
        .write_value(DVUCR_VRMD | DVUCR_VVMD);

    // VRCTR: enable volume ramp per active channel (BSP "dummy ramp").
    // BSP ALWAYS sets VREN for each active channel even for instant changes.
    let vren_mask: u32 = (1u32 << cfg.audio.channels as u32) - 1;
    let (vrpdr, vrdbr) = match cfg.ramp {
        Some(r) => (r.vrpdr, r.vrdbr),
        None => (0, 0), // instant: zero-rate dummy ramp
    };
    SCUX.dvu(ch as usize)
        .vrctr()
        .write_value(vren_mask * VRCTR_VREN);
    SCUX.dvu(ch as usize).vrpdr().write_value(vrpdr);
    SCUX.dvu(ch as usize).vrdbr().write_value(vrdbr);
    SCUX.dvu(ch as usize).vrwtr().write_value(0);

    // DVUER.DVUEN — enable DVU output.  Must be last (BSP SetupDvuVolume).
    SCUX.dvu(ch as usize).dvuer().write_value(DVUER_DVUEN);
}

/// Set the volume for a single audio sub-channel within a DVU instance.
///
/// | Value        | Gain |
/// |--------------|------|
/// | 0x0000_0000  | Mute (−∞ dB) |
/// | 0x0010_0000  | Unity (0 dB) — 4.20 fixed-point value 1.0 |
/// | 0x7F_FFFF    | Maximum (+18 dB) |
///
/// The DVU must not be in bypass mode for this to take effect.
///
/// # Safety
/// Writes to a DVU VOLxR register.
pub unsafe fn set_volume(dvu_ch: u8, audio_ch: u8, vol: u32) {
    SCUX.dvu(dvu_ch as usize)
        .volr(audio_ch as usize)
        .write_value(vol);
}

/// Set the same volume level on all audio sub-channels of a DVU instance.
///
/// # Safety
/// Writes to DVU VOL0R–VOL7R registers.
pub unsafe fn set_volume_all(dvu_ch: u8, n_channels: u8, vol: u32) {
    for i in 0..n_channels {
        SCUX.dvu(dvu_ch as usize).volr(i as usize).write_value(vol);
    }
}

/// Arm the hardware volume ramp engine on a DVU instance.
///
/// Writes `vrpdr` (ramp period), `vrdbr` (dB step), and enables the ramp.
/// The ramp runs until the target volume set by [`set_volume`] is reached.
///
/// # Safety
/// Writes to DVU ramp registers.
pub unsafe fn start_ramp(dvu_ch: u8, ramp: RampConfig) {
    SCUX.dvu(dvu_ch as usize).vrpdr().write_value(ramp.vrpdr);
    SCUX.dvu(dvu_ch as usize).vrdbr().write_value(ramp.vrdbr);
    SCUX.dvu(dvu_ch as usize).vrctr().write_value(VRCTR_VREN); // caller sets channel mask if needed
}

/// Configure the MIX block.
///
/// Writes MADIR_MIX0_0 (TRM §37.3.53), MIXBR_MIX0_0 (TRM §37.3.54), and in
/// non-bypass mode also MDBAR–MDBDR and MDBER (TRM §§37.3.57–37.3.61).
/// Must be called while MIXIR_MIX0_0.INIT = 1 (TRM §37.4.1, Fig. 37.8 step <9>).
///
/// In bypass mode only the first input source passes through; the other
/// sources are ignored.
///
/// # Safety
/// Writes to MIX memory-mapped registers.
pub unsafe fn configure_mix(cfg: MixConfig) {
    SCUX.mix().madir().write_value(cfg.audio.to_reg());

    if cfg.bypass {
        SCUX.mix().mixbr().write_value(MIXBR_BYPASS);
    } else {
        SCUX.mix().mixbr().write_value(0);
        // Unity gain on all four mix inputs, all enabled
        // MDBxR: 10-bit dB gain, 0x000 = 0 dB (unity), 0x3FF = mute.
        SCUX.mix().mdbar().write_value(0x000);
        SCUX.mix().mdbbr().write_value(0x000);
        SCUX.mix().mdbcr().write_value(0x000);
        SCUX.mix().mdbdr().write_value(0x000);
        SCUX.mix().mdber().write_value(MDBER_MIXDBEN); // BSP: MDBER_MIX0_MIXDBEN_SET
    }
}

/// Set the per-source gain on one MIX input (MDB0R–MDB3R).
///
/// The MIX block has four input slots, each with a 10-bit dB gain coefficient:
/// - `0x000` = 0 dB (unity)
/// - `0x001`–`0x3FE` = attenuation (larger = more attenuation, ~−0.5 dB/step)
/// - `0x3FF` = mute
///
/// `source` is 0–3 corresponding to DVU0_0–DVU0_3.
/// This allows hardware crossfades between multiple FFD paths meeting at the MIX.
///
/// # Safety
/// Writes to MIX MDBxR registers.  MIX must not be in bypass mode.
pub unsafe fn set_mix_source_gain(source: u8, gain: u32) {
    let mdb = match source {
        0 => SCUX.mix().mdbar(),
        1 => SCUX.mix().mdbbr(),
        2 => SCUX.mix().mdbcr(),
        _ => SCUX.mix().mdbdr(),
    };
    mdb.write_value(gain & 0x3FF);
}

/// Configure the FDTSELn_CIM input-timing-signal selector for one SRC unit.
///
/// Writes FDTSELn_CIM (TRM §37.3.68).  Called when async mode is used with
/// an FFD input path (TRM §37.4.6).  Pass the encoded value for the `SCKSEL`
/// field; set `DIVEN = 1` (bit 8, [`FDTSEL_DIVEN`]) to enable the signal output.
/// For example `FDTSEL_DIVEN | FDTSEL_SCKSEL_SSIF0_WS` selects SSIF0 WS with
/// output enabled.
///
/// # Safety
/// Writes to CIM FDTSELn register.
pub unsafe fn set_fdtsel(src_unit: u8, val: u32) {
    SCUX.fdtsel(src_unit.min(3) as usize).write_value(val);
}

/// FDTSEL/FUTSEL SCKSEL: the USB_X1 clock (TRM §37.3.68).
pub const FDTSEL_SCKSEL_USB_X1: u32 = 0b0011;
/// USB_X1: the USB module's clock, 48 MHz (UCKSEL=0, as the Deluge runs it, requires it).
pub const USB_X1_HZ: u32 = 48_000_000;

/// The FDTSEL value (without DIVEN) that paces an asynchronous SRC's input at `input_hz`:
/// USB_X1 divided down when it divides evenly by an even ratio SCKDIV holds (48 kHz is
/// USB_X1 / 1000), else the SSIF0 word clock (the codec's rate, which is right only when the
/// input runs at it). Pacing a 48 kHz input by SSIF0's 44.1 kHz word clock consumed it at
/// 44.1 kHz: the conversion ran at 1:1.
pub fn input_timing(input_hz: u32) -> u32 {
    // SCKDIV (bits 26:16) is the division ratio itself; bit 16 must be 0, so the ratio is
    // even, and at most 0x7FE.
    match USB_X1_HZ.checked_div(input_hz) {
        Some(ratio)
            if USB_X1_HZ % input_hz == 0 && ratio % 2 == 0 && (2..=0x7FE).contains(&ratio) =>
        {
            (ratio << 16) | FDTSEL_SCKSEL_USB_X1
        }
        _ => FDTSEL_SCKSEL_SSIF0_WS,
    }
}

/// Configure the FUTSELn_CIM output-timing-signal selector for one SRC unit.
///
/// Writes FUTSELn_CIM (TRM §37.3.69).  Required when async mode is used and
/// output goes through MIX or FFU (TRM §37.4.6).  Same encoding as
/// [`set_fdtsel`].
///
/// # Safety
/// Writes to CIM FUTSELn register.
pub unsafe fn set_futsel(src_unit: u8, val: u32) {
    SCUX.futsel(src_unit.min(3) as usize).write_value(val);
}

/// Write the CIM SSIRSEL register to route an SSI channel into a 2SRC unit.
///
/// Writes SSIRSEL_CIM (TRM §37.3.67).  `ssirsel` is written verbatim; consult
/// TRM §37.3.67 Table 37-3 for encoding.  Required for SSIF→SRC paths.
///
/// # Safety
/// Writes to CIM SSIRSEL register.
pub unsafe fn set_ssirsel(ssirsel: u32) {
    SCUX.ssirsel_cim().write_value(ssirsel);
}

/// Write the CIM SSIPMD register (SSI pin-mode / port direction).
///
/// Writes SSIPMD_CIM (TRM §37.3.70).  Controls whether each SSI pin is used
/// as transmit, receive, or master input.  Required during initial setup
/// (TRM §37.4.1, Fig. 37.6 step <4>).
///
/// # Safety
/// Writes to CIM register.
pub unsafe fn set_ssipmd(val: u32) {
    SCUX.ssipmd_cim().write_value(val);
}

/// Write the CIM SSICTRL register (SSI clock/direct-drive control).
///
/// Writes SSICTRL_CIM (TRM §37.3.71).  Setting [`SSICTRL_SSI0TX`] (bit 14)
/// routes the SCUX output directly to the SSIF0 transmitter, bypassing the
/// SSI DMA path.  Set this as the final step before enabling SSI TX
/// (TRM §37.4.2, Fig. 37.9 "Set transmission to start").
///
/// # Safety
/// Writes to CIM register.
pub unsafe fn set_ssictrl(val: u32) {
    SCUX.ssictrl_cim().write_value(val);
}

/// Set up DMA for one FFD (download) channel and connect it to CIM.
///
/// `src_buf` must point to a buffer accessible by the DMAC (physical SRAM).
/// `buf_bytes` is the total buffer size in bytes; the DMA will loop over this
/// entire region continuously.
///
/// # Safety
/// - Must be called after [`reset`] and sub-block configuration, before
///   [`start`].
/// - `src_buf` must remain valid and DMA-accessible for the lifetime of the
///   transfer.
/// - Writes to static link-descriptor memory and DMAC registers.
pub unsafe fn init_ffd_dma(
    ffd_ch: u8,
    mut dma: Dma<'static>,
    src_buf: *const u32,
    buf_bytes: usize,
) {
    unsafe {
        // An FFD ring raises DMAINT every pass (DEM clear); nothing awaits it.
        dma.mask_interrupt();
        let (desc_ptr, dmars) = match ffd_ch {
            0 => (core::ptr::addr_of_mut!(FFD0_DESC), DMARS_SCUTXI0),
            1 => (core::ptr::addr_of_mut!(FFD1_DESC), DMARS_SCUTXI1),
            2 => (core::ptr::addr_of_mut!(FFD2_DESC), DMARS_SCUTXI2),
            3 => (core::ptr::addr_of_mut!(FFD3_DESC), DMARS_SCUTXI3),
            _ => panic!("init_ffd_dma: invalid channel"),
        };

        let desc_u = (desc_ptr as usize + UNCACHED_MIRROR_OFFSET) as *mut u32;

        // Patch: source buffer address (SRAM audio data, increments each transfer)
        desc_u.add(1).write_volatile(src_buf as u32);
        // Patch: destination = DMATD_CIM data register (fixed address, SCUX ingests here)
        desc_u
            .add(2)
            .write_volatile(SCUX.dmatd(ffd_ch as usize).addr() as u32);
        // Patch: transfer byte count
        desc_u.add(3).write_volatile(buf_bytes as u32);
        // Patch: CHCFG (computed from dma_ch at runtime)
        desc_u.add(4).write_volatile(ffd_chcfg(dma.number()));
        // Patch: self-referential NXLA (uncached alias)
        desc_u.add(7).write_volatile(desc_u as u32);

        dma.init_with_link_descriptor(desc_u as *const u32, dmars);
        FFD_DMA.lock(|t| t.borrow_mut()[ffd_ch as usize] = Some(dma));
    }
}

/// Set up DMA for one FFU (upload) channel.
///
/// # Safety
/// Same requirements as [`init_ffd_dma`].
pub unsafe fn init_ffu_dma(ffu_ch: u8, mut dma: Dma<'static>, dst_buf: *mut u32, buf_bytes: usize) {
    unsafe {
        dma.mask_interrupt();
        let (desc_ptr, dmars) = match ffu_ch {
            0 => (core::ptr::addr_of_mut!(FFU0_DESC), DMARS_SCURXI0),
            1 => (core::ptr::addr_of_mut!(FFU1_DESC), DMARS_SCURXI1),
            2 => (core::ptr::addr_of_mut!(FFU2_DESC), DMARS_SCURXI2),
            3 => (core::ptr::addr_of_mut!(FFU3_DESC), DMARS_SCURXI3),
            _ => panic!("init_ffu_dma: invalid channel"),
        };

        let desc_u = (desc_ptr as usize + UNCACHED_MIRROR_OFFSET) as *mut u32;

        // Patch: source = DMATU_CIM data register (fixed address, SCUX outputs here)
        desc_u
            .add(1)
            .write_volatile(SCUX.dmatu(ffu_ch as usize).addr() as u32);
        // Patch: destination buffer (SRAM, increments each transfer)
        desc_u.add(2).write_volatile(dst_buf as u32);
        // Patch: byte count
        desc_u.add(3).write_volatile(buf_bytes as u32);
        // Patch: CHCFG (computed from dma_ch at runtime)
        desc_u.add(4).write_volatile(ffu_chcfg(dma.number()));
        // Patch: NXLA
        desc_u.add(7).write_volatile(desc_u as u32);

        dma.init_with_link_descriptor(desc_u as *const u32, dmars);
        FFU_DMA.lock(|t| t.borrow_mut()[ffu_ch as usize] = Some(dma));
    }
}

/// Enable DMA in CIM and clear INIT bits on all configured sub-blocks.
///
/// Call this after all sub-blocks have been configured and their DMA channels
/// set up.  The TRM mandates a specific INIT-clear order for async SRC mode
/// (TRM §37.4.2, Fig. 37.9 "Clear initialization of FFD and start boot" and
/// "Clear initialization of FFU, SRC, DVU, MIX, IPC, and OPC"):
///
/// 1. Enable DMACR_CIM (§37.3.64) for FFD and FFU channels.
/// 2. Start corresponding DMAC channels.
/// 3. FFD: clear FFDIR.INIT, set FFDBR.BOOT (§§37.3.5, 37.3.9).
/// 4. FFU: clear FFUIR.INIT (§37.3.12).
/// 5. 2SRC: clear SRCIRp.INIT (§37.3.18) + SRCIRR.INIT (§37.3.30).
/// 6. DVU: clear DVUIR.INIT (§37.3.31) — then call [`apply_dvu_after_init`].
/// 7. MIX: clear MIXIR.INIT (§37.3.52).
/// 8. IPC: clear IPCIR.INIT (§37.3.1) — second-to-last.
/// 9. OPC: clear OPCIR.INIT (§37.3.3) — **must be last**.
///
/// Also writes SRCRSELn_CIM and MIXRSEL_CIM (§§37.3.72–37.3.73) to identity
/// mapping (0x7654_3210) after soft-reset clears them.
///
/// `ffd_mask`, `ffu_mask`: bitmask of channels to start (bit 0 = ch 0, etc.).
/// `src_mask`: bits 0–3 → unit0/pair0, unit0/pair1, unit1/pair0, unit1/pair1.
/// `dvu_mask`: bitmask of DVU channels to start.
/// `mix_en`: true to start the MIX block.
/// `ipc_mask`, `opc_mask`: bitmasks of IPC/OPC channels to start.
///
/// # Safety
/// All specified sub-blocks must have been configured before calling this.
pub unsafe fn start(
    ffd_mask: u8,
    ffu_mask: u8,
    src_mask: u8,
    dvu_mask: u8,
    mix_en: bool,
    ipc_mask: u8,
    opc_mask: u8,
) {
    unsafe {
        // Enable DMA triggers in CIM.
        // DMACR: bits [3:0] = FFD0-3 TX enable, bits [7:4] = FFU0-3 RX enable
        let dmacr = (ffd_mask as u32) | ((ffu_mask as u32) << 4);
        SCUX.dmacr_cim().write_value(dmacr);

        // Start DMA channels
        for ch in 0..4u8 {
            if ffd_mask & (1 << ch) != 0 {
                with_fifo(&FFD_DMA, ch as usize, |d| d.channel_start());
            }
        }
        for ch in 0..4u8 {
            if ffu_mask & (1 << ch) != 0 {
                with_fifo(&FFU_DMA, ch as usize, |d| d.channel_start());
            }
        }

        // 1. FFD: clear INIT (start), then set BOOT to arm the FIFO.
        for ch in 0..4u8 {
            if ffd_mask & (1 << ch) != 0 {
                SCUX.ffd(ch as usize).ffdir().write_value(INIT_CLR);
                SCUX.ffd(ch as usize).ffdbr().write_value(FFDBR_BOOT);
            }
        }

        // 2. FFU: clear INIT (start).
        for ch in 0..4u8 {
            if ffu_mask & (1 << ch) != 0 {
                SCUX.ffu(ch as usize).ffuir().write_value(INIT_CLR);
            }
        }

        // 3a. Routing registers — must be written after SWRST which may clear them.
        //     Hardware reset value 0x76543210 = pass-through identity mapping.
        SCUX.srcrsel(0).write_value(0x7654_3210);
        SCUX.srcrsel(1).write_value(0x7654_3210);
        SCUX.srcrsel(2).write_value(0x7654_3210);
        SCUX.srcrsel(3).write_value(0x7654_3210);
        SCUX.mixrsel_cim().write_value(0x7654_3210);

        // 3b. 2SRC: clear SRCIR and SRCIRR for each active path.
        for idx in 0..4u8 {
            if src_mask & (1 << idx) != 0 {
                let unit = idx / 2;
                let pair = idx % 2;
                src(unit, pair).srcir().write_value(INIT_CLR);
                SCUX.srcirr(unit as usize).write_value(INIT_CLR);
            }
        }

        // 4. DVU: clear INIT only.
        //    VOLxR, VRCTR, DVUCR, DVUER must be written AFTER DVUIR.INIT=0;
        //    the caller must invoke apply_dvu_after_init() once start() returns.
        for ch in 0..4u8 {
            if dvu_mask & (1 << ch) != 0 {
                SCUX.dvu(ch as usize).dvuir().write_value(INIT_CLR);
                // DVUER and volume regs: written by apply_dvu_after_init().
            }
        }

        // 5. MIX: clear INIT.
        if mix_en {
            SCUX.mix().mixir().write_value(INIT_CLR);
        }

        // 6. IPC and OPC — MUST be last (TRM §37.4 startup order).
        for ch in 0..4u8 {
            if ipc_mask & (1 << ch) != 0 {
                SCUX.ipc(ch as usize).ipcir().write_value(INIT_CLR);
            }
        }
        for ch in 0..4u8 {
            if opc_mask & (1 << ch) != 0 {
                SCUX.opc(ch as usize).opcir().write_value(INIT_CLR);
            }
        }
    }
}

/// Start additional SCUX sub-blocks without disturbing already-running paths.
///
/// Identical to [`start`] except:
/// - DMACR_CIM (§37.3.64) is updated with a read-modify-write (OR) so
///   FFD/FFU channels already enabled by a prior path are not cleared.
/// - Routing registers SRCRSELn_CIM and MIXRSEL_CIM (§§37.3.72–73) are
///   **not** re-written; they retain the identity mapping from [`start`].
///
/// INIT-clear order follows the same TRM §37.4.2 async sequence as [`start`]:
/// FFD → FFU → 2SRC → DVU → MIX → IPC → OPC (last).
///
/// # Safety
/// SCUX must have been started at least once via [`start`] before calling
/// this function.  Writes to SCUX and DMAC registers.
pub unsafe fn start_path(
    ffd_mask: u8,
    ffu_mask: u8,
    src_mask: u8,
    dvu_mask: u8,
    mix_en: bool,
    ipc_mask: u8,
    opc_mask: u8,
) {
    unsafe {
        // Read-modify-write DMACR so running channels are not cleared.
        let existing_dmacr = SCUX.dmacr_cim().read();
        let new_dmacr = existing_dmacr | (ffd_mask as u32) | ((ffu_mask as u32) << 4);
        SCUX.dmacr_cim().write_value(new_dmacr);

        // Start new DMA channels only.
        for ch in 0..4u8 {
            if ffd_mask & (1 << ch) != 0 {
                with_fifo(&FFD_DMA, ch as usize, |d| d.channel_start());
            }
        }
        for ch in 0..4u8 {
            if ffu_mask & (1 << ch) != 0 {
                with_fifo(&FFU_DMA, ch as usize, |d| d.channel_start());
            }
        }

        // FFD: clear INIT + set BOOT.
        clear_inits(ffd_mask, ffu_mask, src_mask, dvu_mask, mix_en, ipc_mask, opc_mask);
    }
}

/// Take the masked units out of INIT in the TRM §37.4.2 start order: FFD (then BOOT), FFU, 2SRC,
/// DVU, MIX, then IPC and OPC last.
///
/// # Safety
/// Writes SCUX registers; the units must be configured.
unsafe fn clear_inits(
    ffd_mask: u8,
    ffu_mask: u8,
    src_mask: u8,
    dvu_mask: u8,
    mix_en: bool,
    ipc_mask: u8,
    opc_mask: u8,
) {
    for ch in 0..4u8 {
        if ffd_mask & (1 << ch) != 0 {
            SCUX.ffd(ch as usize).ffdir().write_value(INIT_CLR);
            SCUX.ffd(ch as usize).ffdbr().write_value(FFDBR_BOOT);
        }
    }

    // FFU: clear INIT.
    for ch in 0..4u8 {
        if ffu_mask & (1 << ch) != 0 {
            SCUX.ffu(ch as usize).ffuir().write_value(INIT_CLR);
        }
    }

    // 2SRC: clear SRCIR + SRCIRR.
    for idx in 0..4u8 {
        if src_mask & (1 << idx) != 0 {
            let unit = idx / 2;
            let pair = idx % 2;
            src(unit, pair).srcir().write_value(INIT_CLR);
            SCUX.srcirr(unit as usize).write_value(INIT_CLR);
        }
    }

    // DVU: clear INIT (caller must call apply_dvu_after_init after this).
    for ch in 0..4u8 {
        if dvu_mask & (1 << ch) != 0 {
            SCUX.dvu(ch as usize).dvuir().write_value(INIT_CLR);
        }
    }

    // MIX: clear INIT.
    if mix_en {
        SCUX.mix().mixir().write_value(INIT_CLR);
    }

    // IPC and OPC — MUST be last (TRM §37.4 startup order).
    for ch in 0..4u8 {
        if ipc_mask & (1 << ch) != 0 {
            SCUX.ipc(ch as usize).ipcir().write_value(INIT_CLR);
        }
    }
    for ch in 0..4u8 {
        if opc_mask & (1 << ch) != 0 {
            SCUX.opc(ch as usize).opcir().write_value(INIT_CLR);
        }
    }
}

/// Stop the specified SCUX sub-blocks without disturbing other running paths.
///
/// Re-asserts INIT on each specified block in the reverse of the TRM startup
/// order (TRM §37.4.2, Fig. 37.11 "Transfer stop setting"):
/// OPC → IPC → MIX → DVU → 2SRC → FFU → FFD.
///
/// Clears the corresponding FFD/FFU bits in DMACR_CIM (§37.3.64) via
/// read-modify-write so other running channels are not disturbed.
///
/// After this call the stopped sub-blocks can be reconfigured and restarted
/// with [`start_path`].
///
/// # Safety
/// Writes to SCUX and DMAC registers.
pub unsafe fn stop_path(
    ffd_mask: u8,
    ffu_mask: u8,
    src_mask: u8,
    dvu_mask: u8,
    mix_en: bool,
    ipc_mask: u8,
    opc_mask: u8,
) {
    // IPC / OPC first (TRM stop order: reverse of start).
    for ch in 0..4u8 {
        if opc_mask & (1 << ch) != 0 {
            SCUX.opc(ch as usize).opcir().write_value(INIT_SET);
        }
    }
    for ch in 0..4u8 {
        if ipc_mask & (1 << ch) != 0 {
            SCUX.ipc(ch as usize).ipcir().write_value(INIT_SET);
        }
    }

    // MIX.
    if mix_en {
        SCUX.mix().mixir().write_value(INIT_SET);
    }

    // DVU.
    for ch in 0..4u8 {
        if dvu_mask & (1 << ch) != 0 {
            SCUX.dvu(ch as usize).dvuir().write_value(INIT_SET);
        }
    }

    // 2SRC.
    for idx in 0..4u8 {
        if src_mask & (1 << idx) != 0 {
            let unit = idx / 2;
            let pair = idx % 2;
            src(unit, pair).srcir().write_value(INIT_SET);
        }
    }

    // FFU, FFD.
    for ch in 0..4u8 {
        if ffu_mask & (1 << ch) != 0 {
            SCUX.ffu(ch as usize).ffuir().write_value(INIT_SET);
        }
    }
    for ch in 0..4u8 {
        if ffd_mask & (1 << ch) != 0 {
            SCUX.ffd(ch as usize).ffdir().write_value(INIT_SET);
        }
    }

    // Read-modify-write DMACR: clear only the stopped channels.
    let existing = SCUX.dmacr_cim().read();
    let clear_mask = (ffd_mask as u32) | ((ffu_mask as u32) << 4);
    SCUX.dmacr_cim().write_value(existing & !clear_mask);
}

/// Stop all active SCUX paths and re-assert INIT on all sub-blocks.
///
/// Asserts INIT in the reverse order from [`start`]: OPC/IPC first, MIX,
/// DVU, 2SRC, FFU, FFD last.  Disables DMA in CIM.
///
/// After this call, sub-blocks can be reconfigured and [`start`] called again.
///
/// # Safety
/// Writes to SCUX and DMAC registers.
pub unsafe fn stop() {
    // OPC / IPC first (TRM §37.4 stop order: reverse of start)
    for ch in 0..4u8 {
        SCUX.opc(ch as usize).opcir().write_value(INIT_SET);
        SCUX.ipc(ch as usize).ipcir().write_value(INIT_SET);
    }

    // MIX
    SCUX.mix().mixir().write_value(INIT_SET);

    // DVU
    for ch in 0..4u8 {
        SCUX.dvu(ch as usize).dvuir().write_value(INIT_SET);
    }

    // 2SRC
    for unit in 0..2u8 {
        for pair in 0..2u8 {
            src(unit, pair).srcir().write_value(INIT_SET);
        }
    }

    // FFU, FFD
    for ch in 0..4u8 {
        SCUX.ffu(ch as usize).ffuir().write_value(1);
        SCUX.ffd(ch as usize).ffdir().write_value(1);
    }

    // Disable DMA triggers in CIM
    SCUX.dmacr_cim().write_value(0);
}

/// Returns the DMA channel number assigned to the given FFD path.
///
/// `u8::MAX` until [`init_ffd_dma`] has been called for `ffd_ch`.
pub fn ffd_dma_ch(ffd_ch: u8) -> u8 {
    FFD_DMA.lock(|t| {
        t.borrow()[ffd_ch as usize]
            .as_ref()
            .map_or(u8::MAX, |d| d.number())
    })
}

// ── Owned paths ───────────────────────────────────────────────────────────────

use crate::dmac::ring::{Dir, RingDma};
use crate::ring_buffer::{ReadableRing, RingBuf, WritableRing};

mod sealed_ch {
    pub trait Sealed {}
}

/// A SCUX channel: FFDn → IPCn → SRCn (2SRC unit n/2, pair n%2) → DVUn → OPCn → FFUn or an SSI.
pub trait ChannelId: sealed_ch::Sealed + 'static {
    /// The channel number, 0–3.
    const N: u8;
}

macro_rules! channel_ids {
    ($($ch:ident = $n:literal),*) => {$(
        #[doc = concat!("SCUX channel ", stringify!($n), ".")]
        pub struct $ch;
        impl sealed_ch::Sealed for $ch {}
        impl ChannelId for $ch {
            const N: u8 = $n;
        }
    )*};
}
channel_ids!(Ch0 = 0, Ch1 = 1, Ch2 = 2, Ch3 = 3);

/// One SCUX channel, to build a [`Path`] from.
pub struct Channel<'d, C: ChannelId> {
    _ch: PhantomData<(&'d mut (), C)>,
}

/// The four channels of [`Scux::new`].
#[allow(missing_docs)]
pub struct Channels<'d> {
    pub ch0: Channel<'d, Ch0>,
    pub ch1: Channel<'d, Ch1>,
    pub ch2: Channel<'d, Ch2>,
    pub ch3: Channel<'d, Ch3>,
}

/// The SCUX, split into its channels.
pub struct Scux;

impl Scux {
    /// Reset the SCUX, set its routing registers to their identity mapping, and hand out its
    /// four channels.
    pub fn new<'d>(_scux: Peri<'d, crate::peripherals::SCUX>) -> Channels<'d> {
        // SAFETY: the SCUX is owned; nothing runs on it yet.
        unsafe { reset() };
        for n in 0..4 {
            SCUX.srcrsel(n).write_value(0x7654_3210);
        }
        SCUX.mixrsel_cim().write_value(0x7654_3210);
        Channels {
            ch0: Channel { _ch: PhantomData },
            ch1: Channel { _ch: PhantomData },
            ch2: Channel { _ch: PhantomData },
            ch3: Channel { _ch: PhantomData },
        }
    }
}

/// A channel's sample-rate converter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Src {
    /// Pass samples through at their rate.
    Bypass,
    /// Convert `input_hz` to `output_hz`, asynchronously.
    Async {
        /// The input rate.
        input_hz: u32,
        /// The output rate.
        output_hz: u32,
    },
}

/// A channel's digital volume unit.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Dvu {
    /// No volume processing.
    Bypass,
    /// Every audio channel at `volume` (Q20: 0x0010_0000 is unity).
    Volume {
        /// The volume.
        volume: u32,
        /// A ramp to it, or none.
        ramp: Option<RampConfig>,
        /// Mute at zero crossings.
        zero_cross_mute: bool,
    },
}

/// The sub-units one path runs, one bit each per mask.
#[derive(Debug, Clone, Copy)]
struct Units {
    ffd: u8,
    ffu: u8,
    src: u8,
    dvu: u8,
    ipc: u8,
    opc: u8,
}

/// Enable `u`'s FIFO DMA requests (DMACR, read-modify-written so other paths keep theirs) and
/// take its units out of INIT. Its rings are already running.
///
/// # Safety
/// Writes SCUX registers; the units must be configured.
unsafe fn enable_units(u: Units) {
    critical_section::with(|_| {
        let dmacr = SCUX.dmacr_cim().read();
        SCUX.dmacr_cim().write_value(dmacr | u.ffd as u32 | ((u.ffu as u32) << 4));
    });
    // SAFETY: the caller's contract.
    unsafe { clear_inits(u.ffd, u.ffu, u.src, u.dvu, false, u.ipc, u.opc) };
}

impl<'d, C: ChannelId> Channel<'d, C> {
    /// FFD ring → SRC → DVU → FFU ring: convert what's written to `ffd` and read it back from
    /// `ffu`. Frames are `format.channels` words.
    pub fn into_ffu_path(
        self,
        format: AudioInfo,
        ffd: RingBuf<'d>,
        src: Src,
        dvu: Dvu,
        ffu: RingBuf<'d>,
    ) -> Path<'d> {
        let n = C::N;
        let frame = format.channels as usize;
        let tx = start_ffd(n, ffd, frame);
        let ffu_ch = ffu.dma.number();
        let rx = RingDma::start(
            ffu.dma,
            ffu.buf,
            ffu.blocks,
            frame,
            ffu_chcfg(ffu_ch) & !crate::dmac::CHCFG_DEM,
            Dir::PeriphToMem { src: SCUX.dmatu(n as usize).addr() as u32 },
            DMARS_SCURXI0 + 4 * n as u32,
        );
        // SAFETY: channel n's units are this path's.
        let control = unsafe {
            configure_ipc(n, IpcSel::FfdToSrcAsync);
            configure_opc(n, OpcSel::ToFfu);
            configure_ffd(n, format, 8);
            configure_ffu(n, format, 8);
            let control = configure_src_dvu(n, format, src, dvu, true);
            enable_units(control.units);
            control.after_start();
            control
        };
        Path { control, tx: WritableRing::new(tx), rx: Some(ReadableRing::new(rx)) }
    }
}

/// SCUX channel `Self` can drive SSIF `S` directly: SSIRSEL's SOSEL field for `S`, at
/// `SOSEL_SHIFT`, set to `SOSEL` (HW manual §37.3.67). The pairs:
/// - SSIF0 ← channel 0 (00) or 1 (01);
/// - SSIF1 ← channel 2 (00);
/// - SSIF3 ← channel 1 (00) or 0 (01).
///
/// Channel 3 drives only SSIF2, which [`crate::ssi`] doesn't support yet.
pub trait RoutesTo<S: crate::ssi::Instance>: ChannelId {
    /// The SOSEL field's bit position in SSIRSEL.
    const SOSEL_SHIFT: u32;
    /// The SOSEL value that picks this channel.
    const SOSEL: u32;
}

macro_rules! routes {
    ($($ch:ident => $ssi:ident, $shift:literal, $sel:literal;)*) => {$(
        impl RoutesTo<crate::peripherals::$ssi> for $ch {
            const SOSEL_SHIFT: u32 = $shift;
            const SOSEL: u32 = $sel;
        }
    )*};
}

routes! {
    Ch0 => SSIF0, 0, 0b00;
    Ch1 => SSIF0, 0, 0b01;
    Ch2 => SSIF1, 4, 0b00;
    Ch1 => SSIF3, 12, 0b00;
    Ch0 => SSIF3, 12, 0b01;
}

/// SSICTRL's direct-transmission bit for SSIF `ssi` (HW manual §37.3.71).
const fn ssictrl_tx_bit(ssi: u8) -> u32 {
    match ssi {
        0 => 1 << 14,
        1 => 1 << 13,
        2 => 1 << 12,
        _ => 1 << 30,
    }
}

/// Read-modify-write a SCUX register shared between channels, with interrupts masked.
fn shared_update(reg: rza1_pac::common::Reg<u32, rza1_pac::common::RW>, f: impl FnOnce(u32) -> u32) {
    critical_section::with(|_| reg.write_value(f(reg.read())));
}

impl<'d, C: ChannelId> Channel<'d, C> {
    /// FFD ring → SRC → DVU → SSI `S`: play what's written to `ffd` straight out of the SSI,
    /// whose transmit this path enables once its route is up. Only routes the hardware has
    /// compile ([`RoutesTo`]).
    pub fn into_ssi_path<S: crate::ssi::Instance>(
        self,
        format: AudioInfo,
        ffd: RingBuf<'d>,
        src: Src,
        dvu: Dvu,
        out: crate::ssi::ScuxOutput<'d, S>,
    ) -> Path<'d>
    where
        C: RoutesTo<S>,
    {
        let n = C::N;
        let tx = start_ffd(n, ffd, format.channels as usize);
        let ws = FDTSEL_DIVEN | (0b1000 + S::INDEX as u32);
        // SAFETY: channel n's units are this path's; the shared registers are read-modify-written.
        let mut control = unsafe {
            configure_ipc(n, IpcSel::FfdToSrcAsync);
            configure_opc(n, OpcSel::ToSsi);
            configure_ffd(n, format, 8);
            let control = configure_src_dvu(n, format, src, dvu, false);
            shared_update(SCUX.ssirsel_cim(), |v| {
                (v & !(0b11 << C::SOSEL_SHIFT)) | (C::SOSEL << C::SOSEL_SHIFT)
            });
            match src {
                // DIVEN must be 0 while SCKSEL changes.
                Src::Async { input_hz, .. } => {
                    let input = input_timing(input_hz);
                    SCUX.fdtsel(n as usize).write_value(input);
                    SCUX.fdtsel(n as usize).write_value(input | FDTSEL_DIVEN);
                }
                Src::Bypass => SCUX.fdtsel(n as usize).write_value(ws),
            }
            SCUX.futsel(n as usize).write_value(ws);
            shared_update(SCUX.ssictrl_cim(), |v| v | ssictrl_tx_bit(S::INDEX));
            enable_units(control.units);
            control.after_start();
            control
        };
        out.enable();
        // The control stops the SSI's transmit when it drops.
        control.ssi = Some(S::INDEX);
        core::mem::forget(out);
        Path { control, tx: WritableRing::new(tx), rx: None }
    }
}

/// Start channel `n`'s FFD ring (feeding DMATDn).
fn start_ffd<'d>(n: u8, ffd: RingBuf<'d>, frame: usize) -> RingDma<'d> {
    let ch = ffd.dma.number();
    RingDma::start(
        ffd.dma,
        ffd.buf,
        ffd.blocks,
        frame,
        ffd_chcfg(ch),
        Dir::MemToPeriph { dst: SCUX.dmatd(n as usize).addr() as u32 },
        DMARS_SCUTXI0 + 4 * n as u32,
    )
}

/// Configure channel `n`'s SRC and DVU (and the MIX's bypass, as the legacy paths did), and
/// return the path's control with its unit masks. `ffu` adds the FFU to them.
///
/// # Safety
/// Writes channel `n`'s SCUX registers.
unsafe fn configure_src_dvu(n: u8, format: AudioInfo, src: Src, dvu: Dvu, ffu: bool) -> Control {
    let bit = 1u8 << n;
    let (bypass, intifs, output_hz) = match src {
        Src::Bypass => (true, INTIFS_44100_TO_44100, None),
        Src::Async { input_hz, output_hz } => (false, intifs(input_hz, output_hz), Some(output_hz)),
    };
    // SAFETY: the caller's contract.
    unsafe {
        configure_src(
            n / 2,
            n % 2,
            SrcConfig { mode: SrcMode::Async, audio: format, bypass, intifs, mnfsr: 0, buf_size: 0 },
        );
    }
    let dvu_cfg = match dvu {
        Dvu::Bypass => None,
        Dvu::Volume { volume, ramp, zero_cross_mute } => {
            let cfg = DvuConfig { audio: format, bypass: false, volumes: [volume; 8], ramp, zero_cross_mute };
            // SAFETY: the caller's contract.
            unsafe { configure_dvu(n, cfg) };
            Some(cfg)
        }
    };
    // Both legacy paths set the MIX to bypass, though no path runs through it.
    // SAFETY: the caller's contract.
    unsafe { configure_mix(MixConfig { audio: format, bypass: true }) };
    Control {
        n,
        channels: format.channels,
        output_hz,
        dvu: dvu_cfg,
        ssi: None,
        units: Units {
            ffd: bit,
            ffu: if ffu { bit } else { 0 },
            src: bit,
            dvu: if dvu_cfg.is_some() { bit } else { 0 },
            ipc: bit,
            opc: bit,
        },
    }
}

/// A running SCUX path: [`split`](Self::split) it into its control and its rings.
pub struct Path<'d> {
    control: Control,
    tx: WritableRing<'d>,
    rx: Option<ReadableRing<'d>>,
}

impl<'d> Path<'d> {
    /// The path's control, its FFD ring, and its FFU ring if it has one. Dropping the control
    /// stops the path; dropping a ring halts its DMA.
    pub fn split(self) -> (Control, WritableRing<'d>, Option<ReadableRing<'d>>) {
        (self.control, self.tx, self.rx)
    }
}

/// A running path's volume and rate. Dropping it stops the path's units (and its SSI's
/// transmit, on a path into an SSI).
pub struct Control {
    n: u8,
    channels: u8,
    output_hz: Option<u32>,
    dvu: Option<DvuConfig>,
    ssi: Option<u8>,
    units: Units,
}

impl Control {
    /// Apply the DVU settings that must follow DVUIR.INIT = 0.
    ///
    /// # Safety
    /// The path's units are running.
    unsafe fn after_start(&self) {
        if let Some(cfg) = self.dvu {
            // SAFETY: the caller's contract.
            unsafe { apply_dvu_after_init(self.n, cfg) };
        }
    }

    /// Set audio channel `audio_ch`'s volume (Q20). No effect on a path without a DVU.
    pub fn set_volume(&mut self, audio_ch: u8, vol: u32) {
        // SAFETY: this path's DVU register.
        unsafe { set_volume(self.n, audio_ch, vol) }
    }

    /// Set every audio channel's volume (Q20).
    pub fn set_volume_all(&mut self, vol: u32) {
        // SAFETY: this path's DVU registers.
        unsafe { set_volume_all(self.n, self.channels, vol) }
    }

    /// Ramp every audio channel to `vol` with `ramp`.
    pub fn fade_to(&mut self, vol: u32, ramp: RampConfig) {
        // SAFETY: this path's DVU registers.
        unsafe {
            set_volume_all(self.n, self.channels, vol);
            start_ramp(self.n, ramp);
        }
    }

    /// Change the converter's input rate.
    ///
    /// # Panics
    /// On a path whose SRC is bypassed.
    pub fn set_input_rate(&mut self, input_hz: u32) {
        let output_hz = self.output_hz.expect("scux: set_input_rate on a bypassed SRC");
        // SAFETY: this path's SRC register.
        unsafe { src_update_intifs(self.n / 2, self.n % 2, intifs(input_hz, output_hz)) }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if let Some(ssi) = self.ssi {
            crate::ssi::stop_tx(ssi);
        }
        let u = self.units;
        // SAFETY: this path's units.
        unsafe { stop_path(u.ffd, u.ffu, u.src, u.dvu, false, u.ipc, u.opc) };
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn an_input_rate_usb_x1_divides_is_paced_by_usb_x1() {
        assert_eq!(input_timing(48_000), (1000 << 16) | FDTSEL_SCKSEL_USB_X1);
        assert_eq!(input_timing(96_000), (500 << 16) | FDTSEL_SCKSEL_USB_X1);
        assert_eq!(input_timing(32_000), (1500 << 16) | FDTSEL_SCKSEL_USB_X1);
    }

    #[test]
    fn other_input_rates_are_paced_by_the_ssif0_word_clock() {
        assert_eq!(
            input_timing(44_100),
            FDTSEL_SCKSEL_SSIF0_WS,
            "not a divisor of 48 MHz"
        );
        assert_eq!(
            input_timing(16_000),
            FDTSEL_SCKSEL_SSIF0_WS,
            "ratio 3000 past SCKDIV"
        );
        assert_eq!(
            input_timing(16_000_000),
            FDTSEL_SCKSEL_SSIF0_WS,
            "odd ratio 3"
        );
    }

    #[test]
    fn audio_info_encodes_channels_and_otbl() {
        // STEREO_24: 2 channels, OTBL=0 for 24-bit → reg = 2.
        assert_eq!(AudioInfo::STEREO_24.channels, 2);
        assert_eq!(AudioInfo::STEREO_24.to_reg(), 2);
        // CHNUM is the actual count (not count−1).
        let mono = AudioInfo {
            channels: 1,
            depth: BitDepth::B24,
        };
        assert_eq!(mono.to_reg() & 0xFFFF, 1);
    }

    // The path tests that run a legacy `init` hold the same lock and empty the FIFO tables
    // before releasing it, so every path is in its never-initialised state here. Reading the pointer then must panic
    // rather than read DMA registers at channel `u8::MAX`.
    #[test]
    #[should_panic(expected = "scux path used before init")]
    fn dvu_tx_current_ptr_before_init_panics() {
        let _serial = super::path_tests::serial();
        let _ = super::dvu_path::tx_current_ptr();
    }

    #[test]
    #[should_panic(expected = "scux path used before init")]
    fn src_rx_current_ptr_before_init_panics() {
        let _serial = super::path_tests::serial();
        let _ = super::src_path::rx_current_ptr();
    }

    #[test]
    #[should_panic(expected = "scux path used before init")]
    fn src_tx_current_ptr_before_init_panics() {
        let _serial = super::path_tests::serial();
        let _ = super::src_path::tx_current_ptr();
    }

    #[test]
    #[should_panic(expected = "scux path used before init")]
    fn usb_tx_rx_current_ptr_before_init_panics() {
        let _serial = super::path_tests::serial();
        let _ = super::usb_tx_path::rx_current_ptr();
    }

    #[test]
    #[should_panic(expected = "scux path used before init")]
    fn usb_tx_tx_current_ptr_before_init_panics() {
        let _serial = super::path_tests::serial();
        let _ = super::usb_tx_path::tx_current_ptr();
    }

    #[test]
    fn the_pac_names_the_same_scux_registers() {
        for n in 0..4usize {
            assert_eq!(SCUX.ipc(n).ipcir().addr(), 0xE820_8000 + n * 0x100);
            assert_eq!(SCUX.opc(n).opcir().addr(), 0xE820_8400 + n * 0x100);
            assert_eq!(SCUX.ffd(n).ffdir().addr(), 0xE820_8800 + n * 0x100);
            assert_eq!(SCUX.ffu(n).ffuir().addr(), 0xE820_8C00 + n * 0x100);
            assert_eq!(SCUX.dvu(n).dvuir().addr(), 0xE820_9200 + n * 0x100);
            let (unit, pair) = (n / 2, n % 2);
            assert_eq!(
                SCUX.src(n).srcir().addr(),
                0xE820_9000 + unit * 0x100 + pair * 0x34
            );
        }
        for k in 0..8usize {
            assert_eq!(SCUX.dvu(1).volr(k).addr(), 0xE820_9300 + 0x24 + k * 4);
        }
        for unit in 0..2usize {
            assert_eq!(SCUX.srcirr(unit).addr(), 0xE820_9000 + unit * 0x100 + 0x68);
        }
        assert_eq!(SCUX.mix().mixir().addr(), 0xE820_9600);
        assert_eq!(SCUX.mix().mdbar().addr(), 0xE820_9614);
        assert_eq!(SCUX.mix().mdbdr().addr(), 0xE820_9620);
        assert_eq!(SCUX.mix().mdber().addr(), 0xE820_9624);
        assert_eq!(SCUX.swrsr_cim().addr(), 0xE820_9700);
        assert_eq!(SCUX.dmacr_cim().addr(), 0xE820_9704);
        for n in 0..4usize {
            assert_eq!(SCUX.dmatd(n).addr(), 0xE820_9708 + n * 4);
            assert_eq!(SCUX.dmatu(n).addr(), 0xE820_9718 + n * 4);
            assert_eq!(SCUX.fdtsel(n).addr(), 0xE820_973C + n * 4);
            assert_eq!(SCUX.futsel(n).addr(), 0xE820_974C + n * 4);
            assert_eq!(SCUX.srcrsel(n).addr(), 0xE820_9764 + n * 4);
        }
        assert_eq!(SCUX.ssirsel_cim().addr(), 0xE820_9738);
        assert_eq!(SCUX.ssipmd_cim().addr(), 0xE820_975C);
        assert_eq!(SCUX.ssictrl_cim().addr(), 0xE820_9760);
        assert_eq!(SCUX.mixrsel_cim().addr(), 0xE820_9774);
    }

    #[test]
    fn a_reset_writes_swrsr_with_dummy_reads() {
        crate::mmio::test::reset();
        unsafe { reset() };
        let log: Vec<_> = crate::mmio::test::log()
            .iter()
            .map(|a| (a.write, a.addr, a.width))
            .collect();
        let swrsr = 0xE820_9700;
        assert_eq!(
            &log[..4],
            [
                (true, swrsr, 4),
                (false, swrsr, 4),
                (true, swrsr, 4),
                (false, swrsr, 4)
            ]
        );
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod path_tests {
    use super::*;
    use crate::dma_buf::UncachedMut;
    use crate::dmac::{BlockDma, BlockInterruptHandler, Dma};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH2, DMA_CH3, DMA_CH5, DMA_CH8};
    use crate::ring_buffer::RingBuf;
    use std::collections::BTreeMap;

    crate::bind_interrupts!(struct Irqs {
        DMAINT2 => BlockInterruptHandler<DMA_CH2>;
        DMAINT3 => BlockInterruptHandler<DMA_CH3>;
        DMAINT5 => BlockInterruptHandler<DMA_CH5>;
        DMAINT8 => BlockInterruptHandler<DMA_CH8>;
    });

    /// The SCUX and its legacy path tables are process-wide.
    pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ring<C: crate::dmac::Channel>(ch: crate::Peri<'static, C>, words: usize) -> RingBuf<'static>
    where
        Irqs: crate::interrupt::Binding<C::Interrupt, BlockInterruptHandler<C>>,
    {
        RingBuf {
            dma: BlockDma::new(ch, Irqs),
            buf: UncachedMut::new(std::vec![0u32; words].leak()).unwrap(),
            blocks: 4,
        }
    }

    /// The final value of every SCUX register written since the log began.
    fn scux_writes() -> BTreeMap<usize, u32> {
        test::log()
            .into_iter()
            .filter(|a| a.write && (0xE820_8000..0xE820_A000).contains(&a.addr))
            .map(|a| (a.addr, a.val))
            .collect()
    }

    /// Empty the legacy paths' FIFO DMA tables, so the "used before init" tests see them empty.
    fn forget_legacy_fifos() {
        for table in [&FFD_DMA, &FFU_DMA] {
            table.lock(|t| t.borrow_mut().iter_mut().for_each(|d| *d = None));
        }
    }

    /// Every register `legacy` wrote ends with the same value under `new`.
    fn assert_covers(legacy: &BTreeMap<usize, u32>, new: &BTreeMap<usize, u32>) {
        for (addr, val) in legacy {
            assert_eq!(new.get(addr), Some(val), "SCUX register {addr:#010x}");
        }
    }

    fn dmars(ch: usize) -> u32 {
        let reg = test::peek32(0xFCFE_1000 + (ch / 2) * 4);
        if ch % 2 == 0 { reg & 0xFFFF } else { reg >> 16 }
    }

    #[test]
    fn a_channel1_ffu_path_writes_what_the_legacy_src_path_writes() {
        let _serial = serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        unsafe {
            src_path::init(
                Dma::new_without_interrupt(DMA_CH3::steal()),
                Dma::new_without_interrupt(DMA_CH5::steal()),
                48_000,
                44_100,
            )
        };
        let legacy = scux_writes();
        forget_legacy_fifos();

        test::reset();
        let chs = Scux::new(unsafe { crate::peripherals::SCUX::steal() });
        let path = chs.ch1.into_ffu_path(
            AudioInfo::STEREO_24,
            ring(unsafe { DMA_CH3::steal() }, 512),
            Src::Async { input_hz: 48_000, output_hz: 44_100 },
            Dvu::Bypass,
            ring(unsafe { DMA_CH5::steal() }, 512),
        );
        assert_covers(&legacy, &scux_writes());
        assert_eq!((dmars(3), dmars(5)), (0x105, 0x106), "FFD1 / FFU1 requests");
        let (_control, tx, rx) = path.split();
        assert_eq!(tx.len(), 512);
        assert!(rx.is_some());
    }

    #[test]
    fn a_channel2_ffu_path_writes_what_the_legacy_usb_tx_path_writes() {
        let _serial = serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        unsafe {
            usb_tx_path::init(
                Dma::new_without_interrupt(DMA_CH2::steal()),
                Dma::new_without_interrupt(DMA_CH8::steal()),
                48_000,
                44_100,
            )
        };
        let legacy = scux_writes();
        forget_legacy_fifos();

        test::reset();
        let chs = Scux::new(unsafe { crate::peripherals::SCUX::steal() });
        let _path = chs.ch2.into_ffu_path(
            AudioInfo::STEREO_24,
            ring(unsafe { DMA_CH2::steal() }, 512),
            Src::Async { input_hz: 48_000, output_hz: 44_100 },
            Dvu::Bypass,
            ring(unsafe { DMA_CH8::steal() }, 512),
        );
        assert_covers(&legacy, &scux_writes());
        assert_eq!((dmars(2), dmars(8)), (0x109, 0x10A), "FFD2 / FFU2 requests");
        assert_eq!(SCUX.dmacr_cim().read() & 0xFF, 0b0100 | (0b0100 << 4), "only channel 2's");
    }

    #[test]
    fn two_paths_keep_each_others_dmacr_bits() {
        let _serial = serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let chs = Scux::new(unsafe { crate::peripherals::SCUX::steal() });
        let src = Src::Async { input_hz: 48_000, output_hz: 44_100 };
        let a = chs.ch1.into_ffu_path(
            AudioInfo::STEREO_24,
            ring(unsafe { DMA_CH3::steal() }, 512),
            src,
            Dvu::Bypass,
            ring(unsafe { DMA_CH5::steal() }, 512),
        );
        let b = chs.ch2.into_ffu_path(
            AudioInfo::STEREO_24,
            ring(unsafe { DMA_CH2::steal() }, 512),
            src,
            Dvu::Bypass,
            ring(unsafe { DMA_CH8::steal() }, 512),
        );
        assert_eq!(SCUX.dmacr_cim().read() & 0xFF, 0b0110 | (0b0110 << 4));
        let (control_a, _, _) = a.split();
        drop(control_a);
        assert_eq!(SCUX.dmacr_cim().read() & 0xFF, 0b0100 | (0b0100 << 4), "dropping one stops only it");
        drop(b);
    }

    #[test]
    fn set_volume_writes_its_dvu_channel() {
        let _serial = serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let chs = Scux::new(unsafe { crate::peripherals::SCUX::steal() });
        let path = chs.ch2.into_ffu_path(
            AudioInfo::STEREO_24,
            ring(unsafe { DMA_CH2::steal() }, 512),
            Src::Bypass,
            Dvu::Volume { volume: 0x0010_0000, ramp: None, zero_cross_mute: false },
            ring(unsafe { DMA_CH8::steal() }, 512),
        );
        let (mut control, _, _) = path.split();
        control.set_volume(1, 0x0008_0000);
        assert_eq!(test::peek32(SCUX.dvu(2).volr(1).addr()), 0x0008_0000);
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod ssi_path_tests {
    use super::*;
    use crate::dma_buf::UncachedMut;
    use crate::dmac::{BlockDma, BlockInterruptHandler};
    use crate::mmio::test;
    use crate::peripherals::{DMA_CH0, DMA_CH7, P6_8, P6_9, P6_10, P6_11, SSIF0};
    use crate::ring_buffer::RingBuf;

    crate::bind_interrupts!(struct Irqs {
        DMAINT0 => BlockInterruptHandler<DMA_CH0>;
        DMAINT7 => BlockInterruptHandler<DMA_CH7>;
    });

    fn ring<C: crate::dmac::Channel>(ch: crate::Peri<'static, C>) -> RingBuf<'static>
    where
        Irqs: crate::interrupt::Binding<C::Interrupt, BlockInterruptHandler<C>>,
    {
        RingBuf {
            dma: BlockDma::new(ch, Irqs),
            buf: UncachedMut::new(std::vec![0u32; 512].leak()).unwrap(),
            blocks: 4,
        }
    }

    fn scux_output() -> (crate::ssi::ScuxOutput<'static, SSIF0>, crate::ssi::SsiRx<'static>) {
        unsafe {
            crate::ssi::Ssi::new_with_scux_tx(
                SSIF0::steal(),
                P6_8::steal(),
                P6_9::steal(),
                P6_10::steal(),
                P6_11::steal(),
                ring(DMA_CH7::steal()),
                crate::ssi::Config::default(),
            )
        }
    }

    const SSICR: usize = 0xE820_B000;
    const SSICR_TEN: u32 = 1 << 1;

    #[test]
    fn the_dvu_path_routes_ch0_to_ssif0() {
        let _serial = super::path_tests::serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let (out, _rx) = scux_output();
        let chs = Scux::new(unsafe { crate::peripherals::SCUX::steal() });
        let path = chs.ch0.into_ssi_path(
            AudioInfo::STEREO_24,
            ring(unsafe { DMA_CH0::steal() }),
            Src::Bypass,
            Dvu::Volume { volume: 0x0010_0000, ramp: None, zero_cross_mute: false },
            out,
        );
        assert_eq!(test::peek32(SCUX.ipc(0).ipslr().addr()), IpcSel::FfdToSrcAsync as u32);
        assert_eq!(test::peek32(SCUX.opc(0).opslr().addr()), OpcSel::ToSsi as u32);
        assert_eq!(test::peek32(SCUX.ssirsel_cim().addr()) & 0b11, 0b00, "SOSEL0: SRC0");
        let tsel = FDTSEL_DIVEN | FDTSEL_SCKSEL_SSIF0_WS;
        assert_eq!(test::peek32(SCUX.fdtsel(0).addr()), tsel);
        assert_eq!(test::peek32(SCUX.futsel(0).addr()), tsel);
        assert_eq!(test::peek32(SCUX.ssictrl_cim().addr()) & SSICTRL_SSI0TX, SSICTRL_SSI0TX);
        assert_eq!(test::peek32(SCUX.dvu(0).volr(0).addr()), 0x0010_0000);
        let log = test::log();
        let opc_start = log
            .iter()
            .rposition(|a| a.write && a.addr == SCUX.opc(0).opcir().addr() && a.val == 0)
            .expect("OPC0 out of INIT");
        let ten = log
            .iter()
            .rposition(|a| a.write && a.addr == SSICR && a.val & SSICR_TEN != 0)
            .expect("TEN set");
        assert!(ten > opc_start, "SSI transmit starts after the route is up");
        let (control, _tx, rx) = path.split();
        assert!(rx.is_none());
        drop(control);
        assert_eq!(test::peek32(SSICR) & SSICR_TEN, 0, "dropping the control stops transmit");
    }

    #[test]
    fn ch0_to_ssif3_selects_sosel3_01() {
        assert_eq!(<Ch0 as RoutesTo<crate::peripherals::SSIF3>>::SOSEL_SHIFT, 12);
        assert_eq!(<Ch0 as RoutesTo<crate::peripherals::SSIF3>>::SOSEL, 0b01);
    }

    #[test]
    fn ch1_to_ssif0_selects_sosel0_01() {
        assert_eq!(<Ch1 as RoutesTo<SSIF0>>::SOSEL_SHIFT, 0);
        assert_eq!(<Ch1 as RoutesTo<SSIF0>>::SOSEL, 0b01);
    }

    #[test]
    fn the_route_table_matches_ssirsel() {
        use crate::peripherals::{SSIF1, SSIF3};
        assert_eq!((<Ch0 as RoutesTo<SSIF0>>::SOSEL_SHIFT, <Ch0 as RoutesTo<SSIF0>>::SOSEL), (0, 0));
        assert_eq!((<Ch2 as RoutesTo<SSIF1>>::SOSEL_SHIFT, <Ch2 as RoutesTo<SSIF1>>::SOSEL), (4, 0));
        assert_eq!((<Ch1 as RoutesTo<SSIF3>>::SOSEL_SHIFT, <Ch1 as RoutesTo<SSIF3>>::SOSEL), (12, 0));
        assert_eq!([ssictrl_tx_bit(0), ssictrl_tx_bit(1), ssictrl_tx_bit(3)], [1 << 14, 1 << 13, 1 << 30]);
    }

    #[test]
    fn an_async_src_into_an_ssi_paces_its_input_from_usb_x1() {
        let _serial = super::path_tests::serial();
        crate::clocks::init(crate::clocks::TEST_CLOCKS);
        test::reset();
        let (out, _rx) = scux_output();
        let chs = Scux::new(unsafe { crate::peripherals::SCUX::steal() });
        let _path = chs.ch0.into_ssi_path(
            AudioInfo::STEREO_24,
            ring(unsafe { DMA_CH0::steal() }),
            Src::Async { input_hz: 48_000, output_hz: 44_100 },
            Dvu::Bypass,
            out,
        );
        assert_eq!(test::peek32(SCUX.fdtsel(0).addr()), input_timing(48_000) | FDTSEL_DIVEN);
        assert_eq!(test::peek32(SCUX.futsel(0).addr()), FDTSEL_DIVEN | FDTSEL_SCKSEL_SSIF0_WS);
    }
}
