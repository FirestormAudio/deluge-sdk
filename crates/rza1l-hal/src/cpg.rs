//! CPG (clock pulse generator) bring-up: the L2 cache controller's power control, the PLL and
//! divider ratios (FRQCR), and write access to the on-chip data-retention RAM (SYSCR3).

use crate::mmio;

/// PL310 L2 cache controller power-control register (`0x3FFF_F000` + `0xF80`).
const L2_POWER_CTRL: usize = 0x3FFF_FF80;
/// CPG frequency control register: PLL multiplier and the I:G:B:P1:P0 dividers.
const FRQCR: usize = 0xFCFE_0010;
/// System control register 3: write enable for the data-retention RAM banks
/// (`0x2000_0000`–`0x2001_FFFF`).
const SYSCR3: usize = 0xFCFE_0408;

/// Program the clock pulse generator with the board's FRQCR value (the Deluge's `0x1035` is
/// PLL ×30, I:G:B:P1:P0 = 30:20:10:5:5/2). Each write is read back, as the hardware manual
/// requires before the next access. FRQCR2 is not written: it exists only on the RZ/A1H.
///
/// # Safety
/// Reprograms the CPU and bus clocks: call once at boot, before any peripheral is used.
pub(crate) unsafe fn init(frqcr: u16) {
    unsafe {
        mmio::write32(L2_POWER_CTRL, 1);
        let _ = mmio::read32(L2_POWER_CTRL);
        mmio::write16(FRQCR, frqcr);
        let _ = mmio::read16(FRQCR);
        mmio::write8(SYSCR3, 0x0F);
        let _ = mmio::read8(SYSCR3);
    }
}
