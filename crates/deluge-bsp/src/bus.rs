//! Shared-bus arbitration (RSPI0 today; PIC transport to follow).
//!
//! RSPI0 is physically shared between two consumers that need *mutually
//! incompatible* frame modes:
//!
//! * the **OLED** (`oled`) — 8-bit frames streamed via DMAC channel 4, and
//! * the **CV DAC** (`cv_gate`) — 32-bit blocking writes to a MAX5136.
//!
//! Driving either without coordinating corrupts the other's transfer, and
//! switching frame mode underneath an in-flight transfer is silently wrong.
//! Until now this was guarded by a hand-rolled `RSPI0_DMA_ACTIVE` spin-flag that
//! every CV write had to *remember* to poll — a forgettable lock, and one that
//! never reconfigured the frame mode (so a CV write after an OLED frame ran in
//! 8-bit mode by accident; it only worked because no firmware interleaved them).
//!
//! This module replaces that with a single owned resource behind an async-aware
//! mutex. Consumers `lock_rspi0().await`, drive the bus through the returned
//! guard, and release it by dropping the guard. The guard may be held across
//! `.await` (e.g. while an OLED DMA transfer completes); other consumers simply
//! wait. The guard also *tracks the frame mode*, so `enter_8bit` / `enter_32bit`
//! reconfigure only when the mode actually changes — arbitration and mode
//! correctness become unforgettable instead of documented.
//!
//! See the Advanced developer guide (`docs/advanced-guide.md`, §7 — *Dropping
//! down to the BSP & HAL*) for the RSPI0 arbitration design.

#![cfg(target_os = "none")]

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::{Mutex, MutexGuard};

use rza1l_hal::rspi;

/// RSPI channel physically shared by the OLED and the CV DAC.
const RSPI0_CH: u8 = 0;

/// RSPI0 frame mode, tracked by the bus owner so consumers don't reconfigure
/// the channel blindly on every transfer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Mode not yet established since boot — the next `enter_*` always
    /// reconfigures.
    Unknown,
    /// 8-bit frames (OLED).
    Bits8,
    /// 32-bit frames (CV DAC).
    Bits32,
}

/// Exclusive-access token for RSPI0.
///
/// The only way to obtain one is [`lock_rspi0`] / [`try_lock_rspi0`] (or, in a
/// dying context, [`steal_rspi0`]). Holding it guarantees no other consumer can
/// touch RSPI0.
pub struct Rspi0 {
    mode: Mode,
}

impl Rspi0 {
    /// Ensure RSPI0 is in 8-bit frame mode (OLED). Reconfigures only if the
    /// channel is not already in 8-bit mode.
    #[inline]
    pub fn enter_8bit(&mut self) {
        if self.mode != Mode::Bits8 {
            // SAFETY: we hold the only token, so no concurrent RSPI0 transfer.
            unsafe { rspi::configure_8bit(RSPI0_CH) };
            self.mode = Mode::Bits8;
        }
    }

    /// Ensure RSPI0 is in 32-bit frame mode (CV DAC). Reconfigures only if the
    /// channel is not already in 32-bit mode.
    #[inline]
    pub fn enter_32bit(&mut self) {
        if self.mode != Mode::Bits32 {
            // SAFETY: we hold the only token, so no concurrent RSPI0 transfer.
            unsafe { rspi::configure_32bit(RSPI0_CH) };
            self.mode = Mode::Bits32;
        }
    }

    /// Send one byte in 8-bit mode. Call [`enter_8bit`](Self::enter_8bit) first.
    #[inline]
    pub fn send8(&mut self, byte: u8) {
        // SAFETY: token-guarded exclusive access; mode is the caller's contract.
        unsafe { rspi::send8(RSPI0_CH, byte) };
    }

    /// Blocking 32-bit send. Call [`enter_32bit`](Self::enter_32bit) first.
    #[inline]
    pub fn send32_blocking(&mut self, word: u32) {
        // SAFETY: token-guarded exclusive access; mode is the caller's contract.
        unsafe { rspi::send32_blocking(RSPI0_CH, word) };
    }

    /// Poll until the shift register has drained (TEND=1). Call before
    /// de-asserting chip-select so the peripheral receives the full frame.
    #[inline]
    pub fn wait_end(&mut self) {
        // SAFETY: token-guarded exclusive access.
        unsafe { rspi::wait_end(RSPI0_CH) };
    }
}

/// The one and only RSPI0 token, behind an async mutex.
///
/// `CriticalSectionRawMutex` (not an async-only raw mutex) is deliberate: it
/// lets [`steal_rspi0`] and [`try_lock_rspi0`] work outside the executor (boot,
/// and a future panic handler).
static RSPI0: Mutex<CriticalSectionRawMutex, Rspi0> = Mutex::new(Rspi0 {
    mode: Mode::Unknown,
});

/// Type of the guard returned by [`lock_rspi0`].
pub type Rspi0Guard = MutexGuard<'static, CriticalSectionRawMutex, Rspi0>;

/// Acquire exclusive RSPI0 access, awaiting if another consumer holds it.
///
/// The guard may be held across `.await` (e.g. across an OLED DMA completion).
pub async fn lock_rspi0() -> Rspi0Guard {
    RSPI0.lock().await
}

/// Try to acquire RSPI0 without awaiting; returns `None` if it is already held.
///
/// Intended for single-threaded startup (`cv_gate::init`) before the executor
/// runs, where contention is impossible and `.await` is unavailable.
pub fn try_lock_rspi0() -> Option<Rspi0Guard> {
    RSPI0.try_lock().ok()
}

/// Fabricate an RSPI0 token, **bypassing the mutex**.
///
/// For use only when the normal locking discipline cannot apply — i.e. a panic
/// handler that must draw to the OLED but cannot `.await` and may run with the
/// executor dead. Safe there because panic is single-threaded with interrupts
/// masked. Never call this from normal code.
///
/// # Safety
/// The caller must guarantee no other RSPI0 consumer is or will be active for
/// the lifetime of the returned token (e.g. inside a panic handler with IRQs
/// masked).
pub unsafe fn steal_rspi0() -> Rspi0 {
    Rspi0 {
        mode: Mode::Unknown,
    }
}
