//! # Deluge SDK
//!
//! A user-friendly SDK for building apps that run on the Synthstrom Deluge
//! (Renesas RZ/A1L). It wraps the board support package ([`deluge_bsp`]) and the
//! HAL ([`rza1l_hal`]) behind a single dependency, and provides the
//! [`#[deluge::app]`](macro@app) attribute that absorbs all of the platform
//! bring-up boilerplate.
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! #![feature(impl_trait_in_assoc_type)]
//! use deluge::prelude::*;
//! use embassy_time::Timer;
//!
//! #[deluge::app]
//! async fn main(_dlg: Deluge) {
//!     loop {
//!         // the heaps, clocks, interrupts and executor are already up.
//!         Timer::after_millis(200).await;
//!     }
//! }
//! ```
//!
//! See `docs/deluge-sdk.md` for the design.

#![no_std]

pub use deluge_macros::app;

// Re-export the underlying layers so apps can reach lower-level functionality
// through the single `deluge` dependency while the capability API (M2+) grows.
pub use deluge_bsp;
pub use rza1l_hal;

/// The app capability handle, passed to `#[deluge::app] async fn main`.
///
/// For now it carries the Embassy [`Spawner`](embassy_executor::Spawner) so an
/// app can spawn its own background tasks. The ergonomic capability accessors
/// (`oled()`, `input()`, `pads()`, …) land in later milestones; see
/// `docs/deluge-sdk.md` §6.
pub struct Deluge {
    spawner: embassy_executor::Spawner,
}

impl Deluge {
    /// Construct the handle. Called by the `#[deluge::app]` expansion; not part
    /// of the public API.
    #[doc(hidden)]
    #[inline]
    pub fn __new(spawner: embassy_executor::Spawner) -> Self {
        Self { spawner }
    }

    /// The Embassy task spawner for the app's executor.
    ///
    /// Use it to launch your own background tasks:
    /// ```ignore
    /// dlg.spawner().must_spawn(my_task());
    /// ```
    #[inline]
    pub fn spawner(&self) -> embassy_executor::Spawner {
        self.spawner
    }
}

/// Convenient glob import for app authors: `use deluge::prelude::*;`.
pub mod prelude {
    pub use crate::Deluge;
    pub use crate::app;
    pub use log::{debug, error, info, warn};
}

/// Runtime support invoked by the [`#[deluge::app]`](macro@app) expansion.
///
/// Not a stable API — these items exist so the generated entry point stays a
/// handful of tokens. Apps should not call them directly.
#[doc(hidden)]
pub mod __rt {
    pub use embassy_executor::Spawner;

    use core::mem::MaybeUninit;
    use embassy_executor::Executor;

    unsafe extern "C" {
        /// Start of the free SRAM heap region (set by the linker script).
        static __sram_heap_start: u8;
        /// End of the free SRAM heap region (start of RTT/stack reservation).
        static __sram_heap_end: u8;
    }

    /// Base address of the 64 MB external SDRAM window.
    const SDRAM_BASE: usize = 0x0C00_0000;
    /// Size of the external SDRAM window in bytes.
    const SDRAM_SIZE: usize = 64 * 1024 * 1024;

    /// Bring up the platform: SRAM/SDRAM heaps, clocks/MMU/caches/GIC/time
    /// driver, and global interrupt enable.
    ///
    /// # Safety
    /// Must run exactly once, at startup, before any allocation or task.
    unsafe fn init_platform() {
        unsafe {
            // SRAM heap (internal RAM) — initialise before any allocation.
            let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
            let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
            rza1l_hal::allocator::SRAM.init(start, size);

            // Module clocks, MMU, caches, SDRAM controller, GIC, OSTM time driver.
            deluge_bsp::system::init_clocks();

            // SDRAM heap — now that the SDRAM window is accessible.
            rza1l_hal::allocator::SDRAM.init(SDRAM_BASE as *mut u8, SDRAM_SIZE);

            // Unmask IRQs so the Embassy time driver and peripheral ISRs fire.
            cortex_ar::interrupt::enable();
        }
    }

    static mut EXECUTOR: MaybeUninit<Executor> = MaybeUninit::uninit();

    /// Initialise the platform, then run the Embassy executor forever, invoking
    /// `init` once with the [`Spawner`] so the app's main task can be spawned.
    pub fn run(init: impl FnOnce(Spawner)) -> ! {
        unsafe { init_platform() };

        #[allow(static_mut_refs)]
        let executor: &'static mut Executor = unsafe {
            EXECUTOR.write(Executor::new());
            EXECUTOR.assume_init_mut()
        };
        executor.run(init)
    }

    /// Default panic behaviour: log and halt.
    ///
    /// A visible (OLED) panic screen + reboot-to-loader is a later milestone
    /// (`docs/deluge-sdk.md` §9); for now this matches the firmwares' handler.
    pub fn panic(info: &core::panic::PanicInfo) -> ! {
        log::error!("PANIC: {}", info);
        loop {
            core::hint::spin_loop();
        }
    }
}
