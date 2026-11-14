//! `USB UPDATE`: a drive to drop a `.uf2` onto. An app goes into the flash app slot, a loader
//! into the inactive boot slot.
//!
//! The drive is [`deluge_loader::uf2::volume::Volume`]: the host's writes are put back together in
//! SDRAM, and nothing touches flash until the session has ended (USB disconnected) and the whole
//! image has passed [`deluge_loader::uf2::validate`]. A refused file leaves both slots as they
//! were. BACK leaves at any time before the write starts; once writing begins it runs to the end.

use core::convert::Infallible;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use deluge_image::slot::{self, VARIANT_OLED};
use deluge_image::uf2::DELUGE_OLED;
use deluge_loader::boot::Form;
use deluge_loader::uf2::volume::{Info, Volume};
use deluge_loader::uf2::{Assembler, Event, Limits, Refusal, Target, validate};
use deluge_loader::update;
use embassy_time::{Duration, Timer};
use log::{error, info};
use usb_classes::bot::{BlockDevice, Inquiry};

use crate::{device, ui};

/// Where the drive's `INDEX.HTM` sends the browser: the loader's manual.
const INDEX_URL: &str = "https://github.com/FirestormAudio/deluge-sdk/blob/main/docs/app-loader.md";

/// Set to end the session: by BACK, or by a completed file.
static EXIT: AtomicBool = AtomicBool::new(false);
/// The file being received: blocks in, and its block count (`0`: none yet).
static RECEIVED: AtomicU32 = AtomicU32::new(0);
static TOTAL: AtomicU32 = AtomicU32::new(0);
/// The last refusal, for the screen ([`refusal_code`]; `0`: none).
static REFUSED: AtomicU8 = AtomicU8::new(0);
/// A completed file: `1` an app, `2` a loader (`0`: none), and its length.
static DONE: AtomicU8 = AtomicU8::new(0);
static DONE_LEN: AtomicU32 = AtomicU32::new(0);

fn refusal_code(r: Refusal) -> u8 {
    match r {
        Refusal::WrongBoard => 1,
        Refusal::Damaged => 2,
        Refusal::TooLarge => 3,
        Refusal::NeedsSlotBoot => 4,
    }
}

fn refusal_of(code: u8) -> Option<Refusal> {
    Some(match code {
        1 => Refusal::WrongBoard,
        2 => Refusal::Damaged,
        3 => Refusal::TooLarge,
        4 => Refusal::NeedsSlotBoot,
        _ => return None,
    })
}

/// The drive as the BOT/SCSI loop serves it.
struct Uf2Device<'v, 'a, 'b>(&'v mut Volume<'a, 'b>);

impl BlockDevice for Uf2Device<'_, '_, '_> {
    fn block_count(&self) -> u32 {
        Volume::BLOCKS
    }

    fn is_ready(&self) -> bool {
        true
    }

    async fn ensure_ready(&mut self) -> bool {
        true
    }

    async fn read(&mut self, lba: u32, count: u32, buf: &mut [u8]) -> Result<(), ()> {
        for (i, sector) in buf.chunks_exact_mut(512).take(count as usize).enumerate() {
            let sector: &mut [u8; 512] = sector.try_into().map_err(|_| ())?;
            self.0.read(lba + i as u32, sector);
        }
        Ok(())
    }

    async fn write(&mut self, lba: u32, count: u32, buf: &[u8]) -> Result<(), ()> {
        for (i, sector) in buf.chunks_exact(512).take(count as usize).enumerate() {
            let sector: &[u8; 512] = sector.try_into().map_err(|_| ())?;
            match self.0.write(lba + i as u32, sector) {
                Event::Ignored => {}
                Event::Progress { received, total } => {
                    RECEIVED.store(received, Ordering::Relaxed);
                    TOTAL.store(total, Ordering::Relaxed);
                }
                Event::Complete { target, len } => {
                    DONE_LEN.store(len as u32, Ordering::Relaxed);
                    let t = match target {
                        Target::App => 1,
                        Target::Loader => 2,
                    };
                    DONE.store(t, Ordering::Release);
                    EXIT.store(true, Ordering::Release);
                }
                Event::Refused(r) => REFUSED.store(refusal_code(r), Ordering::Relaxed),
            }
        }
        // The host's write always succeeds: a refusal is the screen's to show, and a failed
        // write would only make the host retry it.
        Ok(())
    }

    fn inquiry(&self) -> Inquiry {
        Inquiry {
            vendor: *b"Synthstm",
            product: *b"UF2 Update      ",
            revision: *b"1.00",
        }
    }
}

/// The drive screen until the session ends; BACK sets [`EXIT`]. A refusal shows for 2 s.
async fn status_loop() -> Infallible {
    loop {
        if crate::controls::BACK_PRESSED.swap(false, Ordering::AcqRel) {
            EXIT.store(true, Ordering::Release);
        }
        if let Some(refusal) = refusal_of(REFUSED.swap(0, Ordering::Relaxed)) {
            ui::show_bar_screen(b"USB UPDATE", refusal.message(), None, b"NOTHING WRITTEN").await;
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        let (rx, total) = (
            RECEIVED.load(Ordering::Relaxed),
            TOTAL.load(Ordering::Relaxed),
        );
        let pct = (rx * 100).checked_div(total).unwrap_or(0) as u8;
        ui::show_bar_screen(
            b"USB UPDATE",
            b"DROP A .UF2 FILE",
            Some(pct),
            b"BACK TO CANCEL",
        )
        .await;
        Timer::after(Duration::from_millis(100)).await;
    }
}

/// What a session ended with.
enum Outcome {
    Back,
    Complete(Target, usize),
}

/// One session: serve the drive until BACK or a completed file.
async fn session(vol: &mut Volume<'_, '_>) -> Outcome {
    for a in [&RECEIVED, &TOTAL, &DONE_LEN] {
        a.store(0, Ordering::Relaxed);
    }
    REFUSED.store(0, Ordering::Relaxed);
    DONE.store(0, Ordering::Relaxed);
    crate::controls::BACK_PRESSED.store(false, Ordering::Relaxed);
    device::uf2_session(Uf2Device(vol), &EXIT, status_loop()).await;
    let len = DONE_LEN.load(Ordering::Relaxed) as usize;
    match DONE.load(Ordering::Acquire) {
        1 => Outcome::Complete(Target::App, len),
        2 => Outcome::Complete(Target::Loader, len),
        _ => Outcome::Back,
    }
}

/// `USB UPDATE`, from the menu. Returns on BACK or after a write that failed; a written app
/// restarts the unit, a written loader asks for a power cycle.
pub async fn usb_update(form: Form) {
    loop {
        let limits = Limits {
            families: DELUGE_OLED,
            max_app: deluge_bsp::flash::SLOT_LEN as usize,
            max_loader: slot::SLOT_LEN as usize,
        };
        let info = Info {
            label: *b"DELUGE     ",
            model: "Deluge OLED",
            board_id: "DELUGE-OLED",
            loader_version: env!("CARGO_PKG_VERSION"),
            index_url: INDEX_URL,
            app_slot: device::flash_app().map(|h| {
                (
                    deluge_image::app::APP_HEADER_LEN as u32 + h.body_len,
                    h.body_crc,
                )
            }),
        };
        let mut vol = Volume::new(info, Assembler::new(device::uf2_buffer(), limits));
        let (target, len) = match session(&mut vol).await {
            Outcome::Back => {
                info!("USB UPDATE: left (BACK)");
                return;
            }
            Outcome::Complete(t, len) => (t, len),
        };
        info!("USB UPDATE: a {:?} file of {} bytes is in", target, len);
        let image = vol.assembler().image_mut();
        let max_app_body = device::flashboot::MAX_BODY;
        if let Err(r) = validate(
            target,
            image,
            form,
            VARIANT_OLED,
            slot::DELUGE,
            max_app_body,
        ) {
            error!("USB UPDATE: refused: {:?}", r);
            ui::show_bar_screen(b"USB UPDATE", r.message(), None, b"NOTHING WRITTEN").await;
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        match target {
            Target::App => write_app(image).await,
            Target::Loader => write_loader(form, image).await,
        }
    }
}

/// Write a validated app and restart; on a failed write, say so and go back to the menu.
async fn write_app(image: &[u8]) {
    ui::show_bar_screen(b"WRITING", b"DO NOT POWER OFF", Some(0), b"").await;
    match device::write_flash_app(image, b"WRITING APP").await {
        Ok(()) => {
            info!(
                "USB UPDATE: app written ({} bytes); restarting",
                image.len()
            );
            ui::show_message(b"UPDATED", b"APP").await;
            Timer::after(Duration::from_secs(1)).await;
            device::restart();
        }
        Err(e) => {
            error!("USB UPDATE: app write failed: {:?}", e);
            ui::show_message(b"UPDATE FAILED", b"WRITE FAILED").await;
            Timer::after(Duration::from_secs(2)).await;
        }
    }
}

/// Write a validated loader into the other boot slot and ask for a power cycle (a watchdog
/// restart would make stage-0 roll the still-pending loader back).
async fn write_loader(form: Form, image: &mut [u8]) {
    let Form::Slot { slot: running, .. } = form else {
        return; // `validate` refuses a loader in the stock form
    };
    ui::show_message(b"UPDATING LOADER", b"DO NOT POWER OFF").await;
    match update::self_update(
        &mut device::slot_writer(running),
        image,
        &[VARIANT_OLED],
        slot::DELUGE,
    ) {
        Ok(plan) => {
            info!(
                "USB UPDATE: loader written to slot {} as sequence {}",
                plan.slot, plan.sequence
            );
            ui::show_message(b"LOADER UPDATED", b"POWER-CYCLE NOW").await;
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        Err(update::UpdateError::Locked) => device::power_off_for_unlock(b"TO UPDATE").await,
        Err(e) => {
            error!("USB UPDATE: loader update failed: {:?}", e);
            ui::show_message(b"UPDATE FAILED", e.message()).await;
            Timer::after(Duration::from_secs(2)).await;
        }
    }
}
