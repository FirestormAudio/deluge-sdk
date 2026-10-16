use embassy_time::Timer;
use log::info;

use deluge_bsp::usb::host::overbridge;
use usb_classes::overbridge::proto::MAX_CHANNELS;

/// Peak level as whole decibels below full scale, at 6 dB resolution (one per bit), so no
/// floating-point log is needed. `None` for silence.
fn peak_db(peak: u32) -> Option<i32> {
    (peak != 0).then(|| -6 * (31 - (32 - peak.leading_zeros()) as i32))
}

/// Once a second: the device, its stream counters, and each stem's peak since the last report.
/// Drains the receive ring, so it also exercises the app side of the handle.
#[embassy_executor::task]
pub(crate) async fn report_task() {
    let stems = overbridge();
    let mut connection = 0;
    let mut peaks = [0u32; MAX_CHANNELS];
    let mut buf = [0i32; 7 * MAX_CHANNELS];
    loop {
        for _ in 0..100 {
            Timer::after_millis(10).await;
            let Some(dev) = stems.info() else { continue };
            let ch = dev.ch_from as usize;
            loop {
                let n = stems.read(&mut buf[..7 * ch]);
                for frame in buf[..n * ch].chunks_exact(ch) {
                    for (p, &s) in peaks.iter_mut().zip(frame) {
                        *p = (*p).max(s.unsigned_abs());
                    }
                }
                if n < 7 {
                    break;
                }
            }
        }
        let Some(dev) = stems.info() else {
            info!("overbridge: no device");
            continue;
        };
        if stems.connection() != connection {
            connection = stems.connection();
            info!("overbridge: {:?}", dev);
        }
        info!("overbridge: {:?}", stems.stats());
        let ch = dev.ch_from as usize;
        for (c, p) in peaks[..ch].iter_mut().enumerate() {
            match peak_db(*p) {
                Some(db) => info!("  stem {:2}: ~{} dBFS", c, db),
                None => info!("  stem {:2}: silent", c),
            }
            *p = 0;
        }
    }
}
