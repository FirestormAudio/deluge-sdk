//! USB0 in device mode: its interrupt binding, for whichever SDK feature owns the port (`usb-log`
//! or `usb-serial`, never both).

rza1l_hal::bind_interrupts!(pub(crate) struct Irqs {
    USB0 => rza1l_hal::usb::DeviceInterruptHandler<rza1l_hal::peripherals::USB0>;
});
