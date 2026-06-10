//! OLED throughput display for the USB mass-storage app.
//!
//! Shows four live values — TX/RX transfer speed (MB/s) and cumulative TX/RX
//! volume (MB) — refreshed a few times a second.  TX is card→host (host reading
//! the card); RX is host→card (host writing the card).
//!
//! ```text
//! USB MASS STORAGE
//! TX 12.3MB/S 1234MB
//! RX  1.1MB/S   56MB
//! ```

use embassy_time::Timer;

use deluge_bsp::oled::{self, text};
use deluge_bsp::pic;

use crate::tasks::msc::{RX_BYTES, TX_BYTES};

/// Refresh / sampling interval.
const INTERVAL_MS: u64 = 250;

/// First on-screen pixel row.  The top 5 rows of the panel sit off the visible
/// area; an extra 5 px nudges the whole layout further down.
const TOP: usize = 10;

#[embassy_executor::task]
pub(crate) async fn oled_task() {
    // oled::init() drives the panel over RSPI0 and waits on the PIC chip-select
    // echo, so the PIC handshake must finish first.
    pic::wait_ready().await;
    oled::init().await;

    let mut fb = oled::FrameBuffer::new();
    let mut last_tx = TX_BYTES.load(core::sync::atomic::Ordering::Relaxed);
    let mut last_rx = RX_BYTES.load(core::sync::atomic::Ordering::Relaxed);

    loop {
        let tx = TX_BYTES.load(core::sync::atomic::Ordering::Relaxed);
        let rx = RX_BYTES.load(core::sync::atomic::Ordering::Relaxed);
        let dtx = tx.wrapping_sub(last_tx);
        let drx = rx.wrapping_sub(last_rx);
        last_tx = tx;
        last_rx = rx;

        // tenths of MB/s = delta_bytes / (interval_ms * 100)  (see derivation:
        // bytes/s = delta*1000/interval_ms; MB/s*10 = bytes/s*10/1e6).
        let tx_speed_tenths = dtx / (INTERVAL_MS * 100);
        let rx_speed_tenths = drx / (INTERVAL_MS * 100);

        fb.fill(0x00);
        text::draw_str(&mut fb, 0, TOP, b"USB MASS STORAGE");

        let mut line = [0u8; 24];
        let len = build_line(&mut line, b"TX ", tx_speed_tenths, tx / 1_000_000);
        text::draw_str(&mut fb, 0, TOP + 14, &line[..len]);
        let len = build_line(&mut line, b"RX ", rx_speed_tenths, rx / 1_000_000);
        text::draw_str(&mut fb, 0, TOP + 26, &line[..len]);

        oled::send_frame(&fb).await;
        Timer::after_millis(INTERVAL_MS).await;
    }
}

/// Format `"<label><speed>MB/S <total>MB"` into `out`, returning its length.
fn build_line(out: &mut [u8], label: &[u8], speed_tenths: u64, total_mb: u64) -> usize {
    let mut p = 0;
    for &b in label {
        push(out, &mut p, b);
    }
    push_dec1(out, &mut p, speed_tenths);
    for &b in b"MB/S " {
        push(out, &mut p, b);
    }
    push_u64(out, &mut p, total_mb);
    for &b in b"MB" {
        push(out, &mut p, b);
    }
    p
}

#[inline]
fn push(out: &mut [u8], p: &mut usize, b: u8) {
    if *p < out.len() {
        out[*p] = b;
        *p += 1;
    }
}

/// Write a base-10 integer.
fn push_u64(out: &mut [u8], p: &mut usize, mut v: u64) {
    if v == 0 {
        push(out, p, b'0');
        return;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        push(out, p, tmp[i]);
    }
}

/// Write a fixed-point value given in tenths as `"<int>.<dec>"`.
fn push_dec1(out: &mut [u8], p: &mut usize, tenths: u64) {
    push_u64(out, p, tenths / 10);
    push(out, p, b'.');
    push(out, p, b'0' + (tenths % 10) as u8);
}
