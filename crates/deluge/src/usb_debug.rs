//! USB CDC-ACM debug serial (the `usb-log` feature).
//!
//! Brings up a USB device on USB0 exposing a single CDC-ACM (virtual serial)
//! interface and routes the `log` crate to it, so firmware logs appear on a host
//! `/dev/ttyACM*` port over the same USB cable — **no debug probe required**.
//!
//! Flow: [`init_logger`] registers a [`log::Log`] that pushes formatted records
//! into a non-blocking [`Pipe`] (dropping on overflow, so logging never blocks
//! and is callable from any context, even before USB is up). [`build`] (called in
//! the runtime before interrupts are enabled) constructs the [`UsbDevice`] +
//! [`Sender`]; [`spawn`] launches the device task plus a drain task that streams
//! the pipe out the CDC IN endpoint once a host connects.
//!
//! This module is written so a second CDC interface (a future GDB stub, M8) can
//! be added to the same device without restructuring.

use core::fmt::Write;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::pipe::Pipe;
use embassy_usb::class::cdc_acm::{CdcAcmClass, Sender, State};
use embassy_usb::{Builder, Config, UsbDevice};
use log::{LevelFilter, Log, Metadata, Record};

use rza1l_hal::gic;
use rza1l_hal::usb::{Rusb1Driver, USB0_IRQ, dcd_int_handler, init_device_mode};

// ── USB descriptor / class `'static` backing storage ─────────────────────────
//
// `embassy_usb::Builder` and `CdcAcmClass` need `'static` buffers. A single CDC
// interface is small, so 256 B of config descriptor is plenty (the controller
// firmware uses 768 B only because it also carries UAC2 + MIDI).

static mut USB_CONFIG_DESC: [u8; 256] = [0; 256];
static mut USB_BOS_DESC: [u8; 64] = [0; 64];
static mut USB_MSOS_DESC: [u8; 0] = [];
static mut USB_CONTROL_BUF: [u8; 64] = [0; 64];
static mut CDC_ACM_STATE: State = State::new();

// ── Log buffer + backend ─────────────────────────────────────────────────────

/// Non-blocking byte buffer between `log!` call sites and the USB drain task.
/// Logs emitted before a host connects accumulate here (up to capacity).
static LOG_PIPE: Pipe<CriticalSectionRawMutex, 4096> = Pipe::new();

struct UsbLogger;
static USB_LOGGER: UsbLogger = UsbLogger;

impl Log for UsbLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        // Format one line into a stack buffer, then push what fits into the pipe.
        // Overflow is dropped so logging never blocks (and stays ISR-safe).
        let mut line = FmtBuf::new();
        let _ = write!(
            line,
            "{:<5} {}: {}\r\n",
            record.level(),
            record.target(),
            record.args()
        );
        let _ = LOG_PIPE.try_write(line.as_bytes());
    }

    fn flush(&self) {}
}

/// Register the USB logger. Safe to call before the USB device exists — records
/// just queue in [`LOG_PIPE`] until the drain task starts and a host connects.
pub fn init_logger() {
    let _ = log::set_logger(&USB_LOGGER);
    log::set_max_level(LevelFilter::Debug);
}

/// A fixed-capacity `core::fmt::Write` sink for one log line. Excess is dropped.
struct FmtBuf {
    buf: [u8; 256],
    len: usize,
}

impl FmtBuf {
    fn new() -> Self {
        Self {
            buf: [0; 256],
            len: 0,
        }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Write for FmtBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

// ── Device bring-up ──────────────────────────────────────────────────────────

/// Build the USB-debug device (USB0, one CDC-ACM interface) and return it with
/// the CDC IN [`Sender`].
///
/// Must be called from the runtime **before** `cortex_ar::interrupt::enable()`:
/// it registers the USB0 ISR and then `builder.build()` calls `driver.start()`,
/// which enables the USB interrupt source — mirroring the controller firmware's
/// proven ordering.
///
/// # Safety
/// Call exactly once, after platform/clock init and before interrupts are
/// enabled. Takes ownership of USB0 (apps must not also bring up a USB stack).
pub unsafe fn build() -> (
    UsbDevice<'static, Rusb1Driver>,
    Sender<'static, Rusb1Driver>,
) {
    let (_port, driver) = unsafe { init_device_mode(0) };

    let mut config = Config::new(0x16D0, 0x0EDA);
    config.manufacturer = Some("Synthstrom Audible");
    config.product = Some("Deluge (SDK log)");
    config.self_powered = false;
    config.max_power = 250; // 500 mA

    let mut builder = Builder::new(
        driver,
        config,
        unsafe { &mut *core::ptr::addr_of_mut!(USB_CONFIG_DESC) },
        unsafe { &mut *core::ptr::addr_of_mut!(USB_BOS_DESC) },
        unsafe { &mut *core::ptr::addr_of_mut!(USB_MSOS_DESC) },
        unsafe { &mut *core::ptr::addr_of_mut!(USB_CONTROL_BUF) },
    );

    // Register the USB0 ISR before build() enables the interrupt source. Device
    // mode only, so always dispatch to the device handler.
    unsafe { gic::register(USB0_IRQ, || dcd_int_handler(0)) };

    // 512-byte bulk endpoints: the RUSB1 PHY negotiates high speed, and USB 2.0
    // requires HS bulk endpoints to advertise wMaxPacketSize 512.
    let cdc = CdcAcmClass::new(
        &mut builder,
        unsafe { &mut *core::ptr::addr_of_mut!(CDC_ACM_STATE) },
        512,
    );
    // We only emit logs; the OUT direction (host→device) is unused.
    let (tx, _rx) = cdc.split();

    let device = builder.build();
    (device, tx)
}

/// Spawn the USB-debug device + log-drain tasks. Call from the executor closure.
pub fn spawn(
    spawner: Spawner,
    bits: (
        UsbDevice<'static, Rusb1Driver>,
        Sender<'static, Rusb1Driver>,
    ),
) {
    let (device, tx) = bits;
    spawner.spawn(usb_run(device).unwrap());
    spawner.spawn(drain(tx).unwrap());
}

/// Run the USB device stack (enumeration, control transfers, endpoints).
#[embassy_executor::task]
async fn usb_run(mut device: UsbDevice<'static, Rusb1Driver>) {
    device.run().await;
}

/// Stream buffered log bytes out the CDC IN endpoint while a host is connected.
#[embassy_executor::task]
async fn drain(mut tx: Sender<'static, Rusb1Driver>) {
    let mut buf = [0u8; 256];
    loop {
        // Wait for the host to open the port (DTR / endpoint enabled).
        tx.wait_connection().await;
        loop {
            // `read` wakes as soon as any bytes are buffered; ≤256 ≤ 512 (HS max).
            let n = LOG_PIPE.read(&mut buf).await;
            if tx.write_packet(&buf[..n]).await.is_err() {
                break; // host disconnected — go back to waiting
            }
        }
    }
}
