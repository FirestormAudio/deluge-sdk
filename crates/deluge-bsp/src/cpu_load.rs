//! A rough CPU-load meter for hardware tests.
//!
//! [`spin_task`] yields in a loop and counts how often the executor gets back to it, so its
//! count per second falls as interrupts and other tasks take time. [`report_task`] logs that
//! count once a second as a load: 1 − count / the highest count seen so far. Start both before
//! anything heavy runs, so the first seconds set the idle baseline.
//!
//! The spinning keeps the executor from ever sleeping, which is fine on the bench and not
//! something to ship.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::yield_now;
use embassy_time::Timer;
use log::info;

static SPINS: AtomicU32 = AtomicU32::new(0);

/// Counts executor round trips.
#[embassy_executor::task]
pub async fn spin_task() {
    loop {
        SPINS.fetch_add(1, Ordering::Relaxed);
        yield_now().await;
    }
}

/// Logs the load once a second, against the idlest second so far.
#[embassy_executor::task]
pub async fn report_task() {
    let mut idle = 0u32;
    loop {
        Timer::after_secs(1).await;
        let spins = SPINS.swap(0, Ordering::Relaxed);
        idle = idle.max(spins);
        let load_permille = 1000 - (spins as u64 * 1000 / idle.max(1) as u64) as u32;
        info!(
            "load: {}.{}% (spins {} / idle {})",
            load_permille / 10,
            load_permille % 10,
            spins,
            idle
        );
    }
}
