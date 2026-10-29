use core::sync::atomic::{AtomicBool, Ordering};

use embassy_time::Timer;
use log::debug;

static HEARTBEAT_ENABLED: AtomicBool = AtomicBool::new(true);

pub(crate) fn toggle_heartbeat_enabled() -> bool {
    let enabled = !HEARTBEAT_ENABLED.load(Ordering::Relaxed);
    HEARTBEAT_ENABLED.store(enabled, Ordering::Relaxed);
    enabled
}

/// Heartbeat blink — P6_7 plays a double-pulse heartbeat pattern.
#[embassy_executor::task]
pub(crate) async fn blink_task(mut led: rza1_hal::gpio::Output<'static>) {
    debug!("blink_task: started");
    loop {
        if !HEARTBEAT_ENABLED.load(Ordering::Relaxed) {
            led.set_low();
            Timer::after_millis(100).await;
            continue;
        }

        led.set_high();
        Timer::after_millis(80).await;

        led.set_low();
        Timer::after_millis(120).await;

        led.set_high();
        Timer::after_millis(80).await;

        led.set_low();
        Timer::after_millis(720).await;
    }
}
