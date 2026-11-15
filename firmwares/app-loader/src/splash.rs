//! The boot droplet on the panel, from the moment the display is up until the menu or the hand-off
//! ([`deluge_loader::splash`] decides what to show).
//!
//! A boot headed for the menu shows the whole droplet at once ([`begin`]). Otherwise one task,
//! [`animate`], draws it a block at a time at the stock bootloader's pace
//! ([`BLOCK_PERIOD_MS`]), never past a limit: [`PRE_ROLL_BLOCKS`] while the loader works, then as
//! far as the image load has got ([`progress`]). However fast the load, [`finish`] lets the droplet
//! play out whole at that pace before the hand-off.
//!
//! Only the animator draws droplet frames while it runs, so no stale, shorter frame can land last.
//! It checks it may draw and asks for the screen with no await in between: after [`stop`], no new
//! droplet frame starts, and anything drawn next (a message, the menu) goes out after it.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use deluge_bsp::oled::{boot_logo, boot_logo_marked, boot_logo_partial};
use deluge_loader::splash::{
    BLOCK_PERIOD_MS, LOGO_BLOCKS, PRE_ROLL_BLOCKS, SplashPlan, load_blocks, next_block, pad_ms,
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
/// The block count a load started from, once [`start_load`] (or the first [`progress`]) fixed it.
static BASE: AtomicU8 = AtomicU8::new(NO_BASE);
const NO_BASE: u8 = u8::MAX;

fn period() -> Duration {
    Duration::from_millis(BLOCK_PERIOD_MS)
}

/// Show `plan`'s first frame: the whole droplet (with its marker) for a boot headed for the menu,
/// or start the animator's pre-roll.
pub async fn begin(plan: SplashPlan) {
    match plan {
        SplashPlan::Static(marker) => {
            ACTIVE.store(false, Ordering::Release);
            DRAWN.store(LOGO_BLOCKS, Ordering::Release);
            SHOWN.store(LOGO_BLOCKS, Ordering::Release);
            let frame = match marker {
                Some(m) => boot_logo_marked(m.glyph()),
                None => boot_logo(),
            };
            screen::show(&frame).await;
        }
        SplashPlan::Animate => {
            LIMIT.store(PRE_ROLL_BLOCKS, Ordering::Release);
            ACTIVE.store(true, Ordering::Release);
        }
    }
}

/// The animator: each [`BLOCK_PERIOD_MS`], one more block while active and under the limit.
/// Spawned once at boot; idle otherwise.
#[embassy_executor::task]
pub async fn animate() {
    loop {
        Timer::after(period()).await;
        if !ACTIVE.load(Ordering::Acquire) {
            continue;
        }
        let drawn = DRAWN.load(Ordering::Acquire);
        if let Some(n) = next_block(drawn, LIMIT.load(Ordering::Acquire)) {
            DRAWN.store(n, Ordering::Release);
            screen::show(&boot_logo_partial(usize::from(n))).await;
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
    BASE.store(NO_BASE, Ordering::Release);
}

/// A load is starting, nothing read yet: fix its base (the blocks already drawn) and show the
/// droplet as it stands now, over the menu for a launch picked from it.
pub async fn start_load() {
    stop();
    let drawn = DRAWN.load(Ordering::Acquire);
    let _ = BASE.compare_exchange(NO_BASE, drawn, Ordering::AcqRel, Ordering::Acquire);
    screen::show(&boot_logo_partial(usize::from(drawn))).await;
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
}

/// Let the droplet play out whole at the stock pace, and return once its last frame is on the
/// panel. Must run while interrupts, the executor and the PIC service are still live: a frame
/// awaits the OLED DMA-completion IRQ and the PIC's chip-select echo, so it cannot complete once
/// the machine has been quiesced for the hand-off.
pub async fn finish() {
    LIMIT.store(LOGO_BLOCKS, Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
    // The animator needs at most `pad_ms` more; two periods' slack for the frame in flight.
    let deadline = Instant::now()
        + Duration::from_millis(pad_ms(SHOWN.load(Ordering::Acquire)) + 2 * BLOCK_PERIOD_MS);
    while SHOWN.load(Ordering::Acquire) < LOGO_BLOCKS && Instant::now() < deadline {
        Timer::after(period()).await;
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
    DRAWN.store(LOGO_BLOCKS, Ordering::Release);
    screen::show(&boot_logo()).await;
    SHOWN.store(LOGO_BLOCKS, Ordering::Release);
}
