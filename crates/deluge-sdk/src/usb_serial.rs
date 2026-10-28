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
use rza1_hal::usb::{Rusb1Driver, init_device_mode};

/// The `embassy-usb` driver for the Deluge's USB0 device port.
pub type Driver = Rusb1Driver<'static>;

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

use core::cell::RefCell;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use rza1_hal::Peri;
use rza1_hal::peripherals::USB0;

/// USB0, held from the runtime until [`Deluge::usb_serial`](crate::Deluge::usb_serial) takes it.
static PORT: Mutex<CriticalSectionRawMutex, RefCell<Option<Peri<'static, USB0>>>> =
    Mutex::new(RefCell::new(None));

/// Hold USB0 for [`Deluge::usb_serial`](crate::Deluge::usb_serial). The runtime calls this
/// before interrupts are enabled.
pub(crate) fn stash(usb0: Peri<'static, USB0>) {
    PORT.lock(|port| *port.borrow_mut() = Some(usb0));
}

/// Take the stashed USB0, once.
pub(crate) fn take_port() -> Option<Peri<'static, USB0>> {
    PORT.lock(|port| port.borrow_mut().take())
}

/// Build the CDC-ACM device and spawn its task. Called once, via
/// [`Deluge::usb_serial`](crate::Deluge::usb_serial)'s take-once guard.
pub(crate) fn start(spawner: Spawner, id: UsbIdentity, usb0: Peri<'static, USB0>) -> UsbSerial {
    let (_port, driver) = init_device_mode(usb0, crate::usb0::Irqs);
    // SAFETY: owning USB0 (`usb0`, taken once from the runtime's stash) makes this the only
    // caller, so the `static mut` buffers are borrowed exactly once.
    let (device, cdc) = unsafe {
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
