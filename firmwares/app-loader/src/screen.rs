//! The display, shared by everything that draws: the menu, the prompts, DATA TRANSFER's status
//! screen and the dev-mode upload's progress. Each draws a whole frame and hands it to [`show`],
//! which copies it into the SDK's [`Oled`] and flushes it under a lock, so two drawers never
//! interleave a transfer. Who draws *when* is the callers' agreement (`ui::UPLOAD_ACTIVE`).

use core::cell::UnsafeCell;

use deluge::Oled;
use deluge::oled::FrameBuffer;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;

/// The [`Oled`], once [`init`] has put it here.
struct Shared(UnsafeCell<Option<Mutex<NoopRawMutex, Oled>>>);

// SAFETY: the loader is one thread-mode executor on one core, and nothing touches the display
// from an interrupt; `Oled` is `!Send` only to keep it on that executor, which it never leaves.
unsafe impl Sync for Shared {}

static SCREEN: Shared = Shared(UnsafeCell::new(None));

/// Hand the display over. Once, before anything draws.
pub fn init(oled: Oled) {
    let slot = SCREEN.0.get();
    // SAFETY: one thread (see `Shared`); read through the pointer, no reference made yet.
    assert!(unsafe { (*slot).is_none() }, "screen::init called twice");
    // SAFETY: called once at start-up, before any `show`, so no other reference exists.
    unsafe { *slot = Some(Mutex::new(oled)) };
}

/// Show `frame`, whole.
pub async fn show(frame: &FrameBuffer) {
    // SAFETY: `init` ran before the first draw and never runs again (see `Shared`).
    let screen = unsafe { &*SCREEN.0.get() }
        .as_ref()
        .expect("screen::show before screen::init");
    let mut oled = screen.lock().await;
    *oled.frame() = frame.clone();
    oled.flush().await;
}
