//! CPG (clock pulse generator) bring-up: the L2 cache controller's power control, the PLL and
//! divider ratios (FRQCR), and write access to the on-chip data-retention RAM (SYSCR3).

use crate::pac::{CPG, L2C, cpg};

/// Program the clock pulse generator with the board's FRQCR value (the Deluge's `0x1035` is
/// PLL ×30, I:G:B:P1:P0 = 30:20:10:5:5/2). Each write is read back, as the hardware manual
/// requires before the next access. FRQCR2 is not written: it exists only on the RZ/A1H.
///
/// # Safety
/// Reprograms the CPU and bus clocks: call once at boot, before any peripheral is used.
pub(crate) unsafe fn init(frqcr: u16) {
    // The PL310's power control, then the PLL and dividers (FRQCR), then write access to the
    // data-retention RAM banks 0x2000_0000–0x2001_FFFF (SYSCR3).
    L2C.reg15_power_ctrl().write_value(1);
    let _ = L2C.reg15_power_ctrl().read();
    CPG.frqcr().write_value(cpg::regs::Frqcr(frqcr));
    let _ = CPG.frqcr().read();
    CPG.syscr3().write_value(cpg::regs::Syscr3(0x0F));
    let _ = CPG.syscr3().read();
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    #[test]
    fn the_pac_names_the_same_clock_registers() {
        use crate::pac::{CPG, L2C};
        assert_eq!((CPG.frqcr().addr(), CPG.frqcr().width()), (0xFCFE_0010, 2));
        assert_eq!(
            (CPG.syscr3().addr(), CPG.syscr3().width()),
            (0xFCFE_0408, 1)
        );
        assert_eq!(
            (
                L2C.reg15_power_ctrl().addr(),
                L2C.reg15_power_ctrl().width()
            ),
            (0x3FFF_FF80, 4)
        );
    }
}
