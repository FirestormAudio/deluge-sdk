//! SCUX register values: bit-field constants and the small `const fn` helpers
//! that derive register values (the addresses come from `crate::pac::SCUX`). The driver logic is in `scux.rs`.
//!
//! Most items are `pub(super)` so the driver in `scux.rs` can reach them via
//! `use regs::*`; the few that form the public SCUX API (`DMARS_*`, `INTIFS_*`,
//! `intifs`, `FDTSEL_*`, `SSICTRL_*`) stay `pub` and are re-exported there.

use crate::dmac::{
    CHCFG_AM_BURST, CHCFG_DAD, CHCFG_DDS_32BIT, CHCFG_DEM, CHCFG_DMS, CHCFG_HIEN, CHCFG_LVL,
    CHCFG_REQD, CHCFG_SAD, CHCFG_SDS_32BIT,
};

// ── Register addresses ────────────────────────────────────────────────────────
// Every SCUX register address comes from `crate::pac::SCUX` (TRM §37.3, Table 37.2):
// `ipc(n)`/`opc(n)`/`ffd(n)`/`ffu(n)`/`dvu(n)` are the four per-channel banks
// (stride 0x100), `src(2 * unit + pair)` the 2SRC pairs (units 0x100 apart,
// pairs 0x34 apart within a unit), `srcirr(unit)` each unit's RIF init
// register, `mix()` the MIX block, and the CIM registers sit flat on `SCUX`
// (`swrsr_cim()`, `dmacr_cim()`, `dmatd(n)`, `dmatu(n)`, `fdtsel(n)`,
// `futsel(n)`, `srcrsel(n)`, ...).

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

// ── DMA channel assignments ──────────────────────────────────────────────────
//
// DMA channels are board-specific: callers pass them as the `dma_ch` parameter
// of `init_ffd_dma` / `init_ffu_dma`. The Deluge values are in
// `deluge_bsp::system`.

// CHCFG for SCUX FFD DMA (SRAM → SCUX FIFO, source increments, destination fixed):
//   DMS | DEM | DAD | DDS_32BIT | SDS_32BIT | AM_BURST | HIEN | REQD | LVL | channel
//   = 0x8000_0000 | 0x0100_0000 | 0x0020_0000 | 0x0002_0000 | 0x0000_2000
//     | 0x0000_0200 | 0x0000_0020 | 0x0000_0008 | 0x0000_0040 | ch
//   = 0x8122_2268 | ch  (the C BSP scux_dev.c value; `ffd_chcfg` omits DEM, so
//     the end interrupt is not masked and the result is 0x8022_2268 | ch)
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
/// All the `INTIFS_*` constants are derived via [`intifs`] so they follow
/// the documented Q22 encoding.
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
/// The BSP always sets this when the DVU is enabled (not bypassed), even with
/// zero-rate "dummy" ramp parameters (VRPDR=0, VRDBR=0); it appears to be
/// required to open the volume-control data path through the DVU.
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

/// MDBER bit 0: MIXDBEN — apply the per-source MDBxR gain settings.
/// BSP: MDBER_MIX0_MIXDBEN_SET = (1U << 0)
pub(super) const MDBER_MIXDBEN: u32 = 1 << 0;

/// FDTSEL/FUTSEL bit 8: enable divided clock output (DIVEN).
/// BSP: FDTSEL_CIM_DIVEN_SET = (1U << 8)
pub const FDTSEL_DIVEN: u32 = 1 << 8;

/// FDTSEL/FUTSEL SCKSEL value for SSIF0 WS (bits `[3:0]` = 8).
/// BSP: FDTSEL_CIM_SCKSEL_SSIF0_WS_SET = (8U)
pub const FDTSEL_SCKSEL_SSIF0_WS: u32 = 8;

/// SSICTRL bit 14: SCUX drives SSIF0 TX directly.
/// BSP: SSICTRL_CIM_SSI0TX_SET = (1U << 14)
pub const SSICTRL_SSI0TX: u32 = 1 << 14;

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn intifs_unity_is_q22_one_not_the_old_16x_bug() {
        // INTIFS is Q22: unity (Fin==Fout) must be exactly 2^22 = 0x0040_0000.
        // Guard against the easy mis-encoding 0x0400_0000 (2^26, 16× too large).
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
        // FFU (SCUX→CPU): source fixed (SAD), end interrupt masked (DEM).
        assert_eq!(ffu_chcfg(0) & CHCFG_SAD, CHCFG_SAD);
        assert_eq!(ffu_chcfg(0) & CHCFG_DEM, CHCFG_DEM);
        assert_eq!(ffu_chcfg(0) & CHCFG_REQD, 0, "FFU is source-triggered");
        // Channel encoded in low 3 bits.
        assert_eq!(ffd_chcfg(5) & 7, 5);
        assert_eq!(ffu_chcfg(3) & 7, 3);
        assert_eq!(ffd_chcfg(7), ffd_chcfg(0) | 7);
    }
}
