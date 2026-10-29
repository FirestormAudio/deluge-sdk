use embassy_time::Timer;
use log::debug;

/// Heartbeat blink — P6_7 plays a double-pulse heartbeat pattern so it is
/// visually obvious that the firmware is alive even when idle.
#[embassy_executor::task]
pub(crate) async fn blink_task(mut led: rza1_hal::gpio::Output<'static>) {
    debug!("blink_task: started");
    loop {
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
