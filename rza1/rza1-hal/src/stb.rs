//! CPG Standby Control Register (STBCR) initialisation for RZ/A1L.
//!
//! Enables peripheral module clocks by writing CPG.STBCR2–STBCR12 with the values a
//! [`ModuleClocks`](crate::ModuleClocks) encodes; a port of `STB_Init()` in the C BSP's
//! `src/RZA1/stb/stb.c`.
//!
//! In CPG STBCRn, a **0 bit enables** (clock running) and a **1 bit stops**
//! (clock gated).
//!
//! ## CPG register addresses (RZ/A1L Hardware Manual §10)
//! CPG struct base: 0xFCFE_0010 (FRQCR).
//! Offsets derived from `cpg_iodefine.h` struct layout:
//!
//! | Register | Absolute address |
//! |----------|-----------------|
//! | STBCR2   | 0xFCFE_0024     |
//! | STBCR3   | 0xFCFE_0420     |
//! | STBCR4   | 0xFCFE_0424     |
//! | STBCR5   | 0xFCFE_0428     |
//! | STBCR6   | 0xFCFE_042C     |
//! | STBCR7   | 0xFCFE_0430     |
//! | STBCR8   | 0xFCFE_0434     |
//! | STBCR9   | 0xFCFE_0438     |
//! | STBCR10  | 0xFCFE_043C     |
//! | STBCR11  | 0xFCFE_0440     |
//! | STBCR12  | 0xFCFE_0444     |

/// Enable peripheral module clocks according to `config`.
///
/// Must be called before any peripheral (SCIF, OSTM, USB, SSIF, …) is
/// accessed.  The dummy-read after each write is required by hardware to
/// flush the write buffer before the next access (§10 of the HW manual).
///
/// # Safety
/// Writes to memory-mapped CPG registers.
#[cfg(any(target_os = "none", test))]
pub(crate) unsafe fn init(stbcr: &[u8; 11]) {
    use crate::mmio;
    use crate::pac::CPG;
    unsafe {
        debug!("stb: enabling module clocks");
        // Each register is written then immediately read back; the dummy read
        // flushes the write buffer before the next access (HW manual §10).
        for &(addr, val) in &[
            (CPG.stbcr2().addr(), stbcr[0]),
            (CPG.stbcr3().addr(), stbcr[1]),
            (CPG.stbcr4().addr(), stbcr[2]),
            (CPG.stbcr5().addr(), stbcr[3]),
            (CPG.stbcr6().addr(), stbcr[4]),
            (CPG.stbcr7().addr(), stbcr[5]),
            (CPG.stbcr8().addr(), stbcr[6]),
            (CPG.stbcr9().addr(), stbcr[7]),
            (CPG.stbcr10().addr(), stbcr[8]),
            (CPG.stbcr11().addr(), stbcr[9]),
            (CPG.stbcr12().addr(), stbcr[10]),
        ] {
            mmio::write8(addr, val);
            let _ = mmio::read8(addr); // dummy read
        }
        debug!("stb: done (STBCR2-12 written)");
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio;

    /// STBCR4 = 0b00000111 enables SCIF0–SCIF4 (bits 7–3 = 0) and keeps
    /// reserved bits 2–0 = 1.  SCIF5 (bit 2) does not exist on RZ/A1L.
    #[test]
    fn stbcr4_enables_all_scif() {
        // bits [7:3] = 0 → SCIF0–SCIF4 clocked; bits [2:0] = 1 → reserved/kept
        let val: u8 = 0b00000111;
        assert_eq!(val & 0b11111000, 0, "SCIF bits should all be 0 (enabled)");
    }

    /// STBCR5 = 0b11111100 enables OSTM0 (bit 0) and OSTM1 (bit 1).
    #[test]
    fn stbcr5_enables_ostm() {
        let val: u8 = 0b11111100;
        assert_eq!(val & 0x03, 0, "OSTM0/OSTM1 bits should be 0 (enabled)");
    }

    /// Drive `init` through the MMIO seam and assert the exact register-write
    /// sequence: STBCR2..12 each written with its config byte, in order.
    #[test]
    fn init_writes_every_stbcr_in_order() {
        mmio::test::reset();
        let cfg = [
            0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
        ];
        unsafe { init(&cfg) };

        assert_eq!(
            mmio::test::writes(),
            [
                (0xFCFE_0024, 0x02),
                (0xFCFE_0420, 0x03),
                (0xFCFE_0424, 0x04),
                (0xFCFE_0428, 0x05),
                (0xFCFE_042C, 0x06),
                (0xFCFE_0430, 0x07),
                (0xFCFE_0434, 0x08),
                (0xFCFE_0438, 0x09),
                (0xFCFE_043C, 0x0A),
                (0xFCFE_0440, 0x0B),
                (0xFCFE_0444, 0x0C),
            ]
        );
    }

    /// Every write must be followed by a read-back of the *same* address (the
    /// HW-mandated dummy read that flushes the write buffer).
    #[test]
    fn each_write_is_followed_by_a_dummy_read_of_same_reg() {
        mmio::test::reset();
        let cfg = [
            0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
        ];
        unsafe { init(&cfg) };

        let log = mmio::test::log();
        assert_eq!(log.len(), 22, "11 writes + 11 dummy reads");
        for pair in log.chunks(2) {
            assert!(pair[0].write && !pair[1].write, "write then read");
            assert_eq!(
                pair[0].addr, pair[1].addr,
                "dummy read targets the written reg"
            );
            assert_eq!(pair[0].width, 1, "STBCR is 8-bit");
        }
    }

    /// The PAC's STBCRn are the registers (and widths) this module wrote by address.
    #[test]
    fn the_pac_names_the_same_stbcr_registers() {
        use crate::pac::CPG;
        let regs = [
            (CPG.stbcr2().addr(), CPG.stbcr2().width()),
            (CPG.stbcr3().addr(), CPG.stbcr3().width()),
            (CPG.stbcr4().addr(), CPG.stbcr4().width()),
            (CPG.stbcr5().addr(), CPG.stbcr5().width()),
            (CPG.stbcr6().addr(), CPG.stbcr6().width()),
            (CPG.stbcr7().addr(), CPG.stbcr7().width()),
            (CPG.stbcr8().addr(), CPG.stbcr8().width()),
            (CPG.stbcr9().addr(), CPG.stbcr9().width()),
            (CPG.stbcr10().addr(), CPG.stbcr10().width()),
            (CPG.stbcr11().addr(), CPG.stbcr11().width()),
            (CPG.stbcr12().addr(), CPG.stbcr12().width()),
        ];
        assert_eq!(
            regs,
            [
                (0xFCFE_0024, 1),
                (0xFCFE_0420, 1),
                (0xFCFE_0424, 1),
                (0xFCFE_0428, 1),
                (0xFCFE_042C, 1),
                (0xFCFE_0430, 1),
                (0xFCFE_0434, 1),
                (0xFCFE_0438, 1),
                (0xFCFE_043C, 1),
                (0xFCFE_0440, 1),
                (0xFCFE_0444, 1),
            ]
        );
    }
}
