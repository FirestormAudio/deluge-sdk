//! SCUX register map: block base addresses, per-channel register accessors,
//! bit-field constants, and the small `const fn` helpers that derive register
//! values. Split out of `scux.rs` (the driver logic) the same way
//! `usb/regs.rs` is split from the USB driver.
//!
//! Most items are `pub(super)` so the driver in `scux.rs` can reach them via
//! `use regs::*`; the few that form the public SCUX API (`DMARS_*`, `INTIFS_*`,
//! `intifs`, `FDTSEL_*`, `SSICTRL_*`) stay `pub` and are re-exported there.

use crate::dmac::{
    CHCFG_AM_BURST, CHCFG_DAD, CHCFG_DDS_32BIT, CHCFG_DEM, CHCFG_DMS, CHCFG_HIEN, CHCFG_LVL,
    CHCFG_REQD, CHCFG_SAD, CHCFG_SDS_32BIT,
};

// ── SCUX top-level base ───────────────────────────────────────────────────────

pub(super) const SCUX_BASE: usize = 0xE820_8000;

// ── IPC block (×4, stride 0x100) ─────────────────────────────────────────────
// TRM §37.3.1  IPCIR_IPC0_n — Initialization Register
// TRM §37.3.2  IPSLR_IPC0_n — Pass Select Register

pub(super) const IPC_BASE: usize = SCUX_BASE;
pub(super) const IPC_STRIDE: usize = 0x100;

pub(super) const IPCIR_OFF: usize = 0x00; // IPCIR_IPC0_n (§37.3.1)  Init register (bit 0 = INIT)
pub(super) const IPSLR_OFF: usize = 0x04; // IPSLR_IPC0_n (§37.3.2)  Pass-select register

#[inline(always)]
pub(super) fn ipc(ch: u8, off: usize) -> *mut u32 {
    (IPC_BASE + ch as usize * IPC_STRIDE + off) as *mut u32
}

// ── OPC block (×4, stride 0x100) ─────────────────────────────────────────────
// TRM §37.3.3  OPCIR_OPC0_n — Initialization Register
// TRM §37.3.4  OPSLR_OPC0_n — Pass Select Register

pub(super) const OPC_BASE: usize = SCUX_BASE + 0x0400;
pub(super) const OPC_STRIDE: usize = 0x100;

pub(super) const OPCIR_OFF: usize = 0x00; // OPCIR_OPC0_n (§37.3.3)  Init register
pub(super) const OPSLR_OFF: usize = 0x04; // OPSLR_OPC0_n (§37.3.4)  Pass-select register

#[inline(always)]
pub(super) fn opc(ch: u8, off: usize) -> *mut u32 {
    (OPC_BASE + ch as usize * OPC_STRIDE + off) as *mut u32
}

// ── FFD block (×4, stride 0x100) ─────────────────────────────────────────────
// TRM §37.3.5  FFDIR_FFD0_n  — FIFO Download Initialization Register
// TRM §37.3.6  FDAIR_FFD0_n  — FIFO Download Audio Information Register
// TRM §37.3.7  DRQSR_FFD0_n  — FIFO Download Request Size Register
// TRM §37.3.8  FFDPR_FFD0_n  — FIFO Download Pass Register
// TRM §37.3.9  FFDBR_FFD0_n  — FIFO Download Boot Register
// TRM §37.3.10 DEVMR_FFD0_n  — FIFO Download Event Mask Register
// TRM §37.3.11 DEVCR_FFD0_n  — FIFO Download Event Clear Register

pub(super) const FFD_BASE: usize = SCUX_BASE + 0x0800;
pub(super) const FFD_STRIDE: usize = 0x100;

pub(super) const FFDIR_OFF: usize = 0x00; // FFDIR_FFD0_n (§37.3.5)  Init register (bit 0 = INIT)
pub(super) const FDAIR_OFF: usize = 0x04; // FDAIR_FFD0_n (§37.3.6)  Audio info register (channels, bit depth)
pub(super) const DRQSR_OFF: usize = 0x08; // DRQSR_FFD0_n (§37.3.7)  DMA request size register
pub(super) const FFDPR_OFF: usize = 0x0C; // FFDPR_FFD0_n (§37.3.8)  FIFO data pass register
pub(super) const FFDBR_OFF: usize = 0x10; // FFDBR_FFD0_n (§37.3.9)  Boot register (bit 0 = BOOT)
pub(super) const DEVMR_OFF: usize = 0x14; // DEVMR_FFD0_n (§37.3.10) DMA event mode register (0 = DMA trigger, 1 = interrupt)
pub(super) const DEVCR_OFF: usize = 0x1C; // DEVCR_FFD0_n (§37.3.11) DMA event clear register

#[inline(always)]
pub(super) fn ffd(ch: u8, off: usize) -> *mut u32 {
    (FFD_BASE + ch as usize * FFD_STRIDE + off) as *mut u32
}

// ── FFU block (×4, stride 0x100) ─────────────────────────────────────────────
// TRM §37.3.12 FFUIR_FFU0_n  — FIFO Upload Initialization Register
// TRM §37.3.13 FUAIR_FFU0_n  — FIFO Upload Audio Information Register
// TRM §37.3.14 URQSR_FFU0_n  — FIFO Upload Request Size Register
// TRM §37.3.15 FFUPR_FFU0_n  — FIFO Upload Pass Register
// TRM §37.3.16 UEVMR_FFU0_n  — FIFO Upload Event Mask Register
// TRM §37.3.17 UEVCR_FFU0_n  — FIFO Upload Event Clear Register

pub(super) const FFU_BASE: usize = SCUX_BASE + 0x0C00;
pub(super) const FFU_STRIDE: usize = 0x100;

pub(super) const FFUIR_OFF: usize = 0x00; // FFUIR_FFU0_n (§37.3.12) Init register
pub(super) const FUAIR_OFF: usize = 0x04; // FUAIR_FFU0_n (§37.3.13) Audio info register
pub(super) const URQSR_OFF: usize = 0x08; // URQSR_FFU0_n (§37.3.14) DMA request size register
pub(super) const FFUPR_OFF: usize = 0x0C; // FFUPR_FFU0_n (§37.3.15) FIFO data pass register
pub(super) const UEVMR_OFF: usize = 0x10; // UEVMR_FFU0_n (§37.3.16) DMA event mode register (0 = DMA trigger, 1 = interrupt)
pub(super) const UEVCR_OFF: usize = 0x18; // UEVCR_FFU0_n (§37.3.17) DMA event clear register

#[inline(always)]
pub(super) fn ffu(ch: u8, off: usize) -> *mut u32 {
    (FFU_BASE + ch as usize * FFU_STRIDE + off) as *mut u32
}

// ── 2SRC block ───────────────────────────────────────────────────────────────
// TRM §37.3.18 SRCIRp_2SRC0_m  — 2SRC Initialization Register p (m=0,1; p=0,1)
// TRM §37.3.19 SADIRp_2SRC0_m  — 2SRC Audio Information Register p
// TRM §37.3.20 SRCBRp_2SRC0_m  — 2SRC Bypass Register p
// TRM §37.3.21 IFSCRp_2SRC0_m  — 2SRC IFS Control Register p  (INTIFSEN bit 0)
// TRM §37.3.22 IFSVRp_2SRC0_m  — 2SRC IFS Value Setting Register p (INTIFS Q22)
// TRM §37.3.23 SRCCRp_2SRC0_m  — 2SRC Control Register p (SRCMD bit 0; must-be-1 bits 16,8,4)
// TRM §37.3.24 MNFSRp_2SRC0_m  — 2SRC Minimum FS Setting Register p
// TRM §37.3.25 BFSSRp_2SRC0_m  — 2SRC Buffer Size Setting Register p
// TRM §37.3.26 SC2SRp_2SRC0_m  — 2SRC Status Register p (read-only)
// TRM §37.3.27 WATSRp_2SRC0_m  — 2SRC Wait Time Setting Register p
// TRM §37.3.28 SEVMRp_2SRC0_m  — 2SRC Event Mask Register p
// TRM §37.3.29 SEVCRp_2SRC0_m  — 2SRC Event Clear Register p
// TRM §37.3.30 SRCIRR_2SRC0_m  — 2SRC Initialization Register RIF (unit-level)
//
// Two SRC units (unit 0 and unit 1), each containing two SRC paths (pair 0
// and pair 1).  The two paths within a unit share one BFSSR (buffer size
// register) and one SRCCR (common config), but have independent SADIR, IFSCR,
// IFSVR, MNFSR, SEVMR, and SRCIR / SRCIRR registers.
//
// Base of unit 0, pair 0 = 0xE820_9000.  Stride between pairs within a unit = 0x34.
// Unit 1 begins at 0xE820_9100 (stride 0x100 per TRM Table 37.2).

pub(super) const SRC_BASE: usize = 0xE820_9000;
pub(super) const SRC_UNIT_STRIDE: usize = 0x100;
pub(super) const SRC_PAIR_STRIDE: usize = 0x34;

pub(super) const SRCIR_OFF: usize = 0x00; // SRCIRp_2SRC0_m (§37.3.18) Init register
pub(super) const SADIR_OFF: usize = 0x04; // SADIRp_2SRC0_m (§37.3.19) Audio input direction (channels, bit depth)
pub(super) const SRCBR_OFF: usize = 0x08; // SRCBRp_2SRC0_m (§37.3.20) Bypass register (bit 0 = BYPASS)
pub(super) const IFSCR_OFF: usize = 0x0C; // IFSCRp_2SRC0_m (§37.3.21) Input frequency select (0=sync, 1=async)
pub(super) const IFSVR_OFF: usize = 0x10; // IFSVRp_2SRC0_m (§37.3.22) Input frequency value (INTIFS ratio Q22)
pub(super) const SRCCR_OFF: usize = 0x14; // SRCCRp_2SRC0_m (§37.3.23) Common control (must-be-1 bits 16,8,4)
pub(super) const MNFSR_OFF: usize = 0x18; // MNFSRp_2SRC0_m (§37.3.24) Minimum frequency select
pub(super) const BFSSR_OFF: usize = 0x1C; // BFSSRp_2SRC0_m (§37.3.25) Buffer size select
// 0x20: SC2SRp_2SRC0_m (§37.3.26) status register (read-only)
// 0x24: WATSRp_2SRC0_m (§37.3.27) wait time
pub(super) const SEVMR_OFF: usize = 0x28; // SEVMRp_2SRC0_m (§37.3.28) Sampling event mode register
// 0x2C: 4-byte reserved gap
pub(super) const SEVCR_OFF: usize = 0x30; // SEVCRp_2SRC0_m (§37.3.29) Sampling event clear register
// SRCIRR_2SRC0_m (§37.3.30) lives at unit_base + 0x68 (after both pairs); see srcirr() below.
pub(super) const SRCIRR_UNIT_OFF: usize = 0x68;

#[inline(always)]
pub(super) fn src(unit: u8, pair: u8, off: usize) -> *mut u32 {
    (SRC_BASE + unit as usize * SRC_UNIT_STRIDE + pair as usize * SRC_PAIR_STRIDE + off) as *mut u32
}

/// Accessor for SRCIRR (Input-Rate Reload init register), which is per-unit
/// and sits after both pair register banks, at unit_base + 0x68.
#[inline(always)]
pub(super) fn srcirr(unit: u8) -> *mut u32 {
    (SRC_BASE + unit as usize * SRC_UNIT_STRIDE + SRCIRR_UNIT_OFF) as *mut u32
}

// ── DVU block (×4, stride 0x100) ─────────────────────────────────────────────
// TRM §37.3.31 DVUIR_DVU0_n  — DVU Initialization Register
// TRM §37.3.32 VADIR_DVU0_n  — DVU Audio Information Register
// TRM §37.3.33 DVUBR_DVU0_n  — DVU Bypass Register
// TRM §37.3.34 DVUCR_DVU0_n  — DVU Control Register (VRMD bit 4, VVMD bit 8)
// TRM §37.3.35 ZCMCR_DVU0_n  — DVU Zero Cross Mute Control Register
// TRM §37.3.36 VRCTR_DVU0_n  — DVU Volume Ramp Control Register
// TRM §37.3.37 VRPDR_DVU0_n  — DVU Volume Ramp Period Register
// TRM §37.3.38 VRDBR_DVU0_n  — DVU Volume Ramp Decibel Register
// TRM §37.3.39 VRWTR_DVU0_n  — DVU Volume Ramp Wait Time Register
// TRM §37.3.40–47 VOL0R–VOL7R_DVU0_n — Per-channel Volume Value Registers
// TRM §37.3.48 DVUER_DVU0_n  — DVU Enable Register (DVUEN bit 0)
// TRM §37.3.50 VEVMR_DVU0_n  — DVU Volume Event Mask Register
// TRM §37.3.51 VEVCR_DVU0_n  — DVU Volume Event Clear Register
// TRM §37.4.8  DVU Block operation notes

pub(super) const DVU_BASE: usize = 0xE820_9200;
pub(super) const DVU_STRIDE: usize = 0x100;

pub(super) const DVUIR_OFF: usize = 0x00; // DVUIR_DVU0_n (§37.3.31) Init register
pub(super) const VADIR_OFF: usize = 0x04; // VADIR_DVU0_n (§37.3.32) Audio direction (channels, bit depth)
pub(super) const DVUBR_OFF: usize = 0x08; // DVUBR_DVU0_n (§37.3.33) Bypass register (bit 0 = BYPASS)
pub(super) const DVUCR_OFF: usize = 0x0C; // DVUCR_DVU0_n (§37.3.34) Control (VRMD bit 4, VVMD bit 8)
pub(super) const ZCMCR_OFF: usize = 0x10; // ZCMCR_DVU0_n (§37.3.35) Zero-cross mute control
pub(super) const VRCTR_OFF: usize = 0x14; // VRCTR_DVU0_n (§37.3.36) Volume ramp control (bit 0 = enable ramp)
pub(super) const VRPDR_OFF: usize = 0x18; // VRPDR_DVU0_n (§37.3.37) Volume ramp period
pub(super) const VRDBR_OFF: usize = 0x1C; // VRDBR_DVU0_n (§37.3.38) Volume ramp dB step
pub(super) const VRWTR_OFF: usize = 0x20; // VRWTR_DVU0_n (§37.3.39) Volume ramp wait time register
pub(super) const VOL0R_OFF: usize = 0x24; // VOL0R–VOL7R_DVU0_n (§37.3.40–47) Per-channel volume registers
pub(super) const DVUER_OFF: usize = 0x44; // DVUER_DVU0_n (§37.3.48) DVU enable register (DVUEN bit 0)
pub(super) const VEVMR_OFF: usize = 0x4C; // VEVMR_DVU0_n (§37.3.50) Volume event mode
pub(super) const VEVCR_OFF: usize = 0x54; // VEVCR_DVU0_n (§37.3.51) Volume event clear (0x50 is a 4-byte gap)

#[inline(always)]
pub(super) fn dvu(ch: u8, off: usize) -> *mut u32 {
    (DVU_BASE + ch as usize * DVU_STRIDE + off) as *mut u32
}

/// Offset of VOL register for a specific audio sub-channel within a DVU instance.
#[inline(always)]
pub(super) fn vol_off(audio_ch: u8) -> usize {
    VOL0R_OFF + audio_ch as usize * 4
}

// ── MIX block ────────────────────────────────────────────────────────────────
// TRM §37.3.52 MIXIR_MIX0_0  — MIX Initialization Register
// TRM §37.3.53 MADIR_MIX0_0  — MIX Audio Information Register
// TRM §37.3.54 MIXBR_MIX0_0  — MIX Bypass Register
// TRM §37.3.55 MIXMR_MIX0_0  — MIX Mode Register
// TRM §37.3.56 MVPDR_MIX0_0  — MIX Volume Period Register
// TRM §37.3.57 MDBAR_MIX0_0  — MIX Decibel A Register (source 0 gain)
// TRM §37.3.58 MDBBR_MIX0_0  — MIX Decibel B Register (source 1 gain)
// TRM §37.3.59 MDBCR_MIX0_0  — MIX Decibel C Register (source 2 gain)
// TRM §37.3.60 MDBDR_MIX0_0  — MIX Decibel D Register (source 3 gain)
// TRM §37.3.61 MDBER_MIX0_0  — MIX Decibel Enable Register (MIXDBEN bit 0)
// TRM §37.4.9  MIX Block operation notes

pub(super) const MIX_BASE: usize = 0xE820_9600;

pub(super) const MIXIR_OFF: usize = 0x00; // MIXIR_MIX0_0 (§37.3.52) Init register
pub(super) const MADIR_OFF: usize = 0x04; // MADIR_MIX0_0 (§37.3.53) Audio direction
pub(super) const MIXBR_OFF: usize = 0x08; // MIXBR_MIX0_0 (§37.3.54) Bypass register (bit 0 = BYPASS)
pub(super) const MIXMR_OFF: usize = 0x0C; // MIXMR_MIX0_0 (§37.3.55) Mix mode register
pub(super) const MVPDR_OFF: usize = 0x10; // MVPDR_MIX0_0 (§37.3.56) Master volume period
pub(super) const MDB0R_OFF: usize = 0x14; // MDBAR_MIX0_0 (§37.3.57) Mix data buffer A — source 0 gain
pub(super) const MDB1R_OFF: usize = 0x18; // MDBBR_MIX0_0 (§37.3.58) Mix data buffer B — source 1 gain
pub(super) const MDB2R_OFF: usize = 0x1C; // MDBCR_MIX0_0 (§37.3.59) Mix data buffer C — source 2 gain
pub(super) const MDB3R_OFF: usize = 0x20; // MDBDR_MIX0_0 (§37.3.60) Mix data buffer D — source 3 gain
pub(super) const MDBER_OFF: usize = 0x24; // MDBER_MIX0_0 (§37.3.61) Mix data buffer enable

#[inline(always)]
pub(super) fn mix(off: usize) -> *mut u32 {
    (MIX_BASE + off) as *mut u32
}

// ── CIM block ─────────────────────────────────────────────────────────────────
// TRM §37.3.63 SWRSR_CIM     — Software Reset Register
// TRM §37.3.64 DMACR_CIM     — DMA Control Register (bits 3:0=FFD0-3, bits 7:4=FFU0-3)
// TRM §37.3.65 DMATDn_CIM    — DMA Transfer Registers for FFD0_n (n=0–3)
// TRM §37.3.66 DMATUn_CIM    — DMA Transfer Registers for FFU0_n (n=0–3)
// TRM §37.3.67 SSIRSEL_CIM   — SSI Route Select Register
// TRM §37.3.68 FDTSELn_CIM   — FFD0_n Timing Select Register (n=0–3)
// TRM §37.3.69 FUTSELn_CIM   — FFU0_n Timing Select Register (n=0–3)
// TRM §37.3.70 SSIPMD_CIM    — SSI Pin Mode Register
// TRM §37.3.71 SSICTRL_CIM   — SSI Control Register (SSI012TEN bit 14, SSI012REN bit 8)
// TRM §37.3.72 SRCRSELn_CIM  — SRCn Route Select Register (n=0–3; reset = 0x76543210)
// TRM §37.3.73 MIXRSEL_CIM   — MIX Route Select Register (reset = 0x76543210)

pub(super) const CIM_BASE: usize = 0xE820_9700;

pub(super) const SWRSR_CIM_OFF: usize = 0x00; // SWRSR_CIM    (§37.3.63) Software reset (bit 0; 0=reset, 1=run)
pub(super) const DMACR_CIM_OFF: usize = 0x04; // DMACR_CIM    (§37.3.64) DMA enable (bits 3:0=FFD0-3 TX, bits 7:4=FFU0-3 RX)
pub(super) const DMATD0_CIM_OFF: usize = 0x08; // DMATD0_CIM   (§37.3.65) DMA channel number for FFD0
pub(super) const DMATD1_CIM_OFF: usize = 0x0C; // DMATD1_CIM   (§37.3.65) DMA channel number for FFD1
pub(super) const DMATD2_CIM_OFF: usize = 0x10; // DMATD2_CIM   (§37.3.65) DMA channel number for FFD2
pub(super) const DMATD3_CIM_OFF: usize = 0x14; // DMATD3_CIM   (§37.3.65) DMA channel number for FFD3
pub(super) const DMATU0_CIM_OFF: usize = 0x18; // DMATU0_CIM   (§37.3.66) DMA channel number for FFU0
pub(super) const DMATU1_CIM_OFF: usize = 0x1C; // DMATU1_CIM   (§37.3.66) DMA channel number for FFU1
pub(super) const DMATU2_CIM_OFF: usize = 0x20; // DMATU2_CIM   (§37.3.66) DMA channel number for FFU2
pub(super) const DMATU3_CIM_OFF: usize = 0x24; // DMATU3_CIM   (§37.3.66) DMA channel number for FFU3
// 0x28–0x37: 16-byte reserved gap (TRM Table 37.2, between DMATUn and SSIRSEL)
pub(super) const SSIRSEL_CIM_OFF: usize = 0x38; // SSIRSEL_CIM  (§37.3.67) SSI→SRC route selector
pub(super) const FDTSEL0_CIM_OFF: usize = 0x3C; // FDTSEL0_CIM  (§37.3.68) FFD0 timing select
pub(super) const FDTSEL1_CIM_OFF: usize = 0x40; // FDTSEL1_CIM  (§37.3.68) FFD1 timing select
pub(super) const FDTSEL2_CIM_OFF: usize = 0x44; // FDTSEL2_CIM  (§37.3.68) FFD2 timing select
pub(super) const FDTSEL3_CIM_OFF: usize = 0x48; // FDTSEL3_CIM  (§37.3.68) FFD3 timing select
pub(super) const FUTSEL0_CIM_OFF: usize = 0x4C; // FUTSEL0_CIM  (§37.3.69) FFU0 timing select
pub(super) const FUTSEL1_CIM_OFF: usize = 0x50; // FUTSEL1_CIM  (§37.3.69) FFU1 timing select
pub(super) const FUTSEL2_CIM_OFF: usize = 0x54; // FUTSEL2_CIM  (§37.3.69) FFU2 timing select
pub(super) const FUTSEL3_CIM_OFF: usize = 0x58; // FUTSEL3_CIM  (§37.3.69) FFU3 timing select
pub(super) const SSIPMD_CIM_OFF: usize = 0x5C; // SSIPMD_CIM   (§37.3.70) SSI port mode
pub(super) const SSICTRL_CIM_OFF: usize = 0x60; // SSICTRL_CIM  (§37.3.71) SSI clock/gate control
pub(super) const SRCRSEL0_CIM_OFF: usize = 0x64; // SRCRSEL0_CIM (§37.3.72) SRC0 route select (reset = 0x76543210)
pub(super) const SRCRSEL1_CIM_OFF: usize = 0x68; // SRCRSEL1_CIM (§37.3.72) SRC1 route select
pub(super) const SRCRSEL2_CIM_OFF: usize = 0x6C; // SRCRSEL2_CIM (§37.3.72) SRC2 route select
pub(super) const SRCRSEL3_CIM_OFF: usize = 0x70; // SRCRSEL3_CIM (§37.3.72) SRC3 route select
pub(super) const MIXRSEL_CIM_OFF: usize = 0x74; // MIXRSEL_CIM  (§37.3.73) MIX input route selector (reset = 0x76543210)

#[inline(always)]
pub(super) fn cim(off: usize) -> *mut u32 {
    (CIM_BASE + off) as *mut u32
}

// ── DMARS constants ───────────────────────────────────────────────────────────

/// DMARS resource-selector for SCUX input FIFO 0 TX (CPU → FFD0_0).
pub const DMARS_SCUTXI0: u32 = 0x0101;
/// DMARS resource-selector for SCUX input FIFO 1 TX (CPU → FFD0_1).
/// BSP: DMA_RS_SCUTXI1 = 0x105
pub const DMARS_SCUTXI1: u32 = 0x0105;
/// DMARS resource-selector for SCUX input FIFO 2 TX (CPU → FFD0_2, 2-ch path).
pub const DMARS_SCUTXI2: u32 = 0x0109;
/// DMARS resource-selector for SCUX input FIFO 3 TX (CPU → FFD0_3, 2-ch path).
pub const DMARS_SCUTXI3: u32 = 0x010D;
/// DMARS resource-selector for SCUX output FIFO 0 RX (FFU0_0 → CPU).
pub const DMARS_SCURXI0: u32 = 0x0102;
/// DMARS resource-selector for SCUX output FIFO 1 RX (FFU0_1 → CPU).
/// BSP: DMA_RS_SCURXI1 = 0x106
pub const DMARS_SCURXI1: u32 = 0x0106;
/// DMARS resource-selector for SCUX output FIFO 2 RX (FFU0_2 → CPU, 2-ch path).
pub const DMARS_SCURXI2: u32 = 0x010A;
/// DMARS resource-selector for SCUX output FIFO 3 RX (FFU0_3 → CPU, 2-ch path).
pub const DMARS_SCURXI3: u32 = 0x010E;

// ── DMA channel assignments (Deluge) — moved to deluge-bsp ───────────────────
//
// The DMA channels previously declared here as FFD*_DMA_CH / FFU*_DMA_CH
// public constants are board-specific and are now provided by the caller via
// the `dma_ch` parameter of [`init_ffd_dma`] and [`init_ffu_dma`].
// The Deluge values can be found in `deluge_bsp::system`.

/// Link-descriptor HEADER word (same field encoding as SSI — see ssi.rs `DESC_HEADER`).
/// bits: LDEN(3)=1, NXA(2)=1, reserved(1)=0, valid(0)=1 → 0b1101.
pub(super) const DESC_HEADER: u32 = 0b1101;

// CHCFG for SCUX FFD DMA (SRAM → SCUX FIFO, source increments, destination fixed):
//   DMS | DEM | DAD | DDS_32BIT | SDS_32BIT | AM_BURST | HIEN | REQD | LVL | channel
//   = 0x8000_0000 | 0x0100_0000 | 0x0020_0000 | 0x0002_0000 | 0x0000_2000
//     | 0x0000_0200 | 0x0000_0020 | 0x0000_0008 | 0x0000_0040 | ch
//   = 0x8122_2268 | ch  (matches C BSP scux_dev.c)
pub(super) const fn ffd_chcfg(dma_ch: u8) -> u32 {
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

// CHCFG for SCUX FFU DMA (SCUX FIFO → SRAM, source fixed, destination increments):
//   DMS | DEM | SAD | DDS_32BIT | SDS_32BIT | AM_BURST | HIEN | LVL | channel
//   = 0x8000_0000 | 0x0100_0000 | 0x0010_0000 | 0x0002_0000 | 0x0000_2000
//     | 0x0000_0200 | 0x0000_0020 | 0x0000_0040 | ch
//   = 0x8112_2260 | ch  (matches C BSP)
pub(super) const fn ffu_chcfg(dma_ch: u8) -> u32 {
    CHCFG_DMS
        | CHCFG_DEM
        | CHCFG_SAD
        | CHCFG_DDS_32BIT
        | CHCFG_SDS_32BIT
        | CHCFG_AM_BURST
        | CHCFG_HIEN
        | CHCFG_LVL
        | (dma_ch as u32 & 7)
}

// ── INTIFS helper constants (Q22 ratio: fin/fout * 2^22) ─────────────────────

/// Interpolation ratio for 44.1 kHz → 44.1 kHz (unity, passthrough mode).
///
/// TRM Table 37.3 gives unity as 0x0400000 (= 2^22; FSO is fixed at 2^22).
/// These constants were previously hand-written with an extra hex digit
/// (0x0400_0000 = 2^26, 16× too large); deriving them via [`intifs`] keeps
/// them consistent with the documented Q22 encoding.
pub const INTIFS_44100_TO_44100: u32 = intifs(44100, 44100);
/// Interpolation ratio for 44.1 kHz → 48 kHz.
pub const INTIFS_44100_TO_48000: u32 = intifs(44100, 48000);
/// Interpolation ratio for 48 kHz → 44.1 kHz.
pub const INTIFS_48000_TO_44100: u32 = intifs(48000, 44100);
/// Interpolation ratio for 48 kHz → 48 kHz (unity).
pub const INTIFS_48000_TO_48000: u32 = intifs(48000, 48000);
/// Interpolation ratio for 88.2 kHz → 44.1 kHz (exactly 2:1).
/// Use when CPU synthesises at 88.2 kHz for higher quality, output at 44.1 kHz.
pub const INTIFS_88200_TO_44100: u32 = intifs(88200, 44100);
/// Interpolation ratio for 96 kHz → 44.1 kHz.
/// Use when CPU synthesises at 96 kHz, output at 44.1 kHz.
pub const INTIFS_96000_TO_44100: u32 = intifs(96000, 44100);
/// Interpolation ratio for 96 kHz → 48 kHz (exactly 2:1).
pub const INTIFS_96000_TO_48000: u32 = intifs(96000, 48000);

/// Compute an arbitrary INTIFS ratio value for any integer input/output rate.
///
/// The INTIFS field is written to IFSVRp_2SRC0_m (TRM §37.3.22) as a Q22
/// fixed-point value: `INTIFS = 2^22 × (Fin / Fout)`.  FSO is fixed at
/// `2^22 = 0x0040_0000`; FSI tracks the ratio continuously in async mode.
///
/// Example (TRM §37.3.22): Fin=32 kHz, Fout=44.1 kHz →
/// `INTIFS = 2^22 × 32000/44100 = 3043485 = 0x02E709D`.
///
/// Returns 0 if `fout_hz` is zero.
pub const fn intifs(fin_hz: u32, fout_hz: u32) -> u32 {
    if fout_hz == 0 {
        return 0;
    }
    (((fin_hz as u64) << 22) / fout_hz as u64) as u32
}

// ── SRCCR must-be-1 bits ─────────────────────────────────────────────────────

/// SRCCR must-be-1 bits (bits 16, 8, and 4 per TRM §37 note 1).
/// BSP: SRCCR_2SRC0_BASE_VALUE = 0x00010110U
pub(super) const SRCCR_MBZ1: u32 = (1 << 16) | (1 << 8) | (1 << 4);
/// SRCCR bit 0 = SRCMD: 0 = async mode, 1 = sync mode.
/// BSP: SRCCR_2SRC0_SRCMD_SET = (1U << 0)
pub(super) const SRCCR_SYNC: u32 = 1 << 0;

// ── Sub-block register bit constants (verified vs vendor/rbsp scux.h) ─────────

/// SWRSR bit 0: 0 = reset asserted, 1 = normal operation.
pub(super) const SWRSR_RESET: u32 = 0;
pub(super) const SWRSR_RUN: u32 = 1;

/// Generic INIT bit (bit 0) for all xxxIR registers (FFD/FFU/SRC/DVU/MIX/IPC/OPC).
/// Writing 1 asserts init (stops block); writing 0 releases init (starts block).
pub(super) const INIT_SET: u32 = 1; // BSP: FFDIR_FFD0_INIT_SET etc.
pub(super) const INIT_CLR: u32 = 0;

/// FFDBR bit 0: set 1 after clearing FFDIR.INIT to boot the FIFO.
/// BSP: FFDBR_FFD0_BOOT_SET = (1U << 0)
pub(super) const FFDBR_BOOT: u32 = 1 << 0;

/// FFDPR bit 0: 1 = enable async data path (CIM→FFD→IPC).
/// BSP: FFDPR_FFD0_PASS_SET_ASYNC = (1U << 0)
pub(super) const FFDPR_PASS_ASYNC: u32 = 1 << 0;

/// FFUPR bit 0: 1 = enable async data path (OPC→FFU→CIM).
/// BSP: FFUPR_FFU0_PASS_SET_ASYNC = (1U << 0)
pub(super) const FFUPR_PASS_ASYNC: u32 = 1 << 0;

/// SRCBR bit 0: 1 = bypass SRC (audio passes unchanged).
/// BSP: SRCBR_2SRC0_BYPASS_SET = (0x1U << 0)
pub(super) const SRCBR_BYPASS: u32 = 1 << 0;

/// IFSCR bit 0: 1 = use INTIFS (async mode), 0 = sync mode.
/// BSP: IFSCR_2SRC0_INTIFSEN_SET = (0x1U << 0)
pub(super) const IFSCR_INTIFSEN: u32 = 1 << 0;

/// DVUBR bit 0: 1 = bypass DVU.
/// BSP: DVUBR_DVU0_BYPASS_SET = (0x1U << 0)
pub(super) const DVUBR_BYPASS: u32 = 1 << 0;

/// DVUCR bit 4: enable volume ramp mode (VRMD).
/// BSP: DVUCR_DVU0_VRMD_SET = (1U << 4)
/// The BSP ALWAYS sets this when DVU is enabled (not bypassed), even with
/// zero-rate "dummy" ramp parameters (VRPDR=0, VRDBR=0).  This appears to
/// be mandatory to open the volume-control data path through the DVU.
pub(super) const DVUCR_VRMD: u32 = 1 << 4;

/// DVUCR bit 8: enable direct digital volume mode (VVMD).
/// BSP: DVUCR_DVU0_VVMD_SET = (1U << 8)
pub(super) const DVUCR_VVMD: u32 = 1 << 8;

/// VRCTR bit 0: per-channel volume ramp enable (bit N = channel N).
/// BSP: VRCTR_DVU0_VREN_SET = (1U << 0)
pub(super) const VRCTR_VREN: u32 = 1 << 0;

/// ZCMCR bit 0: per-channel zero-cross mute enable (bit N = channel N).
/// BSP: ZCMCR_DVU0_ZCEN_SET = (1U << 0)
pub(super) const ZCMCR_ZCEN: u32 = 1 << 0;

/// DVUER bit 0: activate DVU volume register settings.
/// Must be written AFTER DVUIR.INIT is cleared.
/// BSP: DVUER_DVU0_DVUEN_SET = (1U << 0)
pub(super) const DVUER_DVUEN: u32 = 1 << 0;

/// MIXBR bit 0: 1 = bypass MIX (first source passes through).
pub(super) const MIXBR_BYPASS: u32 = 1 << 0;

/// MDBER bit 0: enable MIX data buffer (enables per-source gain coefficients).
/// BSP: MDBER_MIX0_MIXDBEN_SET = (1U << 0)
pub(super) const MDBER_MIXDBEN: u32 = 1 << 0;

/// FDTSEL/FUTSEL bit 8: enable divided clock output (DIVEN).
/// BSP: FDTSEL_CIM_DIVEN_SET = (1U << 8)
pub const FDTSEL_DIVEN: u32 = 1 << 8;

/// FDTSEL/FUTSEL SCKSEL value for SSIF0 WS (bits [3:0] = 8).
/// BSP: FDTSEL_CIM_SCKSEL_SSIF0_WS_SET = (8U)
pub const FDTSEL_SCKSEL_SSIF0_WS: u32 = 8;

/// SSICTRL bit 14: SCUX drives SSIF0 TX directly.
/// BSP: SSICTRL_CIM_SSI0TX_SET = (1U << 14)
pub const SSICTRL_SSI0TX: u32 = 1 << 14;

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    // SCUX_BASE = 0xE820_8000. Each sub-block (IPC/OPC/FFD/FFU) is a bank of
    // per-channel register windows STRIDE bytes apart.
    #[test]
    fn block_bases_match_datasheet_offsets() {
        assert_eq!(IPC_BASE, 0xE820_8000);
        assert_eq!(OPC_BASE, 0xE820_8400);
        assert_eq!(FFD_BASE, 0xE820_8800);
        assert_eq!(FFU_BASE, 0xE820_8C00);
    }

    #[test]
    fn channel_addressing_is_base_plus_stride_plus_offset() {
        assert_eq!(ipc(0, IPCIR_OFF) as usize, 0xE820_8000);
        assert_eq!(ipc(1, IPCIR_OFF) as usize, 0xE820_8100);
        assert_eq!(ipc(2, IPSLR_OFF) as usize, 0xE820_8200 + 0x04);

        assert_eq!(opc(0, OPCIR_OFF) as usize, 0xE820_8400);
        assert_eq!(opc(3, OPSLR_OFF) as usize, 0xE820_8400 + 3 * 0x100 + 0x04);

        assert_eq!(ffd(0, FFDIR_OFF) as usize, 0xE820_8800);
        assert_eq!(ffd(3, FFDPR_OFF) as usize, 0xE820_8800 + 3 * 0x100 + 0x0C);

        assert_eq!(ffu(0, FFUIR_OFF) as usize, 0xE820_8C00);
        assert_eq!(ffu(2, FUAIR_OFF) as usize, 0xE820_8C00 + 2 * 0x100 + 0x04);
    }

    #[test]
    fn per_channel_windows_do_not_overlap() {
        for ch in 0..3u8 {
            assert_eq!(ffd(ch + 1, 0) as usize - ffd(ch, 0) as usize, FFD_STRIDE);
            assert_eq!(ffu(ch + 1, 0) as usize - ffu(ch, 0) as usize, FFU_STRIDE);
        }
    }

    #[test]
    fn intifs_unity_is_q22_one_not_the_old_16x_bug() {
        // INTIFS is Q22: unity (Fin==Fout) must be exactly 2^22 = 0x0040_0000.
        // A previous hand-written table used 0x0400_0000 (2^26, 16× too large) —
        // guard against that regression.
        assert_eq!(intifs(44100, 44100), 0x0040_0000);
        assert_eq!(intifs(48000, 48000), 0x0040_0000);
        assert_eq!(INTIFS_44100_TO_44100, 0x0040_0000);
        assert_ne!(
            INTIFS_44100_TO_44100, 0x0400_0000,
            "must not be 16× too large"
        );
    }

    #[test]
    fn intifs_integer_ratios_are_exact() {
        // 2:1 downsample → 2 × 2^22 = 2^23.
        assert_eq!(intifs(88200, 44100), 0x0080_0000);
        assert_eq!(intifs(96000, 48000), 0x0080_0000);
        assert_eq!(INTIFS_88200_TO_44100, 0x0080_0000);
        // Fin < Fout → ratio below unity.
        assert!(intifs(44100, 48000) < 0x0040_0000);
        // Defensive: divide-by-zero guard.
        assert_eq!(intifs(48000, 0), 0);
    }

    #[test]
    fn intifs_matches_q22_formula() {
        for (fin, fout) in [(32000u32, 44100u32), (44100, 48000), (96000, 44100)] {
            let expected = (((fin as u64) << 22) / fout as u64) as u32;
            assert_eq!(intifs(fin, fout), expected, "{fin}->{fout}");
        }
    }

    #[test]
    fn ffd_and_ffu_chcfg_direction_and_channel() {
        // FFD (CPU→SCUX): destination fixed (DAD) + dest-triggered (REQD).
        assert_eq!(ffd_chcfg(0) & CHCFG_DAD, CHCFG_DAD);
        assert_eq!(ffd_chcfg(0) & CHCFG_REQD, CHCFG_REQD);
        // FFU (SCUX→CPU): source fixed (SAD) + completion interrupt (DEM).
        assert_eq!(ffu_chcfg(0) & CHCFG_SAD, CHCFG_SAD);
        assert_eq!(ffu_chcfg(0) & CHCFG_DEM, CHCFG_DEM);
        assert_eq!(ffu_chcfg(0) & CHCFG_REQD, 0, "FFU is source-triggered");
        // Channel encoded in low 3 bits.
        assert_eq!(ffd_chcfg(5) & 7, 5);
        assert_eq!(ffu_chcfg(3) & 7, 3);
        assert_eq!(ffd_chcfg(7), ffd_chcfg(0) | 7);
    }

    #[test]
    fn vol_register_offsets_are_4_apart() {
        assert_eq!(vol_off(0), VOL0R_OFF);
        assert_eq!(vol_off(1), VOL0R_OFF + 4);
        assert_eq!(vol_off(7), VOL0R_OFF + 28);
    }
}
