//! Deluge SDK example: clock I/O + jack detection.
//!
//! - **Clock in → clock out:** every pulse on the analog trigger-clock input is
//!   echoed as a pulse on gate-0 (the software clock output), and toggles the
//!   SYNC LED. The measured input interval is shown as a rough BPM.
//! - **Jacks:** the OLED reflects headphone / line-out insertion, and the stock
//!   speaker-mute policy is applied (amp off when headphone or line-out present).
//!
//! Exercises `clock_in()`, `clock_out()`, `jacks()`, and `oled()` together.
//!
//! For a free-running clock instead of an echo, drop the `clock_in` half and do:
//! `clk_out.run(ClockOut::period_from_bpm(120.0, 24)).await`.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};

/// Convert a clock-pulse interval to whole BPM (assuming one pulse per beat).
fn interval_to_bpm(dt: Duration) -> u32 {
    let us = dt.as_micros();
    60_000_000u64.checked_div(us).unwrap_or(0) as u32
}

/// Format `n` (0–999) into `buf` as up to 3 ASCII digits; returns the slice.
fn fmt_u32(buf: &mut [u8; 3], mut n: u32) -> &str {
    n = n.min(999);
    let mut i = 3;
    if n == 0 {
        buf[2] = b'0';
        i = 2;
    } else {
        while n > 0 && i > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("?")
}

#[deluge::app]
async fn main(dlg: Deluge) {
    let mut led = dlg.sync_led();
    let mut clk_in = dlg.clock_in();
    let mut clk_out = dlg.clock_out(0);
    let mut jacks = dlg.jacks();
    let mut oled = dlg.oled().await;

    clk_out.set_pulse_width(Duration::from_millis(10));

    let mut bpm = 0u32;
    loop {
        // Echo a clock pulse if one arrives; otherwise refresh ~5×/sec so jack
        // changes and the speaker policy stay live even with no clock present.
        match select(clk_in.tick(), Timer::after_millis(200)).await {
            Either::First(interval) => {
                clk_out.pulse().await;
                led.toggle();
                if let Some(dt) = interval {
                    bpm = interval_to_bpm(dt);
                }
            }
            Either::Second(()) => {}
        }

        // Stock policy: amp on only when no headphone / line-out is inserted.
        jacks.apply_speaker_mute();

        let mut nbuf = [0u8; 3];
        oled.clear();
        oled.text(0, 10, "CLOCK + JACKS");
        oled.text(0, 22, "BPM ");
        oled.text(24, 22, fmt_u32(&mut nbuf, bpm));
        oled.text(
            0,
            34,
            if jacks.headphone() {
                "HP:IN "
            } else {
                "HP:-- "
            },
        );
        oled.text(
            48,
            34,
            if jacks.line_out_left() || jacks.line_out_right() {
                "LINE:IN"
            } else {
                "LINE:--"
            },
        );
        oled.flush().await;
    }
}
