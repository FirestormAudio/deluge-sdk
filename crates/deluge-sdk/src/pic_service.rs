//! Internal PIC co-processor service.
//!
//! The PIC32 is the Deluge's I/O co-processor: pads, buttons, indicator LEDs,
//! and the OLED chip-select handshake all flow over its UART. Several
//! capabilities ([`Oled`](crate::Oled), and later input/pads) depend on it, so
//! the SDK brings it up once, on demand, and runs a single RX pump that routes
//! incoming events.
//!
//! [`ensure_started`] is idempotent: the first capability that needs the PIC
//! initialises the UART and spawns [`pump`]; later callers are no-ops.

use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "none")]
use deluge_bsp::pic;
#[cfg(target_os = "none")]
use deluge_bsp::uart as bsp_uart;
use embassy_executor::Spawner;

/// PIC UART link speed (matches the demo firmware / PIC power-on baud).
#[cfg(target_os = "none")]
const PIC_BAUD: u32 = 31_250;

static STARTED: AtomicBool = AtomicBool::new(false);

/// Wait for the PIC to finish configuring. The host simulator has no PIC, so it
/// returns immediately.
#[cfg(target_os = "none")]
pub(crate) async fn wait_ready() {
    pic::wait_ready().await;
}
/// Host: nothing to wait for.
#[cfg(not(target_os = "none"))]
pub(crate) async fn wait_ready() {}

/// Host: there is no PIC co-processor to bring up.
#[cfg(not(target_os = "none"))]
pub(crate) fn ensure_started(_spawner: Spawner) {
    let _ = STARTED.swap(true, Ordering::Relaxed);
}

/// Ensure the PIC UART is up and the RX [`pump`] is running. Idempotent.
///
/// Call from an async capability constructor before awaiting [`wait_ready`].
#[cfg(target_os = "none")]
pub(crate) fn ensure_started(spawner: Spawner) {
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: runs once (guarded above). `init_pic` registers the TXI handler
    // before the source is enabled and sets up DMA RX, so it is safe to call with
    // interrupts already enabled.
    unsafe { bsp_uart::init_pic(PIC_BAUD) };
    // `#[embassy_executor::task]` returns a Result in this Embassy version; the
    // only failure is pool exhaustion, impossible for this single spawn.
    spawner.spawn(pump().unwrap());
}

/// PIC RX pump: configure the PIC, then forward decoded events.
///
/// Currently it routes only the OLED chip-select echoes that
/// [`oled::send_frame`](deluge_bsp::oled::send_frame) waits on. Input routing
/// (pads/buttons/encoders) is added alongside the `input()` capability.
#[cfg(target_os = "none")]
#[embassy_executor::task]
async fn pump() {
    // Configures debounce/refresh, switches to the fast baud, and signals
    // `pic::wait_ready()`.
    pic::init().await;

    let mut parser = pic::Parser::new();
    loop {
        let byte = bsp_uart::read_byte(pic::UART_CH).await;
        let Some(event) = parser.push(byte) else {
            continue;
        };
        match event {
            pic::Event::OledSelected => pic::notify_oled_selected(),
            pic::Event::OledDeselected => pic::notify_oled_deselected(),
            // Pads, buttons, etc. go to the input event queue (dropped there if
            // no `input()` consumer is draining it).
            other => crate::input::route_pic_event(other),
        }
    }
}
