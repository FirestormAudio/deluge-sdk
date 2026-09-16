//! USB CDC-ACM serial port on USB0 (the `usb-serial` feature).
//!
//! The USB0 ISR is registered by the runtime before interrupts are enabled (the
//! GIC contract), so the app needs no `unsafe` and no `setup` hook. The device
//! itself — one CDC-ACM interface under the app's own [`UsbIdentity`] — is only
//! built, and its task spawned, when the app calls
//! [`Deluge::usb_serial`](crate::Deluge::usb_serial), which happens later, after
//! interrupts are enabled; that call also hands back the IN/OUT halves.

use core::ptr::addr_of_mut;

use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, Sender, State};
use embassy_usb::{Builder, Config, UsbDevice};
use rza1l_hal::gic;
use rza1l_hal::usb::{Rusb1Driver, USB0_IRQ, dcd_int_handler, init_device_mode};

/// The `embassy-usb` driver for the Deluge's USB0 device port.
pub type Driver = Rusb1Driver;

/// How the serial device identifies itself to the host.
#[derive(Clone, Copy, Debug)]
pub struct UsbIdentity {
    /// USB vendor ID — e.g. an allocation from [pid.codes](https://pid.codes)
    /// for open-source hardware.
    pub vid: u16,
    /// USB product ID.
    pub pid: u16,
    /// Manufacturer string descriptor, shown by the host.
    pub manufacturer: &'static str,
    /// Product string descriptor, shown by the host.
    pub product: &'static str,
    /// Serial-number string descriptor, shown by the host.
    pub serial_number: &'static str,
}

/// A USB CDC-ACM port: write host-bound bytes to `tx`, read host bytes from `rx`.
pub struct UsbSerial {
    pub tx: Sender<'static, Driver>,
    pub rx: Receiver<'static, Driver>,
}

// `embassy_usb::Builder` and `CdcAcmClass` need `'static` buffers. One CDC
// interface fits comfortably in 256 B of config descriptor.
static mut USB_CONFIG_DESC: [u8; 256] = [0; 256];
static mut USB_BOS_DESC: [u8; 64] = [0; 64];
static mut USB_MSOS_DESC: [u8; 0] = [];
static mut USB_CONTROL_BUF: [u8; 64] = [0; 64];
static mut CDC_ACM_STATE: State<'static> = State::new();

/// Register the USB0 device-mode ISR.
///
/// # Safety
/// Call once, with IRQs masked (before `cortex_ar::interrupt::enable()`).
pub(crate) unsafe fn register_irq() {
    unsafe { gic::register(USB0_IRQ, || dcd_int_handler(0)) };
}

/// Build the CDC-ACM device and spawn its task. Called once, via
/// [`Deluge::usb_serial`](crate::Deluge::usb_serial)'s take-once guard.
pub(crate) fn start(spawner: Spawner, id: UsbIdentity) -> UsbSerial {
    // SAFETY: the take-once guard in `Deluge::usb_serial` makes this the only
    // caller, so USB0 is initialised once and the `static mut` buffers are
    // borrowed exactly once.
    let (device, cdc) = unsafe {
        let (_port, driver) = init_device_mode(0);
        let mut config = Config::new(id.vid, id.pid);
        config.manufacturer = Some(id.manufacturer);
        config.product = Some(id.product);
        config.serial_number = Some(id.serial_number);
        config.self_powered = false;
        config.max_power = 250; // 500 mA

        let mut builder = Builder::new(
            driver,
            config,
            &mut *addr_of_mut!(USB_CONFIG_DESC),
            &mut *addr_of_mut!(USB_BOS_DESC),
            &mut *addr_of_mut!(USB_MSOS_DESC),
            &mut *addr_of_mut!(USB_CONTROL_BUF),
        );
        // 512-byte bulk endpoints: the RUSB1 PHY negotiates high speed, where
        // USB 2.0 requires HS bulk wMaxPacketSize 512.
        let cdc = CdcAcmClass::new(&mut builder, &mut *addr_of_mut!(CDC_ACM_STATE), 512);
        (builder.build(), cdc)
    };
    let (tx, rx) = cdc.split();
    spawner.spawn(usb_run(device).unwrap());
    UsbSerial { tx, rx }
}

/// Run the USB device stack (enumeration, control transfers, endpoints).
#[embassy_executor::task]
async fn usb_run(mut device: UsbDevice<'static, Driver>) {
    device.run().await;
}
