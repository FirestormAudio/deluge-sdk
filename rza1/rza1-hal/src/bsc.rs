//! Bus State Controller (BSC) chip-select area timing for CS0 and CS1.
//!
//! The BSC divides the external address space into six areas (CS0–CS5). CS2 and CS3 are
//! configured for SDRAM by the board. This module sets CS0 and CS1 up for a static device (NOR
//! flash, say) with the bus timing the board's part needs: [`CsTiming`] carries the two
//! registers, and [`CsTiming::RENESAS_SAMPLE_NOR`] is the Renesas sample BSP's 16-bit NOR timing.
//!
//! ## BSC register addresses (RZ/A1 HW Manual §8)
//! BSC base: `0x3FFF_C000` (`CMNCR` first).
//!
//! | Register | Offset | Absolute         |
//! |----------|--------|-----------------|
//! | CS0BCR   | 0x04   | `0x3FFF_C004`   |
//! | CS1BCR   | 0x08   | `0x3FFF_C008`   |
//! | CS0WCR   | 0x28   | `0x3FFF_C028`   |
//! | CS1WCR   | 0x2C   | `0x3FFF_C02C`   |
//!
//! Call these after the MMU is enabled (so the BSC register window at `0x3FFF_Cxxx` is in the
//! strongly-ordered I/O region) and before any access to the CS0/CS1 address spaces.

use crate::pac::{BSC, bsc};

/// A chip-select area's bus timing: the `CSnBCR` (bus width, idle cycles, device type) and
/// `CSnWCR` (setup, wait and hold cycles) values, as the RZ/A1 manual §8 lays them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsTiming {
    /// `CSnBCR`.
    pub bcr: u32,
    /// `CSnWCR` (the normal-space layout).
    pub wcr: u32,
}

impl CsTiming {
    /// The Renesas sample BSP's NOR-flash timing (`userdef_bsc_cs0_init` in `bsc_userdef.c`):
    /// 16-bit bus, one idle cycle between accesses, 1.5-cycle setup, 6 wait cycles, 0.5-cycle hold.
    pub const RENESAS_SAMPLE_NOR: CsTiming = CsTiming {
        bcr: 0x1000_0C00,
        wcr: 0x0000_0B40,
    };
}

/// Configure CS0's bus timing. Safe to call with no device on CS0: the timing registers have no
/// side effects until the area is accessed.
///
/// # Safety
/// Writes the BSC's memory-mapped registers; call after [`crate::mmu::init_and_enable`].
pub unsafe fn init_cs0(timing: CsTiming) {
    BSC.cs0bcr().write_value(bsc::regs::Cs0bcr(timing.bcr));
    BSC.cs0wcr().write_value(bsc::regs::Cs0wcr(timing.wcr));
}

/// Configure CS1's bus timing, as [`init_cs0`] does CS0's.
///
/// # Safety
/// Writes the BSC's memory-mapped registers; call after [`crate::mmu::init_and_enable`].
pub unsafe fn init_cs1(timing: CsTiming) {
    BSC.cs1bcr().write_value(bsc::regs::Cs1bcr(timing.bcr));
    BSC.cs1wcr().write_value(bsc::regs::Cs1wcr(timing.wcr));
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio;

    #[test]
    fn init_cs0_writes_the_given_timing_bcr_then_wcr() {
        mmio::test::reset();
        unsafe {
            init_cs0(CsTiming {
                bcr: 0x1234_5600,
                wcr: 0x0000_0789,
            })
        };
        assert_eq!(
            mmio::test::writes(),
            [(0x3FFF_C004, 0x1234_5600), (0x3FFF_C028, 0x0000_0789)],
        );
    }

    #[test]
    fn init_cs1_targets_cs1_registers() {
        mmio::test::reset();
        unsafe { init_cs1(CsTiming::RENESAS_SAMPLE_NOR) };
        assert_eq!(
            mmio::test::writes(),
            [(0x3FFF_C008, 0x1000_0C00), (0x3FFF_C02C, 0x0000_0B40)],
        );
    }

    #[test]
    fn the_sample_nor_timing_is_16_bit_with_6_waits() {
        // 16-bit bus width (IW=0x1) + 6 wait states, from Renesas' bsc_userdef.c.
        assert_eq!(CsTiming::RENESAS_SAMPLE_NOR.bcr, 0x1000_0C00);
        assert_eq!(CsTiming::RENESAS_SAMPLE_NOR.wcr, 0x0000_0B40);
    }

    #[test]
    fn the_pac_names_the_same_bus_registers() {
        use crate::pac::BSC;
        assert_eq!(
            (BSC.cs0bcr().addr(), BSC.cs0bcr().width()),
            (0x3FFF_C004, 4)
        );
        assert_eq!(
            (BSC.cs1bcr().addr(), BSC.cs1bcr().width()),
            (0x3FFF_C008, 4)
        );
        assert_eq!(
            (BSC.cs0wcr().addr(), BSC.cs0wcr().width()),
            (0x3FFF_C028, 4)
        );
        assert_eq!(
            (BSC.cs1wcr().addr(), BSC.cs1wcr().width()),
            (0x3FFF_C02C, 4)
        );
    }
}
