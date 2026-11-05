//! Deluge SDK example: SD-card files.
//!
//! 1. Writes a small file to the card root and reads it back.
//! 2. Streams `sd_demo_stream.bin` (8 MiB, made by `tools/make_stream_file.py` and copied onto
//!    the card beforehand) in 32 KiB chunks, checks every byte, and logs the throughput.
//! 3. Seeks into the middle of it and checks what it reads there.
//!
//! Each step logs its result (RTT with the `rtt` feature); the SYNC LED shows the outcome: solid
//! = every step passed, fast blink = a step failed or there's no card.

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![feature(impl_trait_in_assoc_type)]

use deluge::prelude::*;
use deluge::sd::AlignedBuf;
use embassy_time::{Instant, Timer};

const FILE: &str = "SDKTEST.TXT";
const PAYLOAD: &[u8] = b"deluge sdk";
/// A long name: the card's FAT long-name support is part of the check.
const STREAM_FILE: &str = "sd_demo_stream.bin";
const STREAM_BYTES: u32 = 8 << 20;
/// Where the seek check reads: a word boundary in the middle of the file.
const SEEK_AT: u32 = (4 << 20) | 4;

/// Byte `offset` of the stream file: word `offset / 4` little-endian, word `k` = `k * 0x9E3779B9`.
fn pattern(offset: u32) -> u8 {
    (offset / 4).wrapping_mul(0x9E37_79B9).to_le_bytes()[(offset % 4) as usize]
}

/// Whether `bytes`, read from `offset` in the stream file, are the pattern.
fn pattern_ok(offset: u32, bytes: &[u8]) -> bool {
    (offset..).zip(bytes).all(|(i, &b)| b == pattern(i))
}

/// Write [`PAYLOAD`] and read it back.
async fn round_trip(sd: &mut Sd) -> bool {
    let mut buf = [0u8; 32];
    let ok = sd.write(FILE, PAYLOAD).await.is_ok()
        && matches!(sd.read(FILE, &mut buf).await, Ok(n) if &buf[..n] == PAYLOAD);
    log::info!("round trip: {}", if ok { "OK" } else { "FAILED" });
    ok
}

/// Stream the whole stream file, checking every byte.
async fn stream(sd: &mut Sd, buf: &mut AlignedBuf<32768>) -> bool {
    let mut file = match sd.open(STREAM_FILE).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!(
                "stream: can't open {STREAM_FILE} ({e:?}): put it on the card, from \
                tools/make_stream_file.py"
            );
            return false;
        }
    };
    if file.len() != STREAM_BYTES {
        log::warn!(
            "stream: {STREAM_FILE} is {} bytes, not {STREAM_BYTES}",
            file.len()
        );
        return false;
    }
    let start = Instant::now();
    let mut at = 0u32;
    loop {
        match file.read(&mut buf.0).await {
            Ok(0) => break,
            Ok(n) if pattern_ok(at, &buf.0[..n]) => at += n as u32,
            Ok(_) => {
                log::warn!("stream: wrong bytes in the chunk at {at}");
                return false;
            }
            Err(e) => {
                log::warn!("stream: read failed at {at}: {e:?}");
                return false;
            }
        }
    }
    let us = start.elapsed().as_micros().max(1);
    let _ = file.close().await;
    let kib = u64::from(at) / 1024;
    log::info!(
        "stream: {kib} KiB in {} ms = {} KiB/s",
        us / 1000,
        kib * 1_000_000 / us
    );
    at == STREAM_BYTES
}

/// Seek into the stream file and check the bytes there.
async fn seek(sd: &mut Sd) -> bool {
    let mut four = [0u8; 4];
    let ok = match sd.open(STREAM_FILE).await {
        Ok(mut file) => {
            file.seek(SEEK_AT).await.is_ok()
                && matches!(file.read(&mut four).await, Ok(4))
                && pattern_ok(SEEK_AT, &four)
        }
        Err(_) => false,
    };
    log::info!("seek: {}", if ok { "OK" } else { "FAILED" });
    ok
}

#[deluge::app]
async fn main(dlg: Deluge) {
    let mut led = dlg.sync_led();
    // 32 KiB: a static rather than in `main`'s future.
    static mut BUF: AlignedBuf<32768> = AlignedBuf([0; 32768]);
    // SAFETY: `main` runs once and is the buffer's only user.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };

    let ok = match dlg.sd().await {
        Ok(mut sd) => {
            let trip = round_trip(&mut sd).await;
            let streamed = stream(&mut sd, buf).await;
            let sought = seek(&mut sd).await;
            trip && streamed && sought
        }
        Err(e) => {
            log::warn!("no card: {e:?}");
            false
        }
    };
    log::info!("sd_demo: {}", if ok { "all OK" } else { "FAILED" });

    loop {
        if ok {
            led.on();
            Timer::after_millis(1000).await;
        } else {
            led.toggle();
            Timer::after_millis(120).await;
        }
    }
}
