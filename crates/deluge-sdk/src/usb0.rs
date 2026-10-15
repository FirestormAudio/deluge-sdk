//! USB0 in device mode: its interrupt handler, bound for whichever SDK feature owns the port
//! (`usb-log` or `usb-serial`, never both).

use rza1l_hal::interrupt::{Handler, typelevel};

/// USB0's device-mode interrupt: the RUSB1 device driver's handler for port 0.
pub(crate) struct DeviceHandler;

impl Handler<typelevel::USB0> for DeviceHandler {
    unsafe fn on_interrupt() {
        // SAFETY: the GIC dispatcher calls this for USB0 only, the context the RUSB1 device
        // handler expects.
        unsafe { rza1l_hal::usb::dcd_int_handler(0) };
    }
}

rza1l_hal::bind_interrupts!(pub(crate) struct Irqs { USB0 => DeviceHandler; });
