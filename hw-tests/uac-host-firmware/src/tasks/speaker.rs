//! Hosted UAC capture → the Deluge's codec (speaker and outputs), through the SCUX.
//!
//! Captured frames arrive at the device's rate (48 kHz from a Digitakt). This task writes them
//! into the SCUX DVU path's ring a fixed lead ahead of its DMA, and the SCUX 2SRC converts them
//! to the codec's 44.1 kHz in hardware. The task pulls at exactly the rate the SCUX consumes,
//! so the class driver's capture ring absorbs the drift between the device's clock and the
//! Deluge's, and its resampler trims it (a ratio near 1).

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use deluge_bsp::ring_follow::{self, WriteAhead};
use deluge_bsp::sample_fmt::f32_to_i32;
use deluge_bsp::scux_dvu_path as dvu;
use embassy_time::{Instant, Timer};
use log::info;
use usb_classes::host::uac;

// ── Shared status (read by the OLED task) ────────────────────────────────
pub(crate) const STAGE_WAITING: u8 = 0;
pub(crate) const STAGE_STREAMING: u8 = 1;

pub(crate) static STAGE: AtomicU8 = AtomicU8::new(STAGE_WAITING);
pub(crate) static CAP_CH: AtomicU8 = AtomicU8::new(0);
pub(crate) static PLAY_CH: AtomicU8 = AtomicU8::new(0);
/// Input peak over the last window, in milli-units of full scale (0..=1000).
pub(crate) static PEAK_MILLI: AtomicU32 = AtomicU32::new(0);

const MAX_CH: usize = 8;
/// Distance of the write head ahead of the SCUX's DMA, in frames (~10.7 ms at 48 kHz).
const LEAD_FRAMES: usize = 512;
/// Frames copied per capture read.
const CHUNK_FRAMES: usize = 128;

/// The SCUX DMA's read slot in the DVU ring.
fn read_slot() -> usize {
    // SAFETY: both point into the DVU ring (uncached alias). CRSA can briefly read
    // one-past-the-end at the link-descriptor reload; the modulo folds it back.
    (unsafe { dvu::tx_current_ptr().offset_from(dvu::tx_buf_start()) } as usize)
        % dvu::DVU_PATH_BUF_LEN
}

#[embassy_executor::task]
pub(crate) async fn speaker_task() {
    info!("speaker: running (UAC capture → SCUX 2SRC → codec)");
    let ring = dvu::tx_buf_start();
    let len = dvu::DVU_PATH_BUF_LEN;
    let mut wr = ((read_slot() + LEAD_FRAMES * 2) % len) & !1;
    let mut scratch = [0.0f32; CHUNK_FRAMES * MAX_CH];
    let (mut in_frames, mut silent_frames, mut reanchors, mut window_peak) =
        (0u32, 0u32, 0u32, 0u32);
    let mut last_stats = Instant::now();
    let mut was_connected = false;
    // Frames the SCUX consumed: it should match the device's rate (48 kHz), not the codec's.
    let mut prev_read = read_slot();
    let mut dma_slots: u32 = 0;

    loop {
        let cap_ch = uac::capture_channels() as usize;
        if cap_ch == 0 {
            if was_connected {
                info!("speaker: device disconnected");
                was_connected = false;
            }
            STAGE.store(STAGE_WAITING, Ordering::Relaxed);
            CAP_CH.store(0, Ordering::Relaxed);
            PLAY_CH.store(0, Ordering::Relaxed);
            Timer::after_millis(100).await;
            continue;
        }
        if !was_connected {
            info!("speaker: device connected, {} capture channels", cap_ch);
            was_connected = true;
        }
        STAGE.store(STAGE_STREAMING, Ordering::Relaxed);
        CAP_CH.store(cap_ch as u8, Ordering::Relaxed);
        PLAY_CH.store(uac::playback_channels(), Ordering::Relaxed);

        let now_read = read_slot();
        dma_slots += ((now_read + len - prev_read) % len) as u32;
        prev_read = now_read;
        let mut to_write = match ring_follow::plan(now_read, wr, len, LEAD_FRAMES) {
            WriteAhead::Write(n) => n,
            WriteAhead::Reanchor(slot) => {
                reanchors += 1;
                wr = slot;
                LEAD_FRAMES
            }
        };
        while to_write > 0 {
            let frames = to_write.min(CHUNK_FRAMES);
            let got = uac::capture_read(&mut scratch[..frames * cap_ch]) / cap_ch;
            for f in 0..frames {
                // Left and right from the first two channels (mono duplicated); silence once
                // the capture runs short.
                let (l, r) = if f < got {
                    let s = &scratch[f * cap_ch..f * cap_ch + cap_ch];
                    (s[0], if cap_ch > 1 { s[1] } else { s[0] })
                } else {
                    (0.0, 0.0)
                };
                let p = (l.abs().max(r.abs()).min(1.0) * 1000.0) as u32;
                window_peak = window_peak.max(p);
                // SAFETY: `wr` and `wr + 1` are in-bounds slots of the uncached DVU ring,
                // which only this task writes.
                unsafe {
                    ring.add(wr).write_volatile(f32_to_i32(l));
                    ring.add(wr + 1).write_volatile(f32_to_i32(r));
                }
                wr = (wr + 2) % len;
            }
            in_frames += got as u32;
            silent_frames += (frames - got) as u32;
            to_write -= frames;
        }

        if last_stats.elapsed().as_millis() >= 1000 {
            PEAK_MILLI.store(window_peak, Ordering::Relaxed);
            info!(
                "speaker: in={}f/s scux={}f/s silence={}f/s reanchors={} peak={}m/1000 ({}ch)",
                in_frames,
                dma_slots / 2,
                silent_frames,
                reanchors,
                window_peak,
                cap_ch
            );
            dma_slots = 0;
            (in_frames, silent_frames, reanchors, window_peak) = (0, 0, 0, 0);
            last_stats = Instant::now();
        }

        Timer::after_millis(1).await;
    }
}
