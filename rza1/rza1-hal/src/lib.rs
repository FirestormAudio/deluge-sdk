//! `rza1-hal` — Embassy HAL for the Renesas RZ/A1 (Cortex-A9): the RZ/A1L (`rza1l` feature) and
//! the RZ/A1H (`rza1h`). Enable exactly one chip feature.
//!
//! Peripheral base addresses are taken from the RZ/A1L Hardware Manual.

#![cfg_attr(target_os = "none", no_std)]
#![allow(dead_code)]

// Logging macros (`trace!` … `error!`) over `log`, `defmt` or nothing: first, so every module sees
// them.
mod fmt;

#[cfg(not(any(feature = "rza1l", feature = "rza1h")))]
compile_error!("rza1-hal: enable exactly one chip feature: rza1l or rza1h");
#[cfg(all(feature = "rza1l", feature = "rza1h"))]
compile_error!("rza1-hal: enable exactly one chip feature: rza1l or rza1h (both are enabled)");

/// Offset from a cached address to its uncached mirror alias.
///
/// The RZ/A1L memory map provides uncached mirrors of the SRAM and SDRAM
/// regions offset by 0x4000_0000.  For example, cached SRAM at 0x2002_0000
/// has an uncached alias at 0x6002_0000.  Using the uncached alias for
/// DMA-shared buffers avoids stale cache lines without requiring explicit
/// cache maintenance operations.
pub const UNCACHED_MIRROR_OFFSET: usize = 0x4000_0000;

/// `p`'s uncached alias: write DMA-shared memory through it so the DMAC sees the bytes without a
/// cache clean, and hand it to the DMAC as the bus address. On the host, where tests run, memory
/// has no alias, so it is `p`.
#[inline]
pub(crate) fn uncached<T>(p: *mut T) -> *mut T {
    #[cfg(target_os = "none")]
    return (p as usize + UNCACHED_MIRROR_OFFSET) as *mut T;
    #[cfg(not(target_os = "none"))]
    return p;
}

pub use embassy_hal_internal::{Peri, PeripheralType};
/// The chip's registers, generated from the vendor SVD ([`rza1_pac`], built with the chip feature
/// this crate is built with). The RZ/A1L's are checked against its manual. Behind the
/// `unstable-pac` feature: the PAC's API is not covered by this crate's semver.
#[cfg(feature = "unstable-pac")]
pub use rza1_pac as pac;
#[cfg(not(feature = "unstable-pac"))]
pub(crate) use rza1_pac as pac;
/// The MMIO seam every register access goes through (real volatile on the device, a shadow with
/// an access log under host tests).
pub use rza1_pac::mmio;

embassy_hal_internal::peripherals! {
    // With `time-driver-ostm`, the time driver owns OSTM0 and OSTM1.
    #[cfg(not(feature = "time-driver-ostm"))] OSTM0,
    #[cfg(not(feature = "time-driver-ostm"))] OSTM1,
    RSPI0, RSPI1, RSPI2,
    #[cfg(feature = "rza1h")] RSPI3,
    #[cfg(feature = "rza1h")] RSPI4,
    SCIF0, SCIF1, SCIF2, SCIF3, SCIF4,
    SSIF0, SSIF1, SSIF2, SSIF3,
    #[cfg(feature = "rza1l")] SDHI0,
    #[cfg(feature = "rza1l")] SDHI1,
    USB0, USB1,
    DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, DMA_CH4, DMA_CH5, DMA_CH6, DMA_CH7,
    DMA_CH8, DMA_CH9, DMA_CH10, DMA_CH11, DMA_CH12, DMA_CH13, DMA_CH14, DMA_CH15,
    MTU2_0, MTU2_1, MTU2_2, MTU2_3, MTU2_4,
    SPIBSC0, SCUX, ADC,
}

/// Bring up the chip and hand out its peripherals: record the clock tree, program the CPG,
/// start the module clocks, put back what a warm start left running (the `warm_start` module),
/// initialise the GIC and, with `time-driver-ostm`, start the time driver, in that order.
///
/// Device-only: the GIC bring-up writes real registers, which a host build doesn't have.
///
/// # Panics
/// If called more than once (the peripherals are taken before any register is written).
#[cfg(target_os = "none")]
pub fn init(config: Config) -> Peripherals {
    // Take first: a second call must panic here, before it reprograms any clock or the GIC.
    let peripherals = Peripherals::take();
    #[cfg(feature = "time-driver-ostm")]
    let start_time_driver = time_driver::start;
    #[cfg(not(feature = "time-driver-ostm"))]
    let start_time_driver = |_| {};
    // SAFETY: `take` succeeded, so this is the one bring-up.
    unsafe { bring_up(&config, start_time_driver) };
    peripherals
}

/// [`init`]'s bring-up, with the time driver's start passed in so the order is host-testable
/// (the driver itself is device-only).
///
/// # Safety
/// Reprograms the clocks, the GIC and every module clock: once, at boot.
#[cfg(any(target_os = "none", test))]
unsafe fn bring_up(config: &Config, start_time_driver: impl FnOnce(interrupt::Priority)) {
    unsafe {
        clocks::init(config.clocks);
        cpg_then_stb(config.cpg.frqcr(), &config.module_clocks.stbcr_all());
        dmac::set_interrupt_priority(config.dma_interrupt_priority);
        warm_start::quiesce();
        gic::init();
    }
    start_time_driver(config.time_interrupt_priority);
}

/// CPG, then the module clocks: a module clock must not start before the bus clocks it runs from
/// are programmed.
unsafe fn cpg_then_stb(frqcr: u16, stbcr: &[u8; 11]) {
    unsafe {
        cpg::init(frqcr);
        stb::init(stbcr);
    }
}

pub mod adc;
// mmu and cache use ARM coprocessor instructions and cannot compile on the host. The reset
// vector and exception entry live in the `rza1-rt` crate.
pub mod bsc;
// Host tests run on 32-bit ARM Linux (QEMU), where the coprocessor instructions assemble; only
// the PL310 register paths run there.
#[cfg(any(target_os = "none", all(test, target_arch = "arm")))]
pub mod cache;
/// The clock tree, derived from the board's crystal (set once at boot by the BSP).
pub mod clocks;
mod config;
pub use config::{ClockOutput, ClockOutputOnChange, Config, CpgConfig, CpuClockDivider, ModuleClock, ModuleClocks};
mod cpg;
pub mod dmac;
pub mod gic;
pub mod interrupt;
/// Pure memory-system math (cache-line ranges + MMU section table) used by the
/// bare-metal-only `cache`/`mmu` modules; non-gated so it unit-tests on host.
pub mod memmap;
#[cfg(target_os = "none")]
pub mod mmu;
pub mod mtu2;
pub mod ostm;
pub mod rspi;
pub mod scux;
// The RZ/A1H's PAC has no SDHI block.
#[cfg(feature = "rza1l")]
pub mod sdhi;
pub mod spibsc;
pub mod ssi;
mod stb;
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
mod warm_start;

pub mod gpio;

#[cfg(all(test, not(target_os = "none")))]
mod init_tests {
    use super::*;
    use crate::mmio::test;

    /// STBCR2–STBCR12 as the test writes them: arbitrary, so the order check can't pass by luck.
    const STB: [u8; 11] = [0x6A, 0x80, 0x00, 0x00, 0x00, 0x24, 0x05, 0x00, 0x00, 0xDF, 0xFB];

    #[test]
    fn the_pll_is_programmed_before_any_module_clock() {
        test::reset();
        unsafe { stb::init(&STB) };
        let stb_writes = test::writes();

        test::reset();
        let cpg = CpgConfig::new()
            .with_cpu_clock(CpuClockDivider::Div1)
            .with_clock_output(ClockOutput::LowInStandby);
        unsafe { cpg_then_stb(cpg.frqcr(), &STB) };
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
    fn init_starts_the_time_driver_after_the_gic() {
        test::reset();
        let config = Config::new(clocks::ClockConfig::new(13_225_625))
            .with_time_interrupt_priority(interrupt::Priority::P14);
        let mut started = None;
        unsafe {
            bring_up(&config, |priority| {
                // The GIC is up by now: its priority words hold the default.
                let last_ipr = 0xE820_1400 + (gic::INT_ID_TOTAL / 4) * 4;
                assert!(
                    test::writes().iter().any(|&(a, _)| a == last_ipr),
                    "the GIC is initialised before the time driver starts"
                );
                started = Some(priority);
            })
        };
        assert_eq!(started, Some(interrupt::Priority::P14));
    }

    #[test]
    #[should_panic(expected = "init called more than once!")]
    fn peripherals_can_be_taken_once() {
        let _first = Peripherals::take();
        let _second = Peripherals::take();
    }
}
