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

mod audio;
mod clock;
mod cv_gate;
mod input;
mod jacks;
mod leds;
mod midi;
mod oled;
mod pads;
mod pic_service;
mod sd;
mod sync_led;
#[cfg(feature = "usb-log")]
mod usb_debug;
pub use audio::{Audio, StereoFrame};
pub use clock::{ClockIn, ClockOut};
pub use cv_gate::{Cv, Gate};
pub use input::{Event, Input};
pub use jacks::Jacks;
pub use leds::Leds;
pub use midi::Midi;
pub use oled::Oled;
pub use pads::{Color, Pads};
pub use sd::Sd;
pub use sync_led::SyncLed;

/// Named button / encoder / knob ids — match them against [`Event`], e.g.
/// `Event::Button { id, .. } if id == controls::button::PLAY`.
pub use deluge_bsp::controls;

/// Filesystem error from [`Sd`] read/write operations.
pub use deluge_bsp::fat::FatError;
/// SD-card hardware error from [`Deluge::sd`].
pub use deluge_bsp::sd::SdError;

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

    /// Take the button/indicator LEDs and gold-knob columns. Takeable once.
    ///
    /// ```ignore
    /// let mut leds = dlg.leds().await;
    /// leds.on(controls::button::PLAY).await;
    /// ```
    ///
    /// `async` (brings up + waits for the PIC the LEDs ride on). See [`Leds`].
    pub async fn leds(&self) -> Leds {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::leds() called more than once");
        }
        pic_service::ensure_started(self.spawner);
        deluge_bsp::pic::wait_ready().await;
        Leds::new()
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

    /// Take the CV (control-voltage) outputs. Takeable once.
    ///
    /// Best acquired before entering the main loop (its one-time DAC bring-up
    /// reconfigures RSPI0). See [`Cv`].
    pub fn cv(&self) -> Cv {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::cv() called more than once");
        }
        Cv::new()
    }

    /// Take the gate outputs. Takeable once. See [`Gate`].
    pub fn gate(&self) -> Gate {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::gate() called more than once");
        }
        Gate::new()
    }

    /// Take the analog trigger-clock **input** jack. Takeable once.
    ///
    /// ```ignore
    /// let mut clk = dlg.clock_in();
    /// loop {
    ///     if let Some(dt) = clk.tick().await {
    ///         info!("clock interval: {} ms", dt.as_millis());
    ///     }
    /// }
    /// ```
    ///
    /// Registers the trigger-clock interrupt on first use. See [`ClockIn`].
    pub fn clock_in(&self) -> ClockIn {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::clock_in() called more than once");
        }
        ClockIn::new()
    }

    /// Take a software clock **output** on gate channel `gate_ch`. Takeable once.
    ///
    /// There is no dedicated clock-out jack: this pulses one of the V-trig gate
    /// outputs, so don't also drive `gate_ch` through [`gate`](Deluge::gate).
    ///
    /// ```ignore
    /// let mut clk = dlg.clock_out(0);
    /// clk.run(ClockOut::period_from_bpm(120.0, 24)).await
    /// ```
    ///
    /// See [`ClockOut`].
    pub fn clock_out(&self, gate_ch: u8) -> ClockOut {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::clock_out() called more than once");
        }
        ClockOut::new(gate_ch)
    }

    /// Take the audio jack-detect inputs + speaker-amplifier control. Takeable
    /// once.
    ///
    /// ```ignore
    /// let mut jacks = dlg.jacks();
    /// if jacks.headphone() { /* … */ }
    /// jacks.apply_speaker_mute(); // standard mute policy
    /// ```
    ///
    /// See [`Jacks`].
    pub fn jacks(&self) -> Jacks {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::jacks() called more than once");
        }
        Jacks::new()
    }

    /// Take the DIN MIDI port. Takeable once. See [`Midi`].
    pub fn midi(&self) -> Midi {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::midi() called more than once");
        }
        Midi::new()
    }

    /// Initialise the SD card and take the filesystem handle.
    ///
    /// ```ignore
    /// let mut sd = dlg.sd().await?;
    /// let mut buf = [0u8; 64];
    /// let n = sd.read("CONFIG.TXT", &mut buf)?;
    /// ```
    ///
    /// `async` (the card init does I/O); returns `Err` if no card is present or
    /// init fails. Takeable once: a second call panics.
    pub async fn sd(&self) -> Result<Sd, SdError> {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::sd() called more than once");
        }
        deluge_bsp::sd::init().await?;
        Ok(Sd::new())
    }

    /// Take the codec audio path for per-block DSP. Takeable once.
    ///
    /// ```ignore
    /// dlg.audio().process(|block| {
    ///     for f in block { f.l *= 0.5; f.r *= 0.5; }
    /// }).await
    /// ```
    ///
    /// Owns the codec — don't also run a USB audio (UAC2) device stack. Acquire
    /// before the main loop (its one-time bring-up blocks ~5 ms). See [`Audio`].
    pub fn audio(&self) -> Audio {
        use core::sync::atomic::{AtomicBool, Ordering};
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            panic!("Deluge::audio() called more than once");
        }
        Audio::new()
    }
}

/// Convenient glob import for app authors: `use deluge::prelude::*;`.
pub mod prelude {
    pub use crate::app;
    pub use crate::controls;
    pub use crate::{
        Audio, ClockIn, ClockOut, Color, Cv, Deluge, Event, Gate, Input, Jacks, Leds, Midi, Oled,
        Pads, Sd, StereoFrame, SyncLed,
    };
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

    /// Initialise the RTT logger (only with the `rtt` feature, and only when
    /// `usb-log` is not also enabled — there can be just one global logger).
    ///
    /// Defines the `_SEGGER_RTT` control block (in the `.rtt_buffer` section
    /// provided by the rtt linker script) that the HAL/BSP reference, and
    /// registers the `log` backend.
    #[cfg(all(feature = "rtt", not(feature = "usb-log")))]
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
        // Pick the logger: `usb-log` takes precedence over `rtt` (one logger).
        #[cfg(feature = "usb-log")]
        crate::usb_debug::init_logger();
        #[cfg(all(feature = "rtt", not(feature = "usb-log")))]
        init_logging();

        unsafe { init_platform() };

        // App's synchronous, interrupts-masked initialisation.
        setup();

        // Bring up the USB-debug device (registers the USB0 ISR and starts the
        // controller) while interrupts are still masked — matches the proven
        // controller-firmware ordering. Spawned below once the executor runs.
        #[cfg(feature = "usb-log")]
        let usb = unsafe { crate::usb_debug::build() };

        // Unmask IRQs so the Embassy time driver and peripheral ISRs fire.
        unsafe { cortex_ar::interrupt::enable() };

        #[allow(static_mut_refs)]
        let executor: &'static mut Executor = unsafe {
            EXECUTOR.write(Executor::new());
            EXECUTOR.assume_init_mut()
        };
        executor.run(move |spawner| {
            #[cfg(feature = "usb-log")]
            crate::usb_debug::spawn(spawner, usb);
            spawn(spawner);
        })
    }

    /// Default panic behaviour: stop the world, show it, keep signalling.
    ///
    /// Masks interrupts, logs via RTT (if enabled), draws `APP PANIC` + the panic
    /// location to the OLED (best-effort, via the blocking panic path — no-op if
    /// the OLED was never brought up), then strobes the SYNC LED forever so a
    /// probe-less user sees the crash. See `docs/deluge-sdk.md` §9.
    pub fn panic(info: &core::panic::PanicInfo) -> ! {
        // Stop the world: no more ISRs or task switches.
        cortex_ar::interrupt::disable();

        log::error!("PANIC: {}", info);

        // Best-effort OLED message.
        let mut fb = deluge_bsp::oled::FrameBuffer::new();
        deluge_bsp::oled::text::draw_str(&mut fb, 0, 0, b"APP PANIC");
        if let Some(loc) = info.location() {
            use core::fmt::Write;
            let mut line = LineBuf::new();
            let _ = write!(line, "{}:{}", basename(loc.file()), loc.line());
            deluge_bsp::oled::text::draw_str(&mut fb, 0, 10, line.as_bytes());
        }
        // SAFETY: interrupts are masked and we are single-threaded here.
        unsafe { deluge_bsp::oled::draw_blocking(&fb) };

        // Always-visible fallback: strobe the SYNC LED forever.
        unsafe { rza1l_hal::gpio::set_as_output(6, 7) };
        loop {
            unsafe {
                rza1l_hal::gpio::write(6, 7, true);
                rza1l_hal::ostm::delay_ms(100);
                rza1l_hal::gpio::write(6, 7, false);
                rza1l_hal::ostm::delay_ms(100);
            }
        }
    }

    /// Last path component of `path` (so the OLED shows `main.rs`, not the full
    /// crate path).
    fn basename(path: &str) -> &str {
        match path.rsplit_once(['/', '\\']) {
            Some((_, name)) => name,
            None => path,
        }
    }

    /// A tiny fixed-width `core::fmt::Write` sink for one OLED text line
    /// (128 px / 6 px per glyph ≈ 21 chars). Excess is dropped.
    struct LineBuf {
        buf: [u8; 21],
        len: usize,
    }

    impl LineBuf {
        fn new() -> Self {
            Self {
                buf: [0; 21],
                len: 0,
            }
        }
        fn as_bytes(&self) -> &[u8] {
            &self.buf[..self.len]
        }
    }

    impl core::fmt::Write for LineBuf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if self.len < self.buf.len() {
                    self.buf[self.len] = b;
                    self.len += 1;
                }
            }
            Ok(())
        }
    }
}
