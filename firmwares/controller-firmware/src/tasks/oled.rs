use core::sync::atomic::{AtomicBool, Ordering};
use log::info;

use deluge_bsp::oled;
use deluge_bsp::pic;

// ---------------------------------------------------------------------------
// CDC-supplied framebuffer
// ---------------------------------------------------------------------------
//
// When a CDC host sends MSG_TO_UPDATE_DISPLAY, `set_cdc_display` stores the
// 768-byte frame here and sets CDC_FRAME_VALID.  The oled_task streams this
// framebuffer to the panel.  On MSG_TO_CLEAR_DISPLAY or host disconnect,
// `clear_cdc_display` clears the flag and oled_task blanks the panel.

static mut CDC_FRAME: [u8; oled::FRAME_BYTES] = [0u8; oled::FRAME_BYTES];
static CDC_FRAME_VALID: AtomicBool = AtomicBool::new(false);

/// Store a host-supplied OLED framebuffer and request a redraw.
///
/// `data` must be exactly [`oled::FRAME_BYTES`] bytes (768), laid out as
/// `data[page * oled::WIDTH + col]` (page-major, matching the SSD1309 wire
/// format).  Any extra bytes in `data` are ignored.
pub(crate) fn set_cdc_display(data: &[u8]) {
    let n = data.len().min(oled::FRAME_BYTES);
    // Safety: single-threaded cooperative executor; no concurrent writer.
    unsafe {
        CDC_FRAME[..n].copy_from_slice(&data[..n]);
    }
    CDC_FRAME_VALID.store(true, Ordering::Release);
    oled::notify_redraw();
}

/// Discard the host-supplied framebuffer and blank the panel.
pub(crate) fn clear_cdc_display() {
    CDC_FRAME_VALID.store(false, Ordering::Release);
    oled::notify_redraw();
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

/// OLED render task.
///
/// Initialises the SSD1309, then waits for the CDC host to supply frames via
/// [`set_cdc_display`].  The host owns the display entirely: each redraw either
/// streams the latest host frame or, when no frame is valid (no host connected),
/// blanks the panel.
#[embassy_executor::task]
pub(crate) async fn oled_task() {
    // Wait for pic::init() to complete before issuing any PIC UART commands.
    // Both tasks start concurrently; without this barrier oled::init() would
    // race with the baud-rate handshake in pic::init().
    pic::wait_ready().await;

    info!("OLED: init");
    oled::init().await;
    info!("OLED: ready");

    let mut fb = oled::FrameBuffer::new();

    // Render the initial (blank) state immediately.
    fb.fill(0x00);
    oled::send_frame(&fb).await;

    loop {
        oled::wait_redraw().await;

        if CDC_FRAME_VALID.load(Ordering::Acquire) {
            // Safety: CDC_FRAME is only mutated by set_cdc_display() which runs
            // from the CDC task; since the executor is single-threaded, there is
            // no concurrent access here.
            unsafe {
                fb.pages
                    .as_flattened_mut()
                    .copy_from_slice(&*core::ptr::addr_of!(CDC_FRAME));
            }
        } else {
            fb.fill(0x00);
        }
        oled::send_frame(&fb).await;
    }
}
