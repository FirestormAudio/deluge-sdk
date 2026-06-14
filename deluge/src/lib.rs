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
// The internal PIC service uses `#[embassy_executor::task]`, which needs this.
#![feature(impl_trait_in_assoc_type)]

pub use deluge_macros::app;

mod input;
mod oled;
mod pads;
mod pic_service;
mod sync_led;
pub use input::{Event, Input};
pub use oled::Oled;
pub use pads::{Color, Pads};
pub use sync_led::SyncLed;

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
    /// dlg.spawner().spawn(my_task()).unwrap();
    /// ```
    #[inline]
    pub fn spawner(&self) -> embassy_executor::Spawner {
        self.spawner
    }

    /// Take ownership of the SYNC LED (P6_7).
    ///
    /// ```ignore
    /// let mut led = dlg.sync_led();
    /// led.toggle();
    /// ```
    ///
    /// Takeable once: a second call panics. Owning the returned [`SyncLed`] is
    /// what keeps two places from driving the same pin — no `unsafe`, no shared
    /// globals.
    #[inline]
    pub fn sync_led(&self) -> SyncLed {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::sync_led() called more than once");
        }
        SyncLed::new()
    }

    /// Take ownership of the OLED display, bringing up the PIC service and
    /// initialising the panel.
    ///
    /// ```ignore
    /// let mut oled = dlg.oled().await;
    /// oled.clear();
    /// oled.text(0, 0, "hello deluge");
    /// oled.flush().await;
    /// ```
    ///
    /// `async` because it waits for the PIC handshake and the panel's init
    /// sequence. Takeable once: a second call panics. The returned [`Oled`] is an
    /// `embedded-graphics` `DrawTarget`.
    pub async fn oled(&self) -> Oled {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::oled() called more than once");
        }
        // Bring up the PIC co-processor (UART + RX pump) — the OLED chip-select
        // handshake rides on it — then wait for it to finish configuring.
        pic_service::ensure_started(self.spawner);
        deluge_bsp::pic::wait_ready().await;
        // Run the panel init sequence (SSD1309 over RSPI0, via the bus guard).
        deluge_bsp::oled::init().await;
        Oled::new()
    }

    /// Take the unified input event stream (pads, buttons, encoders).
    ///
    /// ```ignore
    /// let input = dlg.input();
    /// loop {
    ///     match input.next().await {
    ///         Event::Pad { x, y, pressed } => { /* … */ }
    ///         Event::Encoder { index, delta } => { /* … */ }
    ///         _ => {}
    ///     }
    /// }
    /// ```
    ///
    /// Brings up the PIC service (pads/buttons) and the encoder interrupts.
    /// Takeable once: a second call panics.
    pub fn input(&self) -> Input {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::input() called more than once");
        }
        pic_service::ensure_started(self.spawner);
        input::ensure_started(self.spawner);
        Input::new()
    }

    /// Take ownership of the RGB pad grid (18 × 8).
    ///
    /// ```ignore
    /// let mut pads = dlg.pads().await;
    /// pads.set(3, 4, Color::hsv(120, 255, 200));
    /// pads.flush().await;
    /// ```
    ///
    /// `async` because it brings up and waits for the PIC service the pad LEDs
    /// are driven over. Takeable once: a second call panics. Pad coordinates
    /// match [`Event::Pad`].
    pub async fn pads(&self) -> Pads {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::pads() called more than once");
        }
        pic_service::ensure_started(self.spawner);
        deluge_bsp::pic::wait_ready().await;
        Pads::new()
    }
}

/// Convenient glob import for app authors: `use deluge::prelude::*;`.
pub mod prelude {
    pub use crate::app;
    pub use crate::{Color, Deluge, Event, Input, Oled, Pads, SyncLed};
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

    /// Initialise the RTT logger (only with the `rtt` feature).
    ///
    /// Defines the `_SEGGER_RTT` control block (in the `.rtt_buffer` section
    /// provided by the rtt linker script) that the HAL/BSP reference, and
    /// registers the `log` backend. A no-op when `rtt` is disabled.
    #[cfg(feature = "rtt")]
    fn init_logging() {
        let channels = rtt_target::rtt_init! {
            up: {
                0: { size: 16384, name: "Terminal", section: ".rtt_buffer" }
            }
            section_cb: ".rtt_buffer"
        };
        rtt_target::set_print_channel(channels.up.0);
        rtt_target::init_logger_with_level(log::LevelFilter::Debug);
    }

    /// Bring up the platform short of enabling interrupts: SRAM/SDRAM heaps and
    /// clocks/MMU/caches/SDRAM/GIC/OSTM time driver.
    ///
    /// Interrupts are enabled separately, *after* the app's `setup` phase, so
    /// drivers whose init must run with IRQs masked (e.g. GIC source setup) keep
    /// working — see [`run`].
    ///
    /// # Safety
    /// Must run exactly once, at startup, before any allocation.
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
        }
    }

    static mut EXECUTOR: MaybeUninit<Executor> = MaybeUninit::uninit();

    /// The `#[deluge::app]` entry point.
    ///
    /// Sequence: logging → heaps + clocks → `setup()` (app's synchronous,
    /// interrupts-masked init) → enable interrupts → run the Embassy executor,
    /// invoking `spawn` once with the [`Spawner`].
    ///
    /// `setup` runs with IRQs still masked so peripheral/GIC bring-up that
    /// requires it (per some HAL drivers' contracts) is safe; it is empty for
    /// apps that don't opt into `#[deluge::app(setup = …)]`.
    pub fn run(setup: impl FnOnce(), spawn: impl FnOnce(Spawner)) -> ! {
        #[cfg(feature = "rtt")]
        init_logging();

        unsafe { init_platform() };

        // App's synchronous, interrupts-masked initialisation.
        setup();

        // Unmask IRQs so the Embassy time driver and peripheral ISRs fire.
        unsafe { cortex_ar::interrupt::enable() };

        #[allow(static_mut_refs)]
        let executor: &'static mut Executor = unsafe {
            EXECUTOR.write(Executor::new());
            EXECUTOR.assume_init_mut()
        };
        executor.run(spawn)
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
