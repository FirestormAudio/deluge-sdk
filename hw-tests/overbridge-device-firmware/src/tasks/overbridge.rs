use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};
use rza1l_hal::usb::{Rusb1Driver, Rusb1EndpointIn, Rusb1EndpointOut};
use usb_classes::overbridge::device::{OverbridgeReceiver, OverbridgeSender, Shared};
use usb_classes::overbridge::stems::OverbridgeHandle;

/// Channels this device sends (to the host) and receives (from it): a Digitakt's.
pub(crate) const CH_TO_HOST: usize = 12;
pub(crate) const CH_FROM_HOST: usize = 2;

/// The device's stems: "rx" is what the host sends, "tx" what this device sends.
pub(crate) static STEMS: OverbridgeHandle<{ CH_FROM_HOST * 1024 }, { CH_TO_HOST * 1024 }> =
    OverbridgeHandle::new();

#[embassy_executor::task]
pub(crate) async fn usb_task(mut device: embassy_usb::UsbDevice<'static, Rusb1Driver<'static>>) {
    device.run().await;
}

/// Sends a block whenever the stems task has written 7 frames.
#[embassy_executor::task]
pub(crate) async fn sender_task(mut s: OverbridgeSender<Rusb1EndpointIn>) {
    loop {
        s.wait_enabled().await;
        info!("overbridge: host enabled the IN endpoint");
        while s.send_block(&STEMS).await.is_ok() {}
        warn!("overbridge: IN endpoint disabled");
    }
}

#[embassy_executor::task]
pub(crate) async fn receiver_task(mut r: OverbridgeReceiver<Rusb1EndpointOut>) {
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
pub(crate) async fn report_task(shared: &'static Shared) {
    loop {
        Timer::after_millis(1000).await;
        let name = shared.host_name();
        let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        info!(
            "overbridge: streaming={} host={:?} {:?}",
            shared.streaming(),
            core::str::from_utf8(&name[..len]).unwrap_or("?"),
            STEMS.stats()
        );
    }
}
