//! USB **DATA TRANSFER** mass-storage mode for the bootloader: exposes the raw
//! SD card to a host (like the standalone MSC firmware).
//!
//! It is launched from the boot menu and **returns when the BACK button is
//! pressed**, dropping back to the menu.  Rather than spawning detached tasks
//! (which cannot be cancelled), each mode runs the USB device, the BOT/SCSI loop
//! and the OLED status display concurrently with a BACK watcher via `select`;
//! when BACK wins, the other futures are dropped (cancelled), the port is
//! disconnected, and the function returns.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::join::join;
use embassy_futures::select::select;
use embassy_time::{Duration, Timer};
use log::info;

use deluge_bsp::oled::{self, text};
use deluge_bsp::usb::bot::{self, BlockDevice, SdBlock};
use deluge_bsp::usb::classes::msc::MscClass;
use rza1l_hal::gic;
use rza1l_hal::usb::{
    Rusb1Driver, Rusb1EndpointIn, Rusb1EndpointOut, dcd_int_handler, disconnect, init_device_mode,
};

// USB descriptor static buffers — must be `'static` for `embassy_usb::Builder`.
static mut USB_CONFIG_DESC: [u8; 256] = [0; 256];
static mut USB_BOS_DESC: [u8; 64] = [0; 64];
static mut USB_MSOS_DESC: [u8; 0] = [];
static mut USB_CONTROL_BUF: [u8; 64] = [0; 64];
static mut MSC_CLASS_BUF: MaybeUninit<MscClass> = MaybeUninit::uninit();

/// Ensures the USB0 ISR is wired into the GIC exactly once across mode entries.
static USB_IRQ_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Enter SD-card DATA TRANSFER (USB mass storage) mode.  Returns on BACK.
pub async fn run_data_transfer_mode() {
    info!("DATA: entering SD mass-storage mode");
    run_session(SdBlock, "Deluge SD Card").await;
}

/// Bring up USB MSC backed by `dev`, run until BACK, then disconnect.
async fn run_session<B: BlockDevice>(dev: B, product: &'static str) {
    bot::TX_BYTES.store(0, Ordering::Relaxed);
    bot::RX_BYTES.store(0, Ordering::Relaxed);

    let (mut device, ep_in, ep_out) = unsafe { build_usb(product) };
    info!("USB: device built ({})", product);

    let usb = device.run();
    // `run_until` watches BACK itself and returns only *between* SCSI commands,
    // so the SD card is never left mid-transfer (no torn writes / stuck card).
    let proto = bot::run_until(dev, ep_in, ep_out, &crate::BACK_PRESSED);
    let oled = status_loop();

    // USB + status run forever; the session ends exactly when `proto` returns
    // (BACK pressed at a safe command boundary).
    select(proto, join(usb, oled)).await;

    // Drop back to the menu: unplug from the host so it doesn't see a stale
    // unresponsive device.
    unsafe { disconnect(0) };
    info!("USB: mode exited (BACK)");
}

/// Build the USB device in MSC configuration.  Returns `(device, ep_in, ep_out)`.
///
/// # Safety
/// Mutates the module's `'static` descriptor buffers; one session at a time.
unsafe fn build_usb(
    product: &'static str,
) -> (
    embassy_usb::UsbDevice<'static, Rusb1Driver>,
    Rusb1EndpointIn,
    Rusb1EndpointOut,
) {
    unsafe {
        // Wire the USB0 ISR once (global IRQs are already enabled by boot_task).
        if !USB_IRQ_REGISTERED.swap(true, Ordering::AcqRel) {
            gic::register(rza1l_hal::usb::USB0_IRQ, || {
                dcd_int_handler(0);
            });
        }

        let (_port, driver) = init_device_mode(0);
        let mut config = embassy_usb::Config::new(
            deluge_bsp::usb::ids::VID,
            deluge_bsp::usb::ids::PID_APP_LOADER_MSC,
        );
        config.manufacturer = Some("Synthstrom Audible");
        config.product = Some(product);
        config.self_powered = false;
        config.max_power = 250; // 500 mA

        let mut builder = embassy_usb::Builder::new(
            driver,
            config,
            &mut *core::ptr::addr_of_mut!(USB_CONFIG_DESC),
            &mut *core::ptr::addr_of_mut!(USB_BOS_DESC),
            &mut *core::ptr::addr_of_mut!(USB_MSOS_DESC),
            &mut *core::ptr::addr_of_mut!(USB_CONTROL_BUF),
        );

        let (msc, ep_out, ep_in) = MscClass::new(&mut builder, 512);
        let msc_ref = (&mut *core::ptr::addr_of_mut!(MSC_CLASS_BUF)).write(msc);
        builder.handler(msc_ref);

        (builder.build(), ep_in, ep_out)
    }
}

/// Status-display refresh / speed-sampling interval.
const STATUS_INTERVAL_MS: u64 = 250;
/// First on-screen pixel row (matches the MSC firmware's throughput display).
const STATUS_TOP: usize = 10;

/// OLED throughput display for the active DATA TRANSFER session.  Loops forever
/// (cancelled by the `select` in [`run_session`] when BACK is pressed).
async fn status_loop() -> ! {
    // Previous cumulative byte counters, for per-interval speed.
    let mut last_tx = bot::TX_BYTES.load(Ordering::Relaxed);
    let mut last_rx = bot::RX_BYTES.load(Ordering::Relaxed);

    loop {
        // Live TX/RX speed (MB/s, one decimal) + cumulative volume (MB),
        // rendered exactly like the standalone USB mass-storage firmware.
        let tx = bot::TX_BYTES.load(Ordering::Relaxed);
        let rx = bot::RX_BYTES.load(Ordering::Relaxed);
        let dtx = tx.wrapping_sub(last_tx);
        let drx = rx.wrapping_sub(last_rx);
        last_tx = tx;
        last_rx = rx;
        // tenths of MB/s = delta_bytes / (interval_ms * 100).
        let tx_tenths = dtx / (STATUS_INTERVAL_MS * 100);
        let rx_tenths = drx / (STATUS_INTERVAL_MS * 100);

        let mut fb = oled::FrameBuffer::new();
        fb.fill(0x00);
        text::draw_str(&mut fb, 0, STATUS_TOP, b"USB MASS STORAGE");
        let mut line = [0u8; 24];
        let len = build_line(&mut line, b"TX ", tx_tenths, tx / 1_000_000);
        text::draw_str(&mut fb, 0, STATUS_TOP + 14, &line[..len]);
        let len = build_line(&mut line, b"RX ", rx_tenths, rx / 1_000_000);
        text::draw_str(&mut fb, 0, STATUS_TOP + 26, &line[..len]);
        oled::send_frame(&fb).await;

        Timer::after(Duration::from_millis(STATUS_INTERVAL_MS)).await;
    }
}

// ── Throughput line formatting (mirrors the MSC firmware's OLED task) ──────────

/// Format `"<label><speed>MB/S <total>MB"` into `out`, returning its length.
fn build_line(out: &mut [u8], label: &[u8], speed_tenths: u64, total_mb: u64) -> usize {
    let mut p = 0;
    for &b in label {
        push(out, &mut p, b);
    }
    push_dec1(out, &mut p, speed_tenths);
    for &b in b"MB/S " {
        push(out, &mut p, b);
    }
    push_u64(out, &mut p, total_mb);
    for &b in b"MB" {
        push(out, &mut p, b);
    }
    p
}

#[inline]
fn push(out: &mut [u8], p: &mut usize, b: u8) {
    if *p < out.len() {
        out[*p] = b;
        *p += 1;
    }
}

/// Write a base-10 integer.
fn push_u64(out: &mut [u8], p: &mut usize, mut v: u64) {
    if v == 0 {
        push(out, p, b'0');
        return;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        push(out, p, tmp[i]);
    }
}

/// Write a fixed-point value given in tenths as `"<int>.<dec>"`.
fn push_dec1(out: &mut [u8], p: &mut usize, tenths: u64) {
    push_u64(out, p, tenths / 10);
    push(out, p, b'.');
    push(out, p, b'0' + (tenths % 10) as u8);
}
