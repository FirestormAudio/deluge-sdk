use log::info;

/// Runs the `embassy_usb` device state machine (enumeration, control requests,
/// alternate-setting callbacks).  Must be kept alive at all times.
#[embassy_executor::task]
pub(crate) async fn usb_task(
    mut device: embassy_usb::UsbDevice<'static, rza1_hal::usb::Rusb1Driver<'static>>,
) {
    info!("usb_task: running");
    device.run().await;
}
