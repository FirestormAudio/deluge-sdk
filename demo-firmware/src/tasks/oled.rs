use core::sync::atomic::Ordering;
use log::info;

use embassy_time::Timer;

use crate::tasks::analysis::WAVEFORM;
use crate::tasks::audio::USB_AUDIO_STREAMING;
use deluge_bsp::oled;
use deluge_bsp::pads::{pad_get, pad_id_from_xy};
use deluge_bsp::pic;

// ---------------------------------------------------------------------------
// OLED layout constants
// ---------------------------------------------------------------------------
//
// 18 pad columns × 8 pad rows — each cell is CELL_W × CELL_H pixels.
// 18 × 7 = 126 px ≤ 128 (2 px right margin);  8 × 6 = 48 px exactly.

const CELL_W: usize = 7; // px per pad column  (18 × 7 = 126 ≤ 128)
const CELL_H: usize = 5; // px per pad row      ( 8 × 5 =  40 ≤ 43 visible)
const FILL_W: usize = 5; // CELL_W − 2px borders
const FILL_H: usize = 3; // CELL_H − 2px borders
/// First visible OLED row (rows 0–4 are off-panel, C constant OLED_MAIN_TOPMOST_PIXEL=5).
const TOPMOST: usize = 5;

/// Render the latest waveform snapshot from `analysis_task` as a dot-scope.
///
/// Render the latest waveform snapshot from `analysis_task` as an oscilloscope trace.
///
/// Consecutive columns are joined by a vertical line segment so the trace is
/// continuous even when the signal changes by more than one pixel per step.
fn render_waveform(fb: &mut oled::FrameBuffer) {
    fb.fill(0x00);

    // TOPMOST = 5 (first on-screen row); usable rows: [5, 47] = 43 rows.
    const CENTER: i32 = TOPMOST as i32 + (oled::HEIGHT as i32 - TOPMOST as i32) / 2; // ≈ 26
    const HALF_SCALE: f32 = 20.0; // ± pixels for a ±1 signal
    const Y_MIN: i32 = TOPMOST as i32;
    const Y_MAX: i32 = oled::HEIGHT as i32 - 1;

    // SAFETY: written only by analysis_task; no concurrent writer in a
    //         single-threaded cooperative executor.
    let waveform = unsafe { &*core::ptr::addr_of!(WAVEFORM) };

    let sample_to_y =
        |s: f32| -> usize { (CENTER - (s * HALF_SCALE) as i32).clamp(Y_MIN, Y_MAX) as usize };

    let mut prev_y = sample_to_y(waveform[0]);
    fb.set_pixel(0, prev_y, true);

    for (x, &sample) in waveform.iter().enumerate().skip(1).take(oled::WIDTH - 1) {
        let y = sample_to_y(sample);
        // Draw a vertical segment from prev_y to y so there are no gaps.
        let (y0, y1) = if prev_y <= y {
            (prev_y, y)
        } else {
            (y, prev_y)
        };
        for row in y0..=y1 {
            fb.set_pixel(x, row, true);
        }
        prev_y = y;
    }
}

/// Render the current `PAD_BITS` state into `fb`, clearing first.
///
/// OLED rows 0–4 are off-panel (`TOPMOST`=5).  Pad y=0 is the physical
/// bottom row (lower-left origin), so `py=0` (OLED top cell) maps to pad
/// row y=7, and `py=7` (OLED bottom cell) maps to pad row y=0.
fn render_pads(fb: &mut oled::FrameBuffer) {
    fb.fill(0x00);
    for py in 0..8u8 {
        let pad_y = 7 - py; // flip: OLED top row = highest pad row
        for px in 0..18u8 {
            if !pad_get(pad_id_from_xy(px, pad_y)) {
                continue;
            }
            // 1 px gap on all sides → inner rect starts at (+1, +1)
            let ox = px as usize * CELL_W + 1;
            let oy = TOPMOST + py as usize * CELL_H + 1;
            for dy in 0..FILL_H {
                for dx in 0..FILL_W {
                    fb.set_pixel(ox + dx, oy + dy, true);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Starscape screensaver
// ---------------------------------------------------------------------------
//
// Classic perspective-projection starfield: stars have a 3D position (x, y, z)
// and are projected onto the 2D screen with factor = FOV / z.  z decrements
// each frame so stars fly toward the viewer; when z ≤ 0 the star is respawned
// far away at z = MAX_DEPTH.  Brightness = 1 − z/MAX_DEPTH, so dim/distant
// stars are only rendered once they cross a visibility threshold.

const N_OLED_STARS: usize = 60;
const MAX_DEPTH: f32 = 32.0;
/// Depth units consumed per 50 ms frame.
const Z_STEP: f32 = 0.4;
/// Field of view (radians) — matches the reference implementation.
const FOV: f32 = core::f32::consts::PI;
/// Projected screen centre.
const CX: f32 = 64.0;
const CY: f32 = 26.0; // (TOPMOST + HEIGHT) / 2

#[derive(Clone, Copy)]
struct OledStar {
    x: f32, // 3D position, range −128 … +128
    y: f32, // 3D position, range  −43 …  +43
    z: f32, // depth, range 0 … MAX_DEPTH
}

#[inline(always)]
fn lcg(s: &mut u32) -> u32 {
    *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *s
}

/// LCG output mapped to [0, 1).
#[inline(always)]
fn lcg_f32(s: &mut u32) -> f32 {
    lcg(s) as f32 * (1.0 / 4_294_967_296.0)
}

fn spawn_star(star: &mut OledStar, rng: &mut u32, z: f32) {
    // Wide 3D range so that at z=MAX_DEPTH (factor≈0.098) stars project across
    // the full canvas: x=±512 → ±50 px from centre; y=±200 → ±19 px from centre.
    star.x = lcg_f32(rng) * 512.0 - 256.0; // −256 … +256
    star.y = lcg_f32(rng) * 200.0 - 100.0; // −100 … +100
    star.z = z;
}

fn init_oled_stars(stars: &mut [OledStar; N_OLED_STARS], rng: &mut u32) {
    for star in stars.iter_mut() {
        // Scatter across the full depth range so the field is populated immediately.
        let z = lcg_f32(rng) * MAX_DEPTH + 0.1;
        spawn_star(star, rng, z);
    }
}

fn render_starscape(
    stars: &mut [OledStar; N_OLED_STARS],
    rng: &mut u32,
    fb: &mut oled::FrameBuffer,
) {
    fb.fill(0x00);

    for star in stars.iter_mut() {
        star.z -= Z_STEP;

        if star.z <= 0.0 {
            spawn_star(star, rng, MAX_DEPTH);
            continue;
        }

        let factor = FOV / star.z;
        let sx = (star.x * factor + CX) as i32;
        // Negate y so that positive-y is up on screen, matching the reference.
        let sy = (-star.y * factor + CY) as i32;

        // Close stars (z < ~40% of MAX_DEPTH) draw as 2×2 blocks, matching the
        // reference's radius growth from 0→1.7 as z→0.
        let size: i32 = if star.z < MAX_DEPTH * 0.4 { 2 } else { 1 };
        for dy in 0..size {
            for dx in 0..size {
                let px = sx + dx;
                let py = sy + dy;
                if px >= 0
                    && px < oled::WIDTH as i32
                    && py >= TOPMOST as i32
                    && py < oled::HEIGHT as i32
                {
                    fb.set_pixel(px as usize, py as usize, true);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

/// OLED render task.
///
/// Initialises the SSD1309 then loops, choosing a render mode based on state:
///
/// - **Audio streaming** (`USB_AUDIO_STREAMING`): oscilloscope waveform,
///   redrawn by `analysis_task` via [`oled::notify_redraw`].
/// - **Idle**: starscape screensaver — stars fly outward from the centre via
///   perspective projection, animating at 50 ms / frame.
/// - **Pad state** (any other pad change): pad cell map, redrawn on demand.
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

    let mut rng: u32 = 0xCAFE_5678;
    let mut stars = [OledStar {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    }; N_OLED_STARS];
    init_oled_stars(&mut stars, &mut rng);

    // Render the initial (empty) state immediately without waiting for a pad press.
    render_pads(&mut fb);
    oled::send_frame(&fb).await;

    loop {
        let streaming = USB_AUDIO_STREAMING.load(Ordering::Acquire);

        if !streaming {
            // ── Starscape screensaver — timer-driven at 50 ms/frame ───────────
            render_starscape(&mut stars, &mut rng, &mut fb);
            oled::send_frame(&fb).await;
            Timer::after_millis(50).await;
        } else {
            // ── Event-driven modes ────────────────────────────────────────────
            oled::wait_redraw().await;

            if USB_AUDIO_STREAMING.load(Ordering::Acquire) {
                render_waveform(&mut fb);
            } else {
                render_pads(&mut fb);
            }
            oled::send_frame(&fb).await;
        }
    }
}
