//! The boot droplet on the panel, from the moment the display is up until the menu or the hand-off
//! ([`deluge_loader::splash`] decides what to show).
//!
//! An INSTANT boot draws a few blocks on a timer ([`pre_roll`]) while the loader works, then the
//! image load draws the rest in proportion ([`progress`]) and [`finish`] completes it before the
//! hand-off. A boot headed for the menu shows the whole droplet at once ([`begin`]).
//!
//! Every draw renders the block count as it stands *at draw time*, and sends again if the count
//! moved meanwhile, so the pre-roll task and a load never leave a stale, shorter droplet.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use deluge_bsp::oled::{boot_logo, boot_logo_marked, boot_logo_partial};
use deluge_loader::splash::{
    LOGO_BLOCKS, PRE_ROLL_BLOCKS, PRE_ROLL_PERIOD_MS, SplashPlan, load_blocks,
};
use embassy_time::{Duration, Timer};

use crate::screen;

/// Blocks drawn so far; only ever raised (until [`restart`]).
static DRAWN: AtomicU8 = AtomicU8::new(0);
/// Whether the pre-roll task may still draw.
static PRE_ROLLING: AtomicBool = AtomicBool::new(false);
/// The block count a load started from, once the first [`progress`] call has fixed it.
static BASE: AtomicU8 = AtomicU8::new(NO_BASE);
const NO_BASE: u8 = u8::MAX;

/// Show `plan`'s first frame. Returns `true` when the boot animates: the caller then spawns
/// [`pre_roll`].
pub async fn begin(plan: SplashPlan) -> bool {
    match plan {
        SplashPlan::Static(marker) => {
            DRAWN.store(LOGO_BLOCKS, Ordering::Release);
            let frame = match marker {
                Some(m) => boot_logo_marked(m.glyph()),
                None => boot_logo(),
            };
            screen::show(&frame).await;
            false
        }
        SplashPlan::Animate => {
            PRE_ROLLING.store(true, Ordering::Release);
            true
        }
    }
}

/// Draw one block per [`PRE_ROLL_PERIOD_MS`] up to [`PRE_ROLL_BLOCKS`], until [`stop_pre_roll`].
#[embassy_executor::task]
pub async fn pre_roll() {
    while PRE_ROLLING.load(Ordering::Acquire) && DRAWN.load(Ordering::Acquire) < PRE_ROLL_BLOCKS {
        DRAWN.fetch_add(1, Ordering::AcqRel);
        draw().await;
        Timer::after(Duration::from_millis(PRE_ROLL_PERIOD_MS)).await;
    }
    PRE_ROLLING.store(false, Ordering::Release);
}

/// Stop the pre-roll: before anything else draws (a message, the menu), so no droplet frame
/// lands over it. A frame already being sent may still finish.
pub fn stop_pre_roll() {
    PRE_ROLLING.store(false, Ordering::Release);
}

/// Start the droplet over, for a launch picked from the menu.
pub fn restart() {
    stop_pre_roll();
    DRAWN.store(0, Ordering::Release);
    BASE.store(NO_BASE, Ordering::Release);
}

/// `done` of `total` bytes loaded: stop the pre-roll and draw the droplet in proportion. The first
/// call after [`begin`] or [`restart`] fixes the base (the blocks already drawn).
///
/// The first call always draws, even with no new block: after [`restart`] that clears the menu the
/// launch was picked from, at once.
pub async fn progress(done: u32, total: u32) {
    stop_pre_roll();
    let drawn = DRAWN.load(Ordering::Acquire);
    match BASE.compare_exchange(NO_BASE, drawn, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            DRAWN.fetch_max(load_blocks(drawn, done, total), Ordering::AcqRel);
            draw().await;
        }
        Err(base) => raise_to(load_blocks(base, done, total)).await,
    }
}

/// A load is starting, nothing read yet: fix its base and show the droplet now (over the menu,
/// for a launch picked from it).
pub async fn start_load() {
    progress(0, 1).await;
}

/// Draw the whole droplet. Must run while interrupts, the executor and the PIC service are still
/// live: a frame awaits the OLED DMA-completion IRQ and the PIC's chip-select echo, so it cannot
/// complete once the machine has been quiesced for the hand-off.
pub async fn finish() {
    stop_pre_roll();
    raise_to(LOGO_BLOCKS).await;
}

async fn raise_to(n: u8) {
    if DRAWN.fetch_max(n, Ordering::AcqRel) < n {
        draw().await;
    }
}

/// Show the droplet as it stands now. The frame is built before the screen lock is taken, so if
/// the count moved while it was being sent (another drawer raised it), send it again: the last
/// frame out is always the current count.
async fn draw() {
    loop {
        let n = DRAWN.load(Ordering::Acquire);
        screen::show(&boot_logo_partial(usize::from(n))).await;
        if DRAWN.load(Ordering::Acquire) == n {
            break;
        }
    }
}
