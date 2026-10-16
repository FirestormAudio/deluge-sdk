use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};
use usb_classes::overbridge::device::{OverbridgeReceiver, OverbridgeSender};
use usb_classes::overbridge::stems::OverbridgeHandle;

/// Channels this device sends (to the host) and receives (from it): a Digitakt's.
pub(crate) const CH_TO_HOST: usize = 12;
pub(crate) const CH_FROM_HOST: usize = 2;

/// The device's stems: "rx" is what the host sends, "tx" what this device sends.
pub(crate) static STEMS: OverbridgeHandle<{ CH_FROM_HOST * 1024 }, { CH_TO_HOST * 1024 }> =
    OverbridgeHandle::new();

/// Send a block whenever the stems task has written 7 frames, re-waiting while the host has
/// the endpoint disabled.
pub(crate) async fn send_forever<E: embassy_usb::driver::EndpointIn>(mut s: OverbridgeSender<E>) {
    loop {
        s.wait_enabled().await;
        info!("overbridge: host enabled the IN endpoint");
        while s.send_block(&STEMS).await.is_ok() {}
        warn!("overbridge: IN endpoint disabled");
    }
}

/// Receive blocks from the host, re-waiting while it has the endpoint disabled.
pub(crate) async fn receive_forever<E: embassy_usb::driver::EndpointOut>(
    mut r: OverbridgeReceiver<E>,
) {
    loop {
        r.wait_enabled().await;
        info!("overbridge: host enabled the OUT endpoint");
        while r.receive_block(&STEMS).await.is_ok() {}
        warn!("overbridge: OUT endpoint disabled");
    }
}

/// Writes 48 000 frames a second to the host, channel k a DC level of (k + 1) << 24, so each
/// stem is identifiable on the other side. An `Instant`-based frame budget keeps the rate from
/// drifting with the 1 ms timer's jitter.
#[embassy_executor::task]
pub(crate) async fn stems_task() {
    let mut frame = [0i32; CH_TO_HOST];
    for (k, s) in frame.iter_mut().enumerate() {
        *s = (k as i32 + 1) << 24;
    }
    let mut block = [0i32; 48 * CH_TO_HOST];
    for f in block.chunks_exact_mut(CH_TO_HOST) {
        f.copy_from_slice(&frame);
    }
    let start = Instant::now();
    let mut sent: u64 = 0;
    loop {
        Timer::after(Duration::from_millis(1)).await;
        let due = start.elapsed().as_micros() * 48 / 1000;
        while sent + 48 <= due {
            STEMS.write(&block);
            sent += 48;
        }
        // Drain what the host sends; the report shows its stats.
        let mut sink = [0i32; 48 * CH_FROM_HOST];
        while STEMS.read(&mut sink) > 0 {}
    }
}

#[embassy_executor::task]
pub(crate) async fn report_task() {
    loop {
        Timer::after_millis(1000).await;
        info!(
            "usb face {:?}: {:?}",
            crate::tasks::faces::current(),
            STEMS.stats()
        );
    }
}
