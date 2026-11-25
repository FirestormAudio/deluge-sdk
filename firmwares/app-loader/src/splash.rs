//! The boot droplet on the panel, from the moment the display is up until the menu or the hand-off
//! ([`deluge_loader::splash`] decides what to show).
//!
//! One task, [`animate`], draws the droplet a block at a time at the stock bootloader's pace
//! ([`BLOCK_PERIOD_MS`]), never past a limit ([`begin`]): the whole droplet on a boot headed for
//! the menu (every frame marked `(D)` or `(R)` when there's a reason to say), or
//! [`PRE_ROLL_BLOCKS`] on an INSTANT boot while the loader works, then as far as the image load has
//! got ([`progress`]). However fast the load, [`finish`] lets the droplet play out whole at that
//! pace before the menu or the hand-off.
//!
//! Only the animator draws droplet frames while it runs, so no stale, shorter frame can land last.
//! It checks it may draw and asks for the screen with no await in between: after [`stop`], no new
//! droplet frame starts, and anything drawn next (a message, the menu) goes out after it.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use deluge_bsp::oled::FrameBuffer;
use deluge_bsp::oled::{boot_logo_partial, stamp_marker};
#[cfg(doc)]
use deluge_loader::splash::PRE_ROLL_BLOCKS;
use deluge_loader::splash::{
    BLOCK_PERIOD_MS, LOGO_BLOCKS, SplashPlan, load_blocks, next_block, next_due, pad_ms,
};
use embassy_time::{Duration, Instant, Timer};

use crate::screen;

/// Blocks drawn so far (a frame with this many has at least started out).
static DRAWN: AtomicU8 = AtomicU8::new(0);
/// Blocks on the panel: the last droplet frame whose send completed.
static SHOWN: AtomicU8 = AtomicU8::new(0);
/// How far the animator may go.
static LIMIT: AtomicU8 = AtomicU8::new(0);
/// Whether the animator may draw.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The marker's letter for the whole droplet's frame, or 0 for none.
static MARKER: AtomicU8 = AtomicU8::new(0);
/// The block count a load started from, once [`start_load`] (or the first [`progress`]) fixed it.
static BASE: AtomicU8 = AtomicU8::new(NO_BASE);
const NO_BASE: u8 = u8::MAX;

/// Start the droplet for `plan`: how far it may go before a load, and its marker.
pub fn begin(plan: SplashPlan) {
    MARKER.store(plan.marker().map_or(0, |m| m.glyph()), Ordering::Release);
    LIMIT.store(plan.limit(), Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
}

/// The droplet's frame with `n` blocks, with the marker, if any, on every frame.
fn frame(n: u8) -> FrameBuffer {
    let mut fb = boot_logo_partial(usize::from(n));
    match MARKER.load(Ordering::Acquire) {
        0 => {}
        m => stamp_marker(&mut fb, m),
    }
    fb
}

/// The animator: each [`BLOCK_PERIOD_MS`], one more block while active and under the limit.
/// Blocks are due on a fixed grid, so a frame's send time (a few ms) doesn't stretch the pace;
/// after a wake late enough to have missed a slot (the executor was busy), the grid restarts from
/// then ([`next_due`]) rather than drawing the missed blocks back to back. Spawned once at boot;
/// idle otherwise.
#[embassy_executor::task]
pub async fn animate() {
    let mut due = Instant::now().as_millis() + BLOCK_PERIOD_MS;
    loop {
        Timer::at(Instant::from_millis(due)).await;
        due = next_due(due, Instant::now().as_millis());
        if !ACTIVE.load(Ordering::Acquire) {
            continue;
        }
        let drawn = DRAWN.load(Ordering::Acquire);
        if let Some(n) = next_block(drawn, LIMIT.load(Ordering::Acquire)) {
            DRAWN.store(n, Ordering::Release);
            screen::show(&frame(n)).await;
            SHOWN.store(n, Ordering::Release);
        }
    }
}

/// Stop drawing droplet frames: before anything else draws (a message, the menu), so none lands
/// over it. A frame already being sent still finishes, first.
pub fn stop() {
    ACTIVE.store(false, Ordering::Release);
}

/// Start the droplet over, for a launch picked from the menu.
pub fn restart() {
    stop();
    DRAWN.store(0, Ordering::Release);
    SHOWN.store(0, Ordering::Release);
    LIMIT.store(0, Ordering::Release);
    MARKER.store(0, Ordering::Release);
    BASE.store(NO_BASE, Ordering::Release);
}

/// A load is starting, nothing read yet: fix its base (the blocks already drawn) and show the
/// droplet as it stands now, over the menu for a launch picked from it.
pub async fn start_load() {
    stop();
    let drawn = DRAWN.load(Ordering::Acquire);
    let _ = BASE.compare_exchange(NO_BASE, drawn, Ordering::AcqRel, Ordering::Acquire);
    screen::show(&frame(drawn)).await;
    SHOWN.store(drawn, Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
}

/// `done` of `total` bytes loaded: let the droplet go as far as the load has got. It is drawn at
/// the animator's pace, not here.
pub async fn progress(done: u32, total: u32) {
    let drawn = DRAWN.load(Ordering::Acquire);
    let base = match BASE.compare_exchange(NO_BASE, drawn, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => drawn,
        Err(b) => b,
    };
    LIMIT.fetch_max(load_blocks(base, done, total), Ordering::AcqRel);
    ACTIVE.store(true, Ordering::Release);
    // Let the animator run: a caller like the flash app's CRC check otherwise never yields
    // between its chunks.
    embassy_futures::yield_now().await;
}

/// Let the droplet play out whole at the stock pace, and return once its last frame is on the
/// panel: before the menu draws, and before the hand-off. Must run while interrupts, the
/// executor and the PIC service are still live: a frame awaits the OLED DMA-completion IRQ and
/// the PIC's chip-select echo, so it cannot complete once the machine has been quiesced for the
/// hand-off.
pub async fn finish() {
    LIMIT.store(LOGO_BLOCKS, Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
    // The animator needs at most `pad_ms` more; the deadline only catches one that stalled, so
    // it has half as long again, and a 100 ms floor.
    let pad = pad_ms(SHOWN.load(Ordering::Acquire));
    let deadline = Instant::now() + Duration::from_millis(pad + pad / 2 + 100);
    // Polled finely, so the menu or the hand-off follows the last block promptly.
    while SHOWN.load(Ordering::Acquire) < LOGO_BLOCKS && Instant::now() < deadline {
        Timer::after(Duration::from_millis(2)).await;
    }
    stop();
    if SHOWN.load(Ordering::Acquire) < LOGO_BLOCKS {
        // The animator didn't keep up: never hand off on a part-drawn droplet.
        finish_now().await;
    }
}

/// The whole droplet at once, no pacing: for the dev-mode USB launch, where the dev loop's speed
/// matters more than the animation.
pub async fn finish_now() {
    stop();
    // A launch over USB from the dev-mode menu: the app starts on the plain droplet, as from the
    // menu.
    MARKER.store(0, Ordering::Release);
    DRAWN.store(LOGO_BLOCKS, Ordering::Release);
    screen::show(&frame(LOGO_BLOCKS)).await;
    SHOWN.store(LOGO_BLOCKS, Ordering::Release);
}
