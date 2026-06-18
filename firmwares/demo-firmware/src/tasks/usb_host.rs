use embassy_usb_driver::host::DeviceEvent;
use embassy_usb_host::handler::BusRoute;
use embassy_usb_host::{BusState, bus};
use log::{error, info};

use rza1l_hal::usb::Rusb1HostDriver;

/// Runs the USB host stack: waits for a device, enumerates it, then waits for
/// disconnect before looping.
///
/// Spawn this task *instead of* `usb_task` when the port is in host mode.
/// The `driver` must have been initialised via [`rza1l_hal::usb::init_host_mode`]
/// and the ISR wired to [`rza1l_hal::usb::hcd_int_handler`].
#[embassy_executor::task]
pub(crate) async fn usb_host_task(driver: Rusb1HostDriver) {
    info!("usb_host_task: running");

    // Bus-wide state (device-address allocator + enumeration lock) lives for
    // the lifetime of the task; a static gives it the `'static` lifetime that
    // `bus()` requires.
    static BUS_STATE: BusState = BusState::new();
    let (mut controller, handle) = bus(driver, &BUS_STATE);
    let mut config_buf = [0u8; 512];

    loop {
        let speed = controller.wait_for_connection().await;
        info!("USB host: device connected ({:?})", speed);

        match handle
            .enumerate(BusRoute::Direct(speed), &mut config_buf)
            .await
        {
            Ok((dev_info, _)) => {
                let addr = dev_info.device_address;
                info!(
                    "USB host: VID={:04x} PID={:04x} addr={}",
                    dev_info.device_desc.vendor_id, dev_info.device_desc.product_id, addr
                );
                // Wait for disconnect before accepting a new device.
                loop {
                    match controller.wait_for_device_event().await {
                        DeviceEvent::Disconnected => {
                            info!("USB host: device disconnected");
                            handle.free_address(addr);
                            break;
                        }
                        // Spurious re-connect / overcurrent while already
                        // connected; ignore.
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("USB host: enumeration failed: {:?}", e);
            }
        }
    }
}
