use embassy_time::Timer;

use deluge_bsp::usb::host::overbridge;
use usb_classes::overbridge::proto::MAX_CHANNELS;

/// One cycle of a 1 kHz sine at 48 kHz, −12 dBFS (0.25 × full scale).
const SINE: [i32; 48] = [
    0, 70075716, 138952417, 205451603, 268435456, 326826303, 379625062, 425928331, 464943848,
    496004047, 518577479, 532277907, 536870912, 532277907, 518577479, 496004047, 464943848,
    425928331, 379625062, 326826303, 268435456, 205451603, 138952417, 70075716, 0, -70075716,
    -138952417, -205451603, -268435456, -326826303, -379625062, -425928331, -464943848, -496004047,
    -518577479, -532277907, -536870912, -532277907, -518577479, -496004047, -464943848, -425928331,
    -379625062, -326826303, -268435456, -205451603, -138952417, -70075716,
];

/// Keeps the send ring topped up with the 1 kHz tone on every channel to the device.
#[embassy_executor::task]
pub(crate) async fn tone_task() {
    let stems = overbridge();
    let mut cycle = [0i32; 48 * MAX_CHANNELS];
    loop {
        Timer::after_millis(2).await;
        let Some(dev) = stems.info() else { continue };
        let ch = dev.ch_to as usize;
        while stems.stats().fill_out < 256 {
            for (f, &s) in SINE.iter().enumerate() {
                cycle[f * ch..(f + 1) * ch].fill(s);
            }
            if stems.write(&cycle[..48 * ch]) < 48 {
                break;
            }
        }
    }
}
