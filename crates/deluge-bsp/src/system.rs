//! Deluge board system initialisation.
//!
//! Centralises all board-specific peripheral configuration values and
//! provides [`BOARD_CONFIG`], the Deluge's chip configuration: boot is
//! `Board::new(rza1_hal::init(BOARD_CONFIG))` ([`crate::Board`]), which keeps
//! chip-level details out of the top-level firmware crate.
//!
//! ## Module clocks
//!
//! [`MODULE_CLOCKS`] runs exactly what the firmware uses and stops the rest, as the original C
//! BSP's `STB_Init()` did: CoreSight, MTU2, the A/D converter (both its bits), SCIF0–4,
//! OSTM0/1, USB0, SCUX, SPIBSC0, RSPI0–2, SSIF0 and SD host interface 01. The realtime clock,
//! which runs at reset, is stopped.

use rza1_hal::clocks::ClockConfig;
use rza1_hal::{ClockOutput, CpgConfig, CpuClockDivider, ModuleClock as M, ModuleClocks};

// ---------------------------------------------------------------------------
// Board-specific configuration constants
// ---------------------------------------------------------------------------

/// The Deluge's EXTAL crystal: 13.2256 MHz (13,225,625 Hz), not the RZ/A1L's 13.33 MHz maximum.
/// Every peripheral clock derives from it (P0φ = 33,064,062.5 Hz, P1φ = 66,128,125 Hz).
pub const CLOCKS: ClockConfig = ClockConfig::new(13_225_625);

/// The CPG's frequency control: the CPU clock at ×1 (400 MHz from the ×30 PLL) and CKO held low
/// in standby, which is FRQCR `0x1035` (I:G:B:P1:P0 = 30:20:10:5:5/2), as the known-working
/// Deluge firmware and bootloader set it.
pub const CPG: CpgConfig = CpgConfig::new()
    .with_cpu_clock(CpuClockDivider::Div1)
    .with_clock_output(ClockOutput::LowInStandby);

/// The module clocks the Deluge firmware runs; everything else stays stopped, to save power and
/// keep spurious interrupt sources quiet.
pub const MODULE_CLOCKS: ModuleClocks = ModuleClocks::new()
    .with(M::CoreSight)
    .with(M::Mtu2)
    .with(M::AdcAnalog)
    .with(M::Adc)
    .with(M::Scif0)
    .with(M::Scif1)
    .with(M::Scif2)
    .with(M::Scif3)
    .with(M::Scif4)
    .with(M::Ostm0)
    .with(M::Ostm1)
    .with(M::Usb0)
    .with(M::Scux)
    .with(M::Spibsc0)
    .with(M::Rspi0)
    .with(M::Rspi1)
    .with(M::Rspi2)
    .with(M::Ssif0)
    // The SD port the card is on; the SDHI driver ungates the clocks of the port it uses.
    .with(M::Sdhi01)
    .without(M::Rtc);

/// The Deluge's chip configuration for `rza1_hal::init`: its 13.2256 MHz crystal, [`CPG`] and
/// [`MODULE_CLOCKS`].
pub const BOARD_CONFIG: rza1_hal::Config = rza1_hal::Config::new(CLOCKS)
    .with_cpg(CPG)
    .with_module_clocks(MODULE_CLOCKS);

/// SSI0's SSICR: I²S master, 24-bit data in 32-bit words, clock ÷4 (44.1 kHz with the Deluge's
/// audio crystal).
///
/// 0x002B_C020 decodes as: CKS=0 (AUDIO_X1 source), DWL=101 (24-bit data words), SWL=011 (32-bit
/// system words), SCKD=SWSD=1 (master), CKDV=0b0010 (AUDIOφ ÷ 4 → BCLK = 22.5792 MHz ÷ 4 =
/// 5.6448 MHz).
pub const SSICR: u32 = 0x002B_C020;

/// SD_OPTION register value for SDHI port 1 on the Deluge board.
///
/// 0x00BD: 4-bit bus, a data timeout of SD_CLK × 2^24 (TOP = 0xB) and a
/// card-detect time of P1 × 2^23 (CTOP = 0xD; RZ/A1L TRM §38.2.12). This is
/// the Deluge C firmware's value, chosen for reliable card detection on this
/// hardware; it differs from the SDHI reset default.
pub const SD_OPTION: u16 = 0x00BD;

// ---------------------------------------------------------------------------
// SCUX DMA channel assignments for the Deluge board
// ---------------------------------------------------------------------------
//
// Pass these as the `dma_ch` argument to `rza1_hal::scux::init_ffd_dma` and
// `rza1_hal::scux::init_ffu_dma`.

/// DMA channel for FFD path 0 (CPU → SCUX, 8-ch, main synthesis / DVU path).
pub const SCUX_FFD0_DMA_CH: u8 = 0;
/// DMA channel for FFD path 1 (CPU → SCUX, 8-ch, sample playback / SRC path).
pub const SCUX_FFD1_DMA_CH: u8 = 3;
/// DMA channel for FFD path 2 (CPU → SCUX, 2-ch stereo aux path).
pub const SCUX_FFD2_DMA_CH: u8 = 2;
/// DMA channel for FFD path 3 (CPU → SCUX, 2-ch stereo aux path).
/// Channel 15 is used to avoid the collision with OLED DMA on channel 4.
pub const SCUX_FFD3_DMA_CH: u8 = 15;
/// DMA channel for FFU path 0 (SCUX → CPU, 8-ch, main capture / SRC path).
pub const SCUX_FFU0_DMA_CH: u8 = 1;
/// DMA channel for FFU path 1 (SCUX → CPU, 8-ch, capture path).
pub const SCUX_FFU1_DMA_CH: u8 = 5;
/// DMA channel for FFU path 2 (SCUX → CPU, 2-ch stereo aux path).
pub const SCUX_FFU2_DMA_CH: u8 = 8;
/// DMA channel for FFU path 3 (SCUX → CPU, 2-ch stereo aux path).
pub const SCUX_FFU3_DMA_CH: u8 = 9;

// ---------------------------------------------------------------------------
// UART (SCIF) DMA channel assignments
// ---------------------------------------------------------------------------
//
// SCIF0 = MIDI DIN; SCIF1 = PIC32 co-processor.
// DMARS values are fixed by the RZ/A1L hardware (SCIF0 RX=0x62, TX=0x61;
// each subsequent SCIF adds 4: SCIF1 RX=0x66, TX=0x65).

/// DMAC channel for SCIF0 (MIDI) receive ring buffer.
pub const MIDI_DMA_RX_CH: u8 = 13;
/// DMAC channel for SCIF1 (PIC32) receive ring buffer.
pub const PIC_DMA_RX_CH: u8 = 12;
/// DMAC channel for SCIF1 (PIC32) transmit.
pub const PIC_DMA_TX_CH: u8 = 10;

// ---------------------------------------------------------------------------
// SD-card DMA channel assignments (SDHI port 1)
// ---------------------------------------------------------------------------

/// DMAC channel for SDHI1 transmit (memory → SD_BUF0; card writes).
pub const SD_DMA_TX_CH: u8 = 11;
/// DMAC channel for SDHI1 receive (SD_BUF0 → memory; card reads).
pub const SD_DMA_RX_CH: u8 = 14;
/// Maximum sectors in a single DMA transfer; determines the bounce-buffer size.
pub const SD_DMA_MAX_SECTORS: usize = 128;

// ---------------------------------------------------------------------------
// Boot-time system initialisation
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::CLOCKS;

    #[test]
    fn deluge_crystal_is_13_2256_mhz() {
        assert_eq!(CLOCKS.extal_hz(), 13_225_625);
        assert_eq!(CLOCKS.p0_hz(), 33_064_062);
        assert_eq!(CLOCKS.p1_hz(), 66_128_125);
    }

    #[test]
    fn sdram_window_is_the_64_mb_part_on_cs3() {
        let w = crate::sdram::WINDOW;
        assert_eq!(w, 0x0C00_0000..0x1000_0000);
        assert_eq!(w.len(), 64 * 1024 * 1024);
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod board_config_tests {
    use super::*;

    #[test]
    fn board_config_encodes_to_the_pre_phase1_registers() {
        // FRQCR and STBCR2–STBCR12 exactly as the raw-register BOARD_CONFIG wrote them.
        assert_eq!(BOARD_CONFIG.cpg.frqcr(), 0x1035);
        let want = [
            0b0110_1010, // STBCR2
            0b1111_0101, // STBCR3
            0b0000_0111, // STBCR4
            0b1111_1100, // STBCR5
            0b0111_1111, // STBCR6
            0b0011_1101, // STBCR7
            0b1111_1101, // STBCR8
            0b1111_0111, // STBCR9
            0b0001_1111, // STBCR10
            0b1101_1111, // STBCR11
            0b1111_1011, // STBCR12
        ];
        for (i, &want) in want.iter().enumerate() {
            let n = i as u8 + 2;
            assert_eq!(BOARD_CONFIG.module_clocks.stbcr(n), want, "STBCR{n}");
        }
    }
}
