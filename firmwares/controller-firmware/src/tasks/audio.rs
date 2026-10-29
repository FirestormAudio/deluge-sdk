use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use deluge_bsp::audio;
use log::info;

use rza1_hal::usb::iso::{IsoInProducer, IsoOutConsumer};
use usb_classes::audio::{USB_BITS_PER_SAMPLE, USB_CAPTURE_BITS_PER_SAMPLE};

/// `true` while the host is actively sending UAC2 speaker data.
///
/// Tracks actual USB audio traffic, not whether the SCUX/SSI output path is
/// running with dither.
///
/// Set by the ISO OUT consumer ([`SpeakerSink`]) on the first packet of a stream
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

/// The ISO OUT stream's consumer: its state is touched only by the USB interrupt
/// (single producer, IRQs disabled — no locking needed).  `write_off` is the next
/// TX ring word to write; `lfsr` is the dither generator state carried across
/// packets.
struct SpeakerSink {
    write_off: usize,
    lfsr: u32,
}

/// The speaker consumer, lent once to the ISO OUT stream by [`uac2_task`].
static mut SPEAKER: SpeakerSink = SpeakerSink {
    write_off: 0,
    lfsr: 0xACE1,
};

// ── Diagnostics ──────────────────────────────────────────────────────────────
// Updated by the ISO OUT hook (ISR), drained + logged once a second by the
// supervisor task.  The ISR cannot log (RTT is task-only and compiled out in
// release), so it accumulates into atomics instead.

/// Count of samples whose value + dither saturated at the i32 rail this second.
/// A non-zero value means the host is sending full-scale ("hot") audio — see
/// [`SpeakerSink`], where saturation replaces what would otherwise be a
/// sign-flipping wraparound (a loud click).
static CLIP_COUNT: AtomicU32 = AtomicU32::new(0);
/// Count of underrun re-anchors this second (DMA caught up to the write head).
static REANCHOR_COUNT: AtomicU32 = AtomicU32::new(0);
/// Peak absolute sample magnitude (pre-dither, 32-bit MSB-aligned) this second.
/// Full scale is `0x8000_0000`; compare against it to gauge headroom.
static PEAK_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Small LFSR dither to prevent codec DC auto-mute.
///
/// The codec auto-mutes after ~8192 consecutive identical samples (≈0.19 s at
/// 44.1 kHz), so a ±16-LSB (of 24-bit) noise signal is mixed into every sample
/// written to the SSI TX buffer to keep it awake during silence, as the C
/// firmware does.
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
    audio::fill_tx_with_dither();
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
/// `[31:8]`; 24-bit shifts left by 8, 16-bit by 16.
///
/// Runs in the BRDY ISR (single producer, IRQs disabled), bound by
/// [`uac2_task`]'s iso stream.  Uses integer arithmetic only — no VFP state to
/// preserve.
impl IsoOutConsumer for SpeakerSink {
    fn on_packet(&mut self, packet: &[u8]) {
        let (pkt, len) = (packet.as_ptr(), packet.len());
        audio::with_tx(|tx| unsafe {
            let buf_len = tx.len();
            // The DMA's read position, already wrapped into [0, buf_len) and frame-aligned.
            let dma_off = tx.position();
            let ring = tx.ring_mut();

            let mut write_off = self.write_off;

            // First packet after silence — re-anchor the write pointer WRITE_AHEAD
            // slots ahead of the DMA, snapped to a stereo frame boundary (even
            // offset) to keep L/R channels correct.  USB_AUDIO_STREAMING is cleared
            // by uac2_task when packet activity stops.
            if !USB_AUDIO_STREAMING.load(Ordering::Acquire) {
                write_off = ((dma_off + WRITE_AHEAD) % buf_len) & !1;
                USB_AUDIO_STREAMING.store(true, Ordering::Release);
            }

            // Underrun guard: if the DMA has caught up to the write offset, re-anchor.
            let ahead = (write_off + buf_len - dma_off) % buf_len;
            if ahead < WRITE_AHEAD / 2 {
                write_off = ((dma_off + WRITE_AHEAD) % buf_len) & !1;
                REANCHOR_COUNT.fetch_add(1, Ordering::Relaxed);
            }

            // ── Convert USB samples → MSB-aligned 32-bit SSI, mixing in dither ──
            let bytes_per_sample = (USB_BITS_PER_SAMPLE.load(Ordering::Relaxed) / 8) as usize;
            if let Some(num_samples) = len.checked_div(bytes_per_sample) {
                let mut lfsr = self.lfsr;
                let mut clips = 0u32;
                let mut peak = 0u32;
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

                    // Mix in dither with saturation: a near-full-scale sample plus
                    // dither can overflow i32, and wrapping would flip the sign — a
                    // rail-to-rail click on 0 dBFS material.  The clamped LSBs are
                    // inaudible.
                    let dith = dither_sample(&mut lfsr);
                    let sample = raw.saturating_add(dith);
                    if sample != raw.wrapping_add(dith) {
                        clips += 1;
                    }

                    ring[write_off] = sample as u32;
                    write_off = (write_off + 1) % buf_len;
                }
                self.lfsr = lfsr;
                if clips != 0 {
                    CLIP_COUNT.fetch_add(clips, Ordering::Relaxed);
                }
                PEAK_LEVEL.fetch_max(peak, Ordering::Relaxed);
            }

            self.write_off = write_off;
            PACKET_SEQ.fetch_add(1, Ordering::Relaxed);
        });
    }
}

/// UAC2 speaker supervisor task.
///
/// The latency-critical data path lives entirely in [`SpeakerSink`], called from
/// the BRDY ISR.  This task binds it to the ISO OUT stream and then only handles the
/// slow, non-latency-critical policy by watching [`PACKET_SEQ`] for activity:
///
/// - `SPEAKER_ENABLE` (P4.1) is raised when a stream becomes active AND no
///   headphone or line-out jack is inserted (matching the C firmware).
/// - When packets stop arriving it drops `SPEAKER_ENABLE`, clears
///   [`USB_AUDIO_STREAMING`] (so the sink re-anchors on resume), and refills the
///   TX ring with dither so the codec does not auto-mute.
#[embassy_executor::task]
pub(crate) async fn uac2_task(ep_out: rza1_hal::usb::Rusb1EndpointOut) {
    /// Activity poll interval.  At 8000 packets/s an active stream bumps
    /// PACKET_SEQ hundreds of times per tick, so a single tick with no change
    /// unambiguously means the host has stopped — silence follows within ≤2
    /// ticks of a disconnect.
    const POLL_MS: u64 = 25;

    // Pre-fill with dither so the codec stays alive before streaming begins, and
    // anchor the hook's write pointer somewhere valid before its first packet.
    fill_tx_with_dither();

    // Bind the ISR fast path.  BRDY is armed for the ISO OUT pipe whenever the
    // endpoint is enabled (driver `endpoint_set_enabled`), so once the stream is
    // bound every packet is serviced in interrupt context.  The stream lives as
    // long as this task (forever).
    // SAFETY: SPEAKER is lent once, here; only the stream's interrupt uses it after.
    let speaker: &'static mut SpeakerSink = unsafe { &mut *core::ptr::addr_of_mut!(SPEAKER) };
    let _stream = ep_out.into_iso_stream(speaker);
    info!("uac2_task: ISO OUT stream bound; supervising stream state");

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
            use deluge_bsp::jacks::{self, Jack};
            let hp = jacks::is_inserted(Jack::Headphone);
            let lol = jacks::is_inserted(Jack::LineOutL);
            let lor = jacks::is_inserted(Jack::LineOutR);
            jacks::set_speaker_enable(!hp && !lol && !lor);
            info!("uac2_task: streaming started");
        } else if !active && ui_streaming {
            ui_streaming = false;
            // Clear streaming so the hook re-anchors on the next packet, drop
            // SPEAKER_ENABLE, and refill the ring with dither.  The host has
            // stopped, so no hook write races this fill.
            USB_AUDIO_STREAMING.store(false, Ordering::Release);
            deluge_bsp::jacks::set_speaker_enable(false);
            fill_tx_with_dither();
            info!("uac2_task: stream stopped");
        }
    }
}

// ── ISO IN (mic) continuous-hook state ─────────────────────────────────────
/// The ISO IN stream's producer: `read_off` is the SSI RX read offset carried
/// across packets (mirrors [`SpeakerSink`]'s `write_off`).
struct MicSource {
    read_off: usize,
}

/// The mic producer, lent once to the ISO IN stream by [`uac2_mic_task`].
static mut MIC: MicSource = MicSource { read_off: 0 };
/// False until the first packet of a capture stream anchors `read_off`; the
/// supervisor clears it when packet activity stops so the next stream re-anchors.
static MIC_STREAMING: AtomicBool = AtomicBool::new(false);
/// Bumped once per ISO IN packet — the supervisor watches it for activity.
static MIC_PACKET_SEQ: AtomicU32 = AtomicU32::new(0);

/// Fill the next ISO IN (mic) packet from the SSI RX ring, MSB-aligned i32 →
/// USB PCM (stereo 16- or 24-bit LE, per the active capture alt).  Returns the
/// packet length in bytes.
///
/// Bound to the ISO IN stream and called from the BRDY ISR each time a transmit
/// buffer frees up (and twice at enable, to prime both buffers) — the
/// continuous-BUF equivalent of the OUT-side [`SpeakerSink`].  Runs with IRQs
/// disabled; integer arithmetic only — no VFP state to preserve.
impl IsoInProducer for MicSource {
    fn fill_packet(&mut self, packet: &mut [u8]) -> usize {
        let (buf, max) = (packet.as_mut_ptr(), packet.len());
        audio::with_rx(|rx| unsafe {
            let rx_len = rx.len();
            let bytes_per_sample =
                (USB_CAPTURE_BITS_PER_SAMPLE.load(Ordering::Relaxed) / 8) as usize;
            if bytes_per_sample == 0 {
                return 0;
            }
            let max_frames = max / (2 * bytes_per_sample);
            // The DMA's write position, wrapped and frame-aligned.
            let rx_hw_off = rx.position();

            // First packet of a (re)started stream: anchor at the current DMA write
            // head.  captured_mono then grows as the DMA advances; the read offset
            // only ever consumes what the DMA has already written, so it can never
            // overtake the writer.
            let mut read_off = self.read_off;
            if !MIC_STREAMING.load(Ordering::Acquire) {
                read_off = rx_hw_off;
                MIC_STREAMING.store(true, Ordering::Release);
            }

            // Stereo frames captured since the last packet (ties the implicit-feedback
            // cadence to the SSI crystal clock).
            let captured_mono = (rx_hw_off + rx_len - read_off) % rx_len;
            let frames = (captured_mono / 2).min(max_frames);

            let ring = rx.ring();
            for i in 0..frames * 2 {
                let sample = ring[read_off] as i32;
                // SSI audio is in bits [31:8]; shift right to get the significant bits.
                let off = i * bytes_per_sample;
                if bytes_per_sample == 3 {
                    let val = (sample >> 8) as u32;
                    *buf.add(off) = (val & 0xFF) as u8;
                    *buf.add(off + 1) = ((val >> 8) & 0xFF) as u8;
                    *buf.add(off + 2) = ((val >> 16) & 0xFF) as u8;
                } else {
                    let val = (sample >> 16) as u16;
                    *buf.add(off) = (val & 0xFF) as u8;
                    *buf.add(off + 1) = ((val >> 8) & 0xFF) as u8;
                }
                read_off = (read_off + 1) % rx_len;
            }

            self.read_off = read_off;
            MIC_PACKET_SEQ.fetch_add(1, Ordering::Relaxed);
            frames * 2 * bytes_per_sample
        })
        .unwrap_or(0)
    }
}

/// UAC2 microphone capture task — binds the ISO IN stream and supervises.
///
/// The ISO IN packet cadence provides **implicit feedback** for the speaker
/// stream (the host observes the IN packet rate and adapts its OUT rate).  The
/// packets are staged in the BRDY ISR by [`MicSource`] (continuous BUF
/// mode), so a busy executor can never stall servicing; a per-packet `write()`
/// from a task cannot keep up with the microframe cadence.
#[embassy_executor::task]
pub(crate) async fn uac2_mic_task(ep_in: rza1_hal::usb::Rusb1EndpointIn) {
    let (pipe, mps) = (ep_in.pipe, ep_in.info.max_packet_size);
    // Bind before the host enables capture; `endpoint_set_enabled` arms BRDY and
    // primes both buffers for a bound pipe.  The stream lives as long as this task.
    // SAFETY: MIC is lent once, here; only the stream's interrupt uses it after.
    let mic: &'static mut MicSource = unsafe { &mut *core::ptr::addr_of_mut!(MIC) };
    let _stream = ep_in.into_iso_stream(mic);
    info!(
        "uac2_mic_task: ISO IN stream bound (pipe={}, mps={})",
        pipe, mps
    );

    // Supervise: when packets stop flowing (host closed the stream), clear
    // MIC_STREAMING so the next stream re-anchors the read pointer.  Mirrors the
    // OUT-side uac2_task supervisor.
    let mut ticker = embassy_time::Ticker::every(embassy_time::Duration::from_millis(25));
    let mut last_seq = MIC_PACKET_SEQ.load(Ordering::Relaxed);
    loop {
        ticker.next().await;
        let seq = MIC_PACKET_SEQ.load(Ordering::Relaxed);
        if seq == last_seq {
            MIC_STREAMING.store(false, Ordering::Release);
        }
        last_seq = seq;
    }
}
