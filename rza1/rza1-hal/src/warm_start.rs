//! Undoes what a warm start left running.
//!
//! A debugger's halt-only reset, or a bootloader that jumps here, restarts the core but not the
//! peripherals: DMA channels keep transferring, the SSI keeps streaming, the SCUX keeps its paths
//! and the SPI flash controller stays in whatever mode it was in, all as the previous program set
//! them up. A flash controller left in manual mode makes every memory-mapped flash read return zeros. A running SCUX holds SSIF0's transmitter even
//! with its SSIF routing register at 0, so the TX FIFO never drains, the SSI never requests its
//! DMA, and audio stays silent. [`quiesce`] puts those modules back into their reset state before
//! any driver starts. What the GIC was left holding is [`crate::gic::init`]'s to undo.

/// Stop every DMA channel, reset the SCUX and every SSI channel, and put the flash controller back
/// into memory-mapped read mode.
///
/// The DMA goes first, so no channel is still moving data into a module being reset.
///
/// # Safety
/// Runs once at bring-up, after the module clocks are running and before any driver starts.
#[cfg(any(target_os = "none", test))]
pub(crate) unsafe fn quiesce() {
    unsafe {
        crate::dmac::stop_all();
        crate::scux::reset();
        crate::ssi::reset_all();
        crate::spibsc::restore_read_mode();
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use crate::mmio::test;
    use crate::pac::{CPG, DMAC, SCUX};

    #[test]
    fn quiesce_stops_every_dma_channel_then_resets_the_scux_and_every_ssi() {
        test::reset();
        test::poke8(CPG.swrstcr1().addr() as usize, 0x81);
        unsafe { super::quiesce() };
        let writes = test::writes();
        let at = |addr: usize, val: u32| {
            writes
                .iter()
                .position(|&w| w == (addr, val))
                .unwrap_or_else(|| panic!("no write of {val:#x} to {addr:#x}"))
        };

        // Every channel is stopped (clear-enable, then software reset) before anything else.
        let mut last_dma = 0;
        for ch in 0..16 {
            let chctrl = DMAC.ch(ch).chctrl().addr() as usize;
            let clren = at(chctrl, 1 << 1);
            let swrst = at(chctrl, 1 << 3);
            assert!(clren < swrst, "channel {ch} is reset before it is stopped");
            last_dma = last_dma.max(swrst);
        }

        // The SCUX goes through reset and back to running.
        let swrsr = SCUX.swrsr_cim().addr() as usize;
        let scux_reset = at(swrsr, 0);
        let scux_run = at(swrsr, 1);
        assert!(last_dma < scux_reset && scux_reset < scux_run);

        // Every SSI channel's reset is pulsed, and the other modules' bits are kept.
        let swrstcr1 = CPG.swrstcr1().addr() as usize;
        let ssi_reset = at(swrstcr1, 0xF9);
        let ssi_run = at(swrstcr1, 0x81);
        assert!(scux_run < ssi_reset && ssi_reset < ssi_run);
    }

    #[test]
    fn quiesce_returns_the_flash_controller_to_memory_mapped_reads() {
        test::reset();
        // As the previous program left it: manual SPI mode (CMNCR.MD), the rest of its configuration kept.
        let cmncr = crate::pac::SPIBSC0.cmncr().addr() as usize;
        let drcr = crate::pac::SPIBSC0.drcr().addr() as usize;
        test::poke32(cmncr, 0x81FF_F320);
        unsafe { super::quiesce() };
        let writes = test::writes();
        let manual_off = writes
            .iter()
            .position(|&w| w == (cmncr, 0x01FF_F320))
            .expect("CMNCR.MD is cleared and nothing else in CMNCR changes");
        assert!(
            writes[manual_off..]
                .iter()
                .any(|&(a, v)| a == drcr && v & (1 << 9) != 0),
            "the read cache is flushed after the switch"
        );
    }
}
