//! `rza1l-hal` — Embassy HAL for the Renesas RZ/A1L (Cortex-A9, R7S721001).
//!
//! Peripheral base addresses are taken from the RZ/A1L Hardware Manual.

#![cfg_attr(target_os = "none", no_std)]
#![allow(dead_code)]

/// Offset from a cached address to its uncached mirror alias.
///
/// The RZ/A1L memory map provides uncached mirrors of the SRAM and SDRAM
/// regions offset by 0x4000_0000.  For example, cached SRAM at 0x2002_0000
/// has an uncached alias at 0x6002_0000.  Using the uncached alias for
/// DMA-shared buffers avoids stale cache lines without requiring explicit
/// cache maintenance operations.
pub const UNCACHED_MIRROR_OFFSET: usize = 0x4000_0000;

pub use embassy_hal_internal::{Peri, PeripheralType};

embassy_hal_internal::peripherals! {
    OSTM0, OSTM1,
    RSPI0, RSPI1, RSPI2, RSPI3, RSPI4,
    SCIF0, SCIF1, SCIF2, SCIF3, SCIF4,
    SSIF0, SSIF1, SSIF2, SSIF3,
    SDHI0, SDHI1,
    USB0, USB1,
    DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, DMA_CH4, DMA_CH5, DMA_CH6, DMA_CH7,
    DMA_CH8, DMA_CH9, DMA_CH10, DMA_CH11, DMA_CH12, DMA_CH13, DMA_CH14, DMA_CH15,
    MTU2_0, MTU2_1, MTU2_2, MTU2_3, MTU2_4,
    SPIBSC0, SCUX, ADC,
}

/// The board's chip configuration for [`init`].
pub struct Config {
    /// The board's crystal, from which every peripheral clock is derived.
    pub clocks: clocks::ClockConfig,
    /// The CPG frequency-control (FRQCR) value: PLL multiplier and divider ratios.
    pub frqcr: u16,
    /// Which module clocks to run (STBCR2–STBCR12).
    pub stb: stb::StbConfig,
}

/// Bring up the chip and hand out its peripherals: record the clock tree, program the CPG,
/// start the module clocks, and initialise the GIC, in that order.
///
/// Device-only: the GIC bring-up writes real registers, which a host build doesn't have.
///
/// # Panics
/// If called more than once (the peripherals are taken before any register is written).
#[cfg(target_os = "none")]
pub fn init(config: Config) -> Peripherals {
    // Take first: a second call must panic here, before it reprograms any clock or the GIC.
    let peripherals = Peripherals::take();
    // SAFETY: `take` succeeded, so this is the one bring-up.
    unsafe {
        clocks::init(config.clocks);
        cpg_then_stb(config.frqcr, &config.stb);
        gic::init();
    }
    peripherals
}

/// CPG, then the module clocks: a module clock must not start before the bus clocks it runs from
/// are programmed.
unsafe fn cpg_then_stb(frqcr: u16, stb: &stb::StbConfig) {
    unsafe {
        cpg::init(frqcr);
        stb::init(stb);
    }
}

pub mod adc;
// mmu and cache use ARM coprocessor instructions and cannot compile on the host. The reset
// vector and exception entry live in the `rza1-rt` crate.
pub mod bsc;
#[cfg(target_os = "none")]
pub mod cache;
/// The clock tree, derived from the board's crystal (set once at boot by the BSP).
pub mod clocks;
mod cpg;
pub mod dmac;
pub mod gic;
pub mod interrupt;
/// Pure memory-system math (cache-line ranges + MMU section table) used by the
/// bare-metal-only `cache`/`mmu` modules; non-gated so it unit-tests on host.
pub mod memmap;
/// MMIO seam: real volatile access on firmware, a shadow + access log under
/// test, so register write *sequences* can be asserted on the host.
pub mod mmio;
#[cfg(target_os = "none")]
pub mod mmu;
pub mod mtu2;
pub mod ostm;
pub mod rspi;
pub mod scux;
pub mod sdhi;
pub mod spibsc;
pub mod ssi;
pub mod stb;
// The OSTM-backed embassy time driver does raw MMIO and registers the global
// `_embassy_time_now`/`_embassy_time_schedule_wake` symbols. On host the
// simulator uses embassy-time's `std` wall-clock driver instead, so keep this
// device-only to avoid a faulting driver and a duplicate-symbol link error. It is behind the
// `time-driver-ostm` feature (on by default) so an application can register its own driver instead.
#[cfg(all(target_os = "none", feature = "time-driver-ostm"))]
pub mod time_driver;
/// Pure arithmetic for the time driver — always compiled so its tests run on the host.
pub mod time_math;
pub mod uart;
pub mod usb;

pub mod gpio;

#[cfg(all(test, not(target_os = "none")))]
mod init_tests {
    use super::*;
    use crate::mmio::test;

    const STB: stb::StbConfig = stb::StbConfig {
        stbcr2: 0x6A,
        stbcr3: 0x80,
        stbcr4: 0x00,
        stbcr5: 0x00,
        stbcr6: 0x00,
        stbcr7: 0x24,
        stbcr8: 0x05,
        stbcr9: 0x00,
        stbcr10: 0x00,
        stbcr11: 0xDF,
        stbcr12: 0xFB,
    };

    #[test]
    fn the_pll_is_programmed_before_any_module_clock() {
        test::reset();
        unsafe { stb::init(&STB) };
        let stb_writes = test::writes();

        test::reset();
        unsafe { cpg_then_stb(0x1035, &STB) };
        let writes = test::writes();

        // PL310 power control, then FRQCR, then SYSCR3 (retention-RAM write enable), exactly
        // as the BSP's CPG init wrote them before it moved here; then the module clocks, unchanged.
        assert_eq!(
            writes[..3],
            [(0x3FFF_FF80, 1), (0xFCFE_0010, 0x1035), (0xFCFE_0408, 0x0F)]
        );
        assert_eq!(writes[3..], stb_writes[..]);
    }

    #[test]
    #[should_panic(expected = "init called more than once!")]
    fn peripherals_can_be_taken_once() {
        let _first = Peripherals::take();
        let _second = Peripherals::take();
    }
}
