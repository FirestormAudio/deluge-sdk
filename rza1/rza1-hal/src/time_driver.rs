//! Embassy time driver for the RZ/A1L, backed by OSTM0 and OSTM1.
//!
//! ## Hardware layout
//!
//! | Channel | Mode                         | Role                         |
//! |---------|------------------------------|------------------------------|
//! | OSTM0   | Free-running + CMP interrupt | 32-bit monotonic clock base  |
//! | OSTM1   | Interval + interrupt         | One-shot alarm               |
//!
//! ## Clock and tick rate
//!
//! OSTM0 runs at P0φ (see [`crate::clocks`]). The Embassy tick rate is 1 MHz (1 µs per tick);
//! `now()` converts OSTM0 ticks with the exact ticks-per-µs rational from
//! [`crate::time_math::TickRatio`] (21161/640 on the Deluge).
//!
//! ## Overflow handling for `now()`
//!
//! OSTM0 is 32-bit and wraps every ~129.9 s. The 64-bit clock is extended **in software**:
//! `raw_ostm_ticks` compares each CNT sample against the last published one and treats CNT going
//! backwards as a wrap, so the extension is monotonic by construction and no interrupt is on the
//! correctness path (see `LAST_RAW`). The wrap arithmetic lives in [`crate::time_math`], where
//! it is host-tested.
//!
//! ## Alarm
//!
//! `schedule_wake(at, waker)` adds the waker to a software queue and, if it
//! is the earliest pending wake-up, programs OSTM1 in interval mode for
//! `at - now()` ticks. The OSTM1 ISR calls `Queue::handle_alarm`, which
//! wakes all expired tasks and returns the next expiration time; we re-arm
//! for that if needed.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::Waker;

use critical_section::{CriticalSection, Mutex};
use embassy_time_driver::Driver;

use crate::time_math::{AtomicTickRatio, TickRatio, extend};
use embassy_time_queue_utils::Queue;

use crate::Peri;
use crate::gic;
use crate::interrupt::typelevel::{OSTMI0, OSTMI1};
use crate::interrupt::{self, Binding, Handler, Interrupt};
use crate::ostm;
use crate::peripherals::{OSTM0, OSTM1};

const OSTM_IRQ_PRIORITY: u8 = 14;

/// Last value [`OstmDriver::raw_ostm_ticks`] published, as `(epoch << 32) | CNT`.
///
/// The 64-bit clock is extended from the 32-bit OSTM0 counter **in software**, by comparing each
/// CNT sample against the previous one: CNT going backwards means the counter wrapped. The wrap is
/// therefore detected by whoever reads the clock, not by an interrupt.
///
/// # Why not an ISR-maintained epoch counter
///
/// An epoch advanced by the OSTM0 wrap interrupt is wrong whenever that interrupt is **late**
/// (pending behind a same-or-higher-priority handler or a critical section): CNT has already
/// wrapped to a small value while the epoch has not advanced, so `now()` returns a value one full
/// epoch (2^32 ticks, ~129.9 s) in the past. `try_set_alarm` computes `at - now`, so a rewound
/// `now` arms OSTM1 for its maximum and every timer-driven task stalls.
///
/// Software extension removes the ISR from the correctness path: `now()` is monotonic by
/// construction however late any interrupt is.
///
/// # Requirement
///
/// The clock must be read at least once per wrap period (~129.9 s), or a wrap goes unobserved and
/// time loses an epoch. Satisfied structurally — the audio render path reads it continuously — and
/// guaranteed regardless by [`OverflowHandler`], which exists only to force one read per wrap.
static LAST_RAW: AtomicU64 = AtomicU64::new(0);

/// OSTM0 ticks per µs, from the registered clock tree. Set by [`init`].
static RATIO: AtomicTickRatio = AtomicTickRatio::unset();

// ---------------------------------------------------------------------------
// Driver struct + timer queue
// ---------------------------------------------------------------------------

struct OstmDriver {
    queue: Mutex<RefCell<Queue>>,
}

impl OstmDriver {
    const fn new() -> Self {
        Self {
            queue: Mutex::new(RefCell::new(Queue::new())),
        }
    }

    /// Raw OSTM0 tick count as a monotonic 64-bit value, extending the 32-bit counter in software.
    ///
    /// Never returns less than a previously returned value, for any interleaving of callers and any
    /// interrupt latency (see [`LAST_RAW`]).
    #[inline]
    fn raw_ostm_ticks() -> u64 {
        loop {
            let last = LAST_RAW.load(Ordering::Acquire);
            // Safety: OSTM0 is owned by this driver, and reading its counter has no side effect.
            let cnt = unsafe { ostm::count(0) };
            let val = extend(last, cnt);
            if val <= last {
                // A concurrent reader (or an ISR that preempted us mid-read) already published a
                // value at or ahead of ours. Hand back theirs: it is at least as current as ours,
                // and going backwards is the one thing this function must never do.
                return last;
            }
            match LAST_RAW.compare_exchange_weak(last, val, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return val,
                // Lost the race; re-read and retry. Bounded in practice: every retry means another
                // caller published, so the loop cannot spin without the clock advancing.
                Err(_) => continue,
            }
        }
    }

    /// Try to set OSTM1 to fire `at` embassy-ticks from epoch.
    ///
    /// Returns `false` (alarm already expired) so the caller can call
    /// `Queue::next_expiration` again and retry with a fresh `now`.
    fn try_set_alarm(&self, _cs: &CriticalSection, at: u64) -> bool {
        let now = self.now();
        if at <= now {
            return false;
        }
        // Convert delta from Embassy µs ticks to OSTM0 ticks.
        let delta_us = at - now;
        let ratio = RATIO.load();
        assert!(
            ratio.is_set(),
            "embassy-time alarm armed before rza1_hal::time_driver::init"
        );
        // Clamp to max_arm_ticks; a farther alarm is re-armed when that intermediate ISR
        // fires, so capping only costs wakeups, never accuracy.
        let delta_ostm = ratio
            .us_to_ticks(delta_us)
            .min(ratio.max_arm_ticks() as u64) as u32;

        if delta_ostm == 0 {
            return false;
        }

        // Safety: both channels are clocked and registers accessible.
        unsafe {
            ostm::start_alarm(1, delta_ostm);
            OSTMI1::enable();
        }
        true
    }
}

impl Driver for OstmDriver {
    fn now(&self) -> u64 {
        // Overflow-free for any uptime; reads 0 before `init` (see `TickRatio::ticks_to_us`).
        RATIO.load().ticks_to_us(Self::raw_ostm_ticks())
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            let mut queue = self.queue.borrow_ref_mut(cs);
            if queue.schedule_wake(at, waker) {
                // We now own the alarm slot. Keep trying until we either
                // arm a future alarm or exhaust the queue (all tasks already due).
                let mut next = queue.next_expiration(self.now());
                while !self.try_set_alarm(&cs, next) {
                    next = queue.next_expiration(self.now());
                }
            }
        });
    }
}

embassy_time_driver::time_driver_impl!(static DRIVER: OstmDriver = OstmDriver::new());

// ---------------------------------------------------------------------------
// ISR handlers registered with the GIC
// ---------------------------------------------------------------------------

/// OSTM0's compare match, at each 32-bit wrap: reads the clock, and nothing else. The wrap itself
/// is detected in software by [`OstmDriver::raw_ostm_ticks`]; this only guarantees the "read at
/// least once per wrap period" requirement [`LAST_RAW`] documents, so its latency is not a
/// correctness concern.
pub struct OverflowHandler;

impl Handler<OSTMI0> for OverflowHandler {
    unsafe fn on_interrupt() {
        let _ = OstmDriver::raw_ostm_ticks();
    }
}

/// OSTM1's interval interrupt: the one-shot alarm expired.
pub struct AlarmHandler;

impl Handler<OSTMI1> for AlarmHandler {
    unsafe fn on_interrupt() {
        // Stop OSTM1 at once (interval mode would reload), and mask it until the next alarm.
        unsafe {
            ostm::stop(1);
            OSTMI1::disable();
        }
        let now = DRIVER.now();
        critical_section::with(|cs| {
            let mut queue = DRIVER.queue.borrow_ref_mut(cs);
            let mut next = queue.next_expiration(now);
            while !DRIVER.try_set_alarm(&cs, next) {
                next = queue.next_expiration(DRIVER.now());
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Start the Embassy time driver on OSTM0 (free-running clock, interrupting at each wrap) and
/// OSTM1 (the alarm, dormant until a timer is scheduled).
///
/// # Panics
/// If [`crate::clocks::init`] has not been called.
pub fn init(
    _ostm0: Peri<'static, OSTM0>,
    _ostm1: Peri<'static, OSTM1>,
    irq: impl Binding<OSTMI0, OverflowHandler> + Binding<OSTMI1, AlarmHandler> + Copy + 'static,
) {
    RATIO.store(TickRatio::from_clocks(&crate::clocks::get()));
    // SAFETY: both channels are owned. CMP=0 makes OSTM0 interrupt as it wraps to 0.
    unsafe {
        ostm::enable_clock();
        ostm::start_free_running_cmp(0, 0);
    }
    ostm::set_running(true);
    interrupt::bind::<OSTMI0, OverflowHandler>(irq);
    interrupt::bind::<OSTMI1, AlarmHandler>(irq);
    // SAFETY: the two channels' own interrupts, below PMR so they reach the CPU. OSTM1 is
    // unmasked by `try_set_alarm` when an alarm is armed.
    unsafe {
        gic::set_priority(OSTMI0::ID, OSTM_IRQ_PRIORITY);
        gic::set_priority(OSTMI1::ID, OSTM_IRQ_PRIORITY);
        OSTMI0::enable();
    }
}

/// Stop the clock and the alarm: OSTM0 and OSTM1 halt and their interrupts are masked. For a
/// loader handing the chip to another program.
///
/// # Safety
/// Embassy time stops: no timer may be awaited afterwards, and `Instant::now` stands still.
pub unsafe fn stop() {
    ostm::set_running(false);
    unsafe {
        OSTMI0::disable();
        OSTMI1::disable();
        ostm::stop(0);
        ostm::stop(1);
    }
}
