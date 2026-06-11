use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use deluge_bsp::scux_dvu_path;
use embassy_usb::driver::{Endpoint as _, EndpointIn as _};
use log::info;
use rza1l_hal::ssi;

use deluge_bsp::usb::classes::audio::{USB_BITS_PER_SAMPLE, USB_CAPTURE_BITS_PER_SAMPLE};

/// `true` while the host is actively sending UAC2 speaker data.
///
/// This is the UI-facing signal for whether the analyzer views should be
/// shown. It deliberately tracks actual USB audio traffic, not whether the
/// SCUX/SSI output path is running with dither.
///
/// Set by the ISO OUT hook ([`iso_out_to_ssi`]) on the first packet of a stream
/// and cleared by [`uac2_task`] when packet activity stops.
pub(crate) static USB_AUDIO_STREAMING: AtomicBool = AtomicBool::new(false);

/// How far ahead of the DMA read head the hook keeps the write pointer (mono
/// i32 slots).  2048 slots = 1024 stereo frames ≈ 23.2 ms at 44.1 kHz — sized to
/// absorb SCUX FFD FIFO burst DMA and USB SOF jitter comfortably.
const WRITE_AHEAD: usize = 2048;

/// Bumped by the ISO OUT hook on every received packet.  The supervisor task
/// polls this to detect when the host has started or stopped streaming without
/// participating in the (latency-critical) data path.
static PACKET_SEQ: AtomicU32 = AtomicU32::new(0);

/// Data-path state owned exclusively by the ISO OUT BRDY ISR (single producer,
/// IRQs disabled — no locking needed).  `HOOK_WRITE_PTR` is the next SSI TX slot
/// to write; `HOOK_LFSR` is the dither generator state carried across packets.
static mut HOOK_WRITE_PTR: *mut i32 = core::ptr::null_mut();
static mut HOOK_LFSR: u32 = 0xACE1;

// ── Diagnostics ──────────────────────────────────────────────────────────────
// Updated by the ISO OUT hook (ISR), drained + logged once a second by the
// supervisor task.  The ISR cannot log (RTT is task-only and compiled out in
// release), so it accumulates into atomics instead.

/// Count of samples whose value + dither saturated at the i32 rail this second.
/// A non-zero value means the host is sending full-scale ("hot") audio — see
/// [`iso_out_to_ssi`], where saturation replaces what would otherwise be a
/// sign-flipping wraparound (a loud click).
static CLIP_COUNT: AtomicU32 = AtomicU32::new(0);
/// Count of underrun re-anchors this second (DMA caught up to the write head).
static REANCHOR_COUNT: AtomicU32 = AtomicU32::new(0);
/// Peak absolute sample magnitude (pre-dither, 32-bit MSB-aligned) this second.
/// Full scale is `0x8000_0000`; compare against it to gauge headroom.
static PEAK_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Small LFSR dither to prevent codec DC auto-mute.
///
/// The Akiyama codec auto-mutes after ~8192 consecutive identical samples
/// (≈0.19 s at 44.1 kHz).  We mix a ±16-LSB (of 24-bit) noise signal into
/// every sample written to the SSI TX buffer to keep the codec awake during
/// silence.  This matches the behaviour of the original C firmware.
#[inline]
fn dither_sample(lfsr: &mut u32) -> i32 {
    let bit = *lfsr & 1;
    *lfsr >>= 1;
    if bit != 0 {
        *lfsr ^= 0xB400;
    }
    // ±16 LSBs in the 24-bit range: noise in bits [12:8] of the 32-bit word.
    ((*lfsr & 0x1F) as i32 - 0x10) << 8
}

/// Fill the entire SSI TX buffer with low-level dither noise.
///
/// Called once at startup and after the USB stream ends to ensure the codec
/// never sees a burst of identical samples.
pub(crate) fn fill_tx_with_dither() {
    let buf_start = scux_dvu_path::tx_buf_start();
    let buf_end = scux_dvu_path::tx_buf_end();
    let mut lfsr: u32 = 0xACE1;
    let mut p = buf_start;
    while p < buf_end {
        unsafe {
            p.write_volatile(dither_sample(&mut lfsr));
            p = p.add(1);
        }
    }
}

/// ISO OUT BRDY hook — runs in **IRQ context** for every speaker packet.
///
/// Converts the host's interleaved stereo PCM into MSB-aligned 32-bit SSI words
/// (with anti-auto-mute dither) and writes them into the SCUX/DVU TX ring a
/// fixed [`WRITE_AHEAD`] lead ahead of the DMA read head.  Doing this in the ISR
/// rather than a task means a slow render/FFT pass can never delay audio
/// servicing and overrun the double-buffered ISO FIFO (which would drop the
/// packet outright — TRM §28.4.9 / Table 28.26).
///
/// Packet format: stereo 16- or 24-bit LE PCM.  SSI expects audio in bits
/// [31:8]; 24-bit shifts left by 8, 16-bit by 16.
///
/// # Safety
/// Installed via [`rza1l_hal::usb::pipe::register_iso_out_hook`] and called
/// only from the BRDY ISR (single producer, IRQs disabled).  `pkt` points to
/// `len` valid bytes.  Uses integer arithmetic only — no VFP state to preserve.
unsafe fn iso_out_to_ssi(pkt: *const u8, len: usize) {
    unsafe {
        let buf_start = scux_dvu_path::tx_buf_start();
        let buf_end = scux_dvu_path::tx_buf_end();
        let buf_len = scux_dvu_path::DVU_PATH_BUF_LEN;

        let dma_ptr = scux_dvu_path::tx_current_ptr();
        // CRSA can briefly read one-past-the-end during the DMA link-descriptor
        // reload at the buffer wrap boundary.  Wrap into [0, buf_len) so the
        // ahead calculation below doesn't see a spuriously small value.
        let dma_off = (dma_ptr.offset_from(buf_start) as usize) % buf_len;

        let mut write_ptr = HOOK_WRITE_PTR;

        // First packet after silence — re-anchor the write pointer WRITE_AHEAD
        // slots ahead of the DMA, snapped to a stereo frame boundary (even
        // offset) to keep L/R channels correct.  USB_AUDIO_STREAMING is cleared
        // by uac2_task when packet activity stops.
        if !USB_AUDIO_STREAMING.load(Ordering::Acquire) {
            let off = ((dma_off + WRITE_AHEAD) % buf_len) & !1;
            write_ptr = buf_start.add(off);
            USB_AUDIO_STREAMING.store(true, Ordering::Release);
        }

        // Underrun guard: if the DMA has caught up to the write pointer, re-anchor.
        let wr_off = write_ptr.offset_from(buf_start) as usize;
        let ahead = (wr_off + buf_len - dma_off) % buf_len;
        if ahead < WRITE_AHEAD / 2 {
            let off = ((dma_off + WRITE_AHEAD) % buf_len) & !1;
            write_ptr = buf_start.add(off);
            REANCHOR_COUNT.fetch_add(1, Ordering::Relaxed);
        }

        // ── Convert USB samples → MSB-aligned 32-bit SSI, mixing in dither ──
        let bytes_per_sample = (USB_BITS_PER_SAMPLE.load(Ordering::Relaxed) / 8) as usize;
        if bytes_per_sample != 0 {
            let mut lfsr = HOOK_LFSR;
            let mut clips = 0u32;
            let mut peak = 0u32;
            let num_samples = len / bytes_per_sample;
            for i in 0..num_samples {
                let raw = if bytes_per_sample == 3 {
                    let b0 = *pkt.add(i * 3) as u32;
                    let b1 = *pkt.add(i * 3 + 1) as u32;
                    let b2 = *pkt.add(i * 3 + 2) as u32;
                    (b0 << 8 | b1 << 16 | b2 << 24) as i32
                } else {
                    // 16-bit signed LE → MSB-align in 32 bits
                    let v = (*pkt.add(i * 2) as u16 | (*pkt.add(i * 2 + 1) as u16) << 8) as i16;
                    (v as i32) << 16
                };

                peak = peak.max(raw.unsigned_abs());

                // Mix in dither with SATURATION, not wraparound.  A near-full-scale
                // sample (e.g. 24-bit 0x7FFFFF → 0x7FFFFF00, only 255 below i32::MAX)
                // plus up to +0xF00 of dither would overflow i32 and flip sign with
                // wrapping_add — a rail-to-rail discontinuity heard as a loud click
                // on "hot" (0 dBFS) material.  saturating_add clamps instead; the
                // few clamped LSBs are inaudible.
                let dith = dither_sample(&mut lfsr);
                let sample = raw.saturating_add(dith);
                if sample != raw.wrapping_add(dith) {
                    clips += 1;
                }

                write_ptr.write_volatile(sample);
                write_ptr = write_ptr.add(1);
                if write_ptr >= buf_end {
                    write_ptr = buf_start;
                }
            }
            HOOK_LFSR = lfsr;
            if clips != 0 {
                CLIP_COUNT.fetch_add(clips, Ordering::Relaxed);
            }
            PEAK_LEVEL.fetch_max(peak, Ordering::Relaxed);
        }

        HOOK_WRITE_PTR = write_ptr;
        PACKET_SEQ.fetch_add(1, Ordering::Relaxed);
    }
}

/// UAC2 speaker supervisor task.
///
/// The latency-critical data path lives entirely in [`iso_out_to_ssi`], invoked
/// from the BRDY ISR.  This task installs that hook and then only handles the
/// slow, non-latency-critical policy by watching [`PACKET_SEQ`] for activity:
///
/// - `SPEAKER_ENABLE` (P4.1) is raised when a stream becomes active AND no
///   headphone or line-out jack is inserted (matching the C firmware).
/// - When packets stop arriving it drops `SPEAKER_ENABLE`, clears
///   [`USB_AUDIO_STREAMING`] (so the hook re-anchors on resume), and refills the
///   TX ring with dither so the codec does not auto-mute.
#[embassy_executor::task]
pub(crate) async fn uac2_task(ep_out: rza1l_hal::usb::Rusb1EndpointOut) {
    /// Activity poll interval.  At 8000 packets/s an active stream bumps
    /// PACKET_SEQ hundreds of times per tick, so a single tick with no change
    /// unambiguously means the host has stopped — silence follows within ≤2
    /// ticks of a disconnect.
    const POLL_MS: u64 = 25;

    // Pre-fill with dither so the codec stays alive before streaming begins, and
    // anchor the hook's write pointer somewhere valid before its first packet.
    fill_tx_with_dither();
    unsafe { HOOK_WRITE_PTR = scux_dvu_path::tx_buf_start() };

    // Install the ISR fast path.  BRDY is armed for the ISO OUT pipe whenever the
    // endpoint is enabled (driver `endpoint_set_enabled`), so once the hook is
    // registered every packet is serviced in interrupt context.
    unsafe {
        rza1l_hal::usb::pipe::register_iso_out_hook(ep_out.pipe as usize, iso_out_to_ssi);
    }
    info!("uac2_task: ISO OUT hook installed; supervising stream state");

    let mut ticker = embassy_time::Ticker::every(embassy_time::Duration::from_millis(POLL_MS));
    let mut last_seq = PACKET_SEQ.load(Ordering::Relaxed);
    let mut ui_streaming = false;
    // Emit a diagnostics line once per second (1000 ms / POLL_MS ticks).
    let mut diag_ticks: u32 = 0;

    loop {
        ticker.next().await;
        let seq = PACKET_SEQ.load(Ordering::Relaxed);
        let active = seq != last_seq;
        last_seq = seq;

        // ── Per-second audio health diagnostics ───────────────────────────────
        // clips  : samples that hit the i32 rail (host sending 0 dBFS material).
        //          Audible crackle on hot songs almost always shows up here.
        // reanch : underrun re-anchors (write head lost its lead on the DMA) —
        //          a USB/scheduling/clock-drift symptom, distinct from clipping.
        // peak   : loudest sample magnitude vs full scale (0x8000_0000).
        diag_ticks += 1;
        if diag_ticks >= (1000 / POLL_MS as u32) {
            diag_ticks = 0;
            if ui_streaming {
                let clips = CLIP_COUNT.swap(0, Ordering::Relaxed);
                let reanch = REANCHOR_COUNT.swap(0, Ordering::Relaxed);
                let peak = PEAK_LEVEL.swap(0, Ordering::Relaxed);
                // peak_pct = peak / full-scale × 100, computed without floats.
                let peak_pct = ((peak as u64 * 100) >> 31) as u32;
                info!(
                    "uac2: clips={} reanchors={} peak={:#010x} ({}% FS)",
                    clips, reanch, peak, peak_pct
                );
            } else {
                // Not streaming — keep the counters from going stale across a stop.
                CLIP_COUNT.store(0, Ordering::Relaxed);
                REANCHOR_COUNT.store(0, Ordering::Relaxed);
                PEAK_LEVEL.store(0, Ordering::Relaxed);
            }
        }

        if active && !ui_streaming {
            ui_streaming = true;
            // Gate speaker: on only if no headphone / line-out is inserted.
            let hp = unsafe { rza1l_hal::gpio::read_pin(6, 5) };
            let lol = unsafe { rza1l_hal::gpio::read_pin(6, 3) };
            let lor = unsafe { rza1l_hal::gpio::read_pin(6, 4) };
            unsafe { rza1l_hal::gpio::write(4, 1, !hp && !lol && !lor) };
            info!("uac2_task: streaming started");
        } else if !active && ui_streaming {
            ui_streaming = false;
            // Clear streaming so the hook re-anchors on the next packet, drop
            // SPEAKER_ENABLE, and refill the ring with dither.  The host has
            // stopped, so no hook write races this fill.
            USB_AUDIO_STREAMING.store(false, Ordering::Release);
            unsafe { rza1l_hal::gpio::write(4, 1, false) }; // SPEAKER_ENABLE off
            fill_tx_with_dither();
            info!("uac2_task: stream stopped");
        }
    }
}

/// UAC2 microphone capture task — reads from SSI RX and sends over ISO IN.
///
/// The ISO IN packet cadence provides **implicit feedback** for the speaker
/// stream: the host observes the IN packet rate and adapts how much data it
/// sends per SOF, correcting long-term clock drift without a separate feedback
/// endpoint.
#[embassy_executor::task]
pub(crate) async fn uac2_mic_task(mut ep_in: rza1l_hal::usb::Rusb1EndpointIn) {
    info!("uac2_mic_task: waiting for capture enable");
    ep_in.wait_enabled().await;
    info!("uac2_mic_task: capture enabled");

    let rx_start = ssi::rx_buf_start();
    let rx_len = ssi::RX_BUF_LEN;
    let mut read_ptr = ssi::rx_current_ptr();

    let mut pkt = [0u8; 288];

    loop {
        // Compute how many stereo frames the SSI RX DMA has captured since we
        // last sent.  This ties the implicit feedback signal directly to the
        // hardware AUDIO_X1 crystal clock rather than an assumed call rate,
        // so the host adapts its OUT rate to exactly match the SSI regardless
        // of async executor scheduling jitter.
        //
        // Over any long interval: total IN frames sent = total SSI frames
        // captured = 44 100 Hz.  Host converges to sending 44 100 frames/sec
        // OUT, eliminating the systematic rate mismatch that caused underruns.
        let bytes_per_sample = (USB_CAPTURE_BITS_PER_SAMPLE.load(Ordering::Relaxed) / 8) as usize;
        let max_frames = pkt.len() / (2 * bytes_per_sample);
        let rx_hw_off = unsafe { (ssi::rx_current_ptr().offset_from(rx_start) as usize) % rx_len };
        let read_off = unsafe { read_ptr.offset_from(rx_start) as usize };
        let captured_mono = (rx_hw_off + rx_len - read_off) % rx_len;
        // Integer divide by 2 for stereo frames; capped so we never exceed the
        // packet buffer.  The fractional remainder carries naturally into the
        // next call via the unchanged read_ptr, giving correct Bresenham-style
        // alternating 5/6 frame packets averaging exactly 44 100 Hz.
        let frames = (captured_mono / 2).min(max_frames);

        // Read `frames` stereo pairs from SSI RX; convert MSB-aligned i32 → USB PCM.
        // Format matches the active capture alt setting (16-bit or 24-bit LE).
        let nbytes = frames * 2 * bytes_per_sample;
        for i in 0..frames * 2 {
            let sample = unsafe { read_ptr.read_volatile() };
            // SSI audio is in bits [31:8]; shift right to get the significant bits.
            let off = i * bytes_per_sample;
            if bytes_per_sample == 3 {
                let val = (sample >> 8) as u32;
                pkt[off] = (val & 0xFF) as u8;
                pkt[off + 1] = ((val >> 8) & 0xFF) as u8;
                pkt[off + 2] = ((val >> 16) & 0xFF) as u8;
            } else {
                // 16-bit: keep the top 16 bits of the MSB-aligned sample
                let val = (sample >> 16) as u16;
                pkt[off] = (val & 0xFF) as u8;
                pkt[off + 1] = ((val >> 8) & 0xFF) as u8;
            }
            unsafe {
                read_ptr = read_ptr.add(1);
                if read_ptr >= rx_start.add(rx_len) {
                    read_ptr = rx_start;
                }
            }
        }

        match ep_in.write(&pkt[..nbytes]).await {
            Ok(()) => {}
            Err(_) => {
                info!("uac2_mic_task: capture stopped");
                ep_in.wait_enabled().await;
                info!("uac2_mic_task: capture re-enabled");
                // Re-anchor behind current DMA write position.
                read_ptr = ssi::rx_current_ptr();
            }
        }
    }
}
