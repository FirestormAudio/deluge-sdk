//! App loader (second-stage bootloader) for the Synthstrom Deluge (RZ/A1L).
//!
//! Loaded by the Deluge first-stage bootloader from SPI flash into SRAM, above
//! the bootloader's own code (see `stock_fsb.x`), or copied there from a boot slot
//! by `firmwares/loader-shim`.  On boot it:
//!   1. Initialises the platform (MMU, caches, SDRAM, GIC, OSTM): `deluge-sdk`'s
//!      `#[deluge::app]` does, as for any app; the display, the controls and the card are the
//!      SDK's `Oled`, `Input` and `Sd`.
//!   2. Probes the SPI-flash app slot and mounts the SD card FAT filesystem.
//!   3. Builds a boot menu from the flash image (the default entry), the ELF
//!      images in `/APPS/` on the card, and the `DATA TRANSFER` and `SETTINGS`
//!      entries.
//!   4. Depending on the persisted auto-boot setting, launches the default entry
//!      at once, after a countdown, or waits for a selection on the OLED +
//!      encoder-wheel menu.  Holding LOAD at power-on always forces the menu.
//!   5. Loads the selected image's `PT_LOAD` segments to their physical
//!      addresses, quiesces the hardware, and branches to its entry point.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod controls;
mod device;
mod file_browser;
mod screen;
mod ui;

use deluge::prelude::*;
use deluge_image::settings;
use log::{error, info, warn};

use core::sync::atomic::{AtomicBool, Ordering};

use controls::LOAD_SEEN;
use deluge_image::slot;
use deluge_loader::boot::{self, BootPolicy, Form, Unlock};
use deluge_loader::update::SlotWriter;
use deluge_loader::{menu, update};

/// Write `byte` as two uppercase hex digits into `out[..2]` (for on-OLED
/// diagnostics, since the loader has no other display channel by default).
pub(crate) fn hex2(out: &mut [u8], byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out[0] = HEX[(byte >> 4) as usize];
    out[1] = HEX[(byte & 0xF) as usize];
}

/// The protection policy: provision the boot region and lock it down before apps.
const POLICY: BootPolicy = BootPolicy {
    protect: cfg!(feature = "flash-protect"),
};

/// Whether [`feed_watchdog`] keeps kicking.
static FEED_WATCHDOG: AtomicBool = AtomicBool::new(false);

/// Kick the watchdog a pending slot's shim started, well inside its shortest timeout, until told
/// to stop.
#[embassy_executor::task]
async fn feed_watchdog() {
    while FEED_WATCHDOG.load(Ordering::Acquire) {
        device::kick_watchdog();
        embassy_time::Timer::after(embassy_time::Duration::from_millis(40)).await;
    }
}

/// The loader update on the card, `/BOOT/LOADER.SLT` (one image for either slot: its shim is
/// position-independent), read into `buf`: its length, or `None` when there's none (or it doesn't
/// fit a slot).
async fn read_update(sd: &mut deluge::Sd, buf: &mut [u8]) -> Option<usize> {
    let mut file = sd.open_in("BOOT", "LOADER.SLT").await.ok()?;
    let len = file.len() as usize;
    let ok = len <= buf.len() && {
        let mut at = 0;
        while at < len {
            match file.read(&mut buf[at..len]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => at += n,
            }
        }
        at == len
    };
    let _ = file.close().await;
    ok.then_some(len)
}

#[deluge::app]
async fn main(dlg: Deluge) {
    info!("app-loader: starting");

    // Who started us: stage-0, from a boot slot (the word it left), or the stock bootloader.
    let form = boot::form(device::take_boot_info());
    let pending = match form {
        Form::Slot { slot, .. } => device::slot_pending(slot),
        Form::Stock => false,
    };
    // A pending slot's shim started the watchdog (at most 125 ms on the Deluge): kick it now and
    // between the steps below, which run without yielding; the feeding task takes over at the
    // first await, until the slot is confirmed.
    if pending {
        device::kick_watchdog();
        FEED_WATCHDOG.store(true, Ordering::Release);
        dlg.spawner().spawn(feed_watchdog().unwrap());
    }
    info!("Boot: {:?}, pending={}", form, pending);
    // Stage-0 leaves the boot ROM's slow reads; the stock bootloader left these already.
    device::fast_read();
    if pending {
        device::kick_watchdog();
    }
    // Reload the protection, provision it, and confirm this slot: before the display, whose
    // bring-up is the slowest part of the boot.
    let early = boot::early_boot(&mut device::boot_flash(), form, !pending, POLICY);
    info!("Boot: {:?}", early);
    if pending && early.confirmed {
        FEED_WATCHDOG.store(false, Ordering::Release);
        device::stop_watchdog();
    } else if pending && early.unlock == Unlock::None {
        // The confirm failed: stop feeding the watchdog, so the slot rolls back.
        FEED_WATCHDOG.store(false, Ordering::Release);
    }

    // The controls first, so a LOAD held from power-on is latched before the boot decision
    // (the PIC reports it in answer to the button-state resend its bring-up sends).
    dlg.spawner().spawn(controls::run(dlg.input()).unwrap());
    screen::init(dlg.oled().await);
    info!("OLED: ready");
    if early.unlock == Unlock::NeedsPowerOff {
        // Pending, but the protection is locked down (an app launched since the flash last lost
        // power): the confirm can't be written until the flash powers off. Still fed.
        device::power_off_for_unlock(b"TO FINISH UPDATE").await;
    }
    if let Form::Slot {
        reason: slot::Reason::RolledBack,
        ..
    } = form
    {
        // The newer slot didn't confirm before the watchdog fired: say so once, so a failed
        // update doesn't look like one that never happened.
        warn!("Boot: rolled back from a newer slot that didn't start");
        ui::show_message(b"LOADER UPDATE", b"ROLLED BACK").await;
        embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
    }

    // Recovery gesture: LOAD pressed or held at any point between reset and
    // here (see LOAD_SEEN).  Sampled once — later menu passes are user-driven
    // and must not re-trigger it.  The splash matters most on an INSTANT unit,
    // which otherwise gives no sign that the *hold* — rather than a failed boot —
    // is why the menu appeared.  Nothing is persisted: the user's setting stands.
    let recovery = LOAD_SEEN.load(core::sync::atomic::Ordering::Acquire);
    if recovery {
        info!("Recovery: LOAD held at boot — forcing the boot menu");
        ui::show_message(b"RECOVERY", b"BOOT MENU").await;
        embassy_time::Timer::after(embassy_time::Duration::from_millis(700)).await;
    }

    // Auto-boot (countdown *or* instant) only ever fires on the first pass of the
    // loop below.  See `settings::boot_mode`.
    let mut auto_boot_allowed = true;

    device::early(form).await;

    // Boot menu loop.  A bootable selection launches and never returns; the
    // DATA TRANSFER USB mode returns here when BACK is pressed, and a
    // write-to-flash also returns here, so the menu is rebuilt from fresh state
    // (re-probing the flash slot and re-listing the SD card).
    //
    // Only the first pass shows the "DELUGE BOOT / INIT SD..." splash; on later
    // passes it would look like a reboot, so the menu is simply redrawn by
    // `run_selector`.
    let mut first_pass = true;
    loop {
        // Read persisted settings each pass so a DEV MODE toggle takes effect on
        // the very next menu rebuild (dev mode adds a background USB listener and
        // disables the auto-boot countdown).
        let cfg = device::read_settings();

        // Probe the on-flash firmware slot.  Independent of the SD card, so a
        // unit with valid flash firmware can boot with no card inserted.
        let flash_img = device::flash_app();
        if flash_img.is_some() {
            info!("Flash: valid firmware image present in slot");
        }

        if first_pass {
            ui::show_message(b"DELUGE BOOT", b"INIT SD...").await;
        }
        first_pass = false;

        // Take the card for this pass.  A missing/failed card is not fatal; every exit
        // from the pass drops it, so the next pass (or DATA TRANSFER) can take it again.
        let mut sd = match dlg.sd().await {
            Ok(sd) => Some(sd),
            Err(e) => {
                warn!("SD: not mounted: {:?}", e);
                None
            }
        };
        let sd_listing = match sd.as_mut() {
            Some(sd) => match file_browser::list_apps(sd).await {
                Ok(list) => Some(list),
                Err(e) => {
                    error!("FAT: failed to list /APPS: {:?}", e);
                    None
                }
            },
            None => None,
        };

        // The menu: the flash app (the default), the SD apps, DATA TRANSFER, UPDATE LOADER
        // (when offered) and SETTINGS (`deluge_loader::menu`). DATA TRANSFER only needs a *card*,
        // not a working filesystem, and the Deluge's card-detect pin is unreliable, so it is
        // always offered: the host can still reach (or reformat) a card that won't mount, and
        // the menu always has an entry.
        let has_flash = flash_img.is_some();
        let sd_count = sd_listing.as_ref().map_or(0, |e| e.len());
        let boot_total = menu::boot_targets(&menu::entries(has_flash, sd_count, false));

        // How to boot: skip the menu entirely (INSTANT), run it with a countdown,
        // or sit on it.  All the precedence lives in the host-tested decision fn.
        let mode = settings::boot_mode(&cfg, boot_total, recovery, auto_boot_allowed);
        // Every pass after this one is user-driven — never auto-boot again.
        auto_boot_allowed = false;

        // A loader update on the card (slot form only), read whole only when a menu will show,
        // and offered if it validates and no slot already holds it.
        let update_len = match (form, sd.as_mut(), mode) {
            (_, _, settings::BootMode::Instant) => None,
            (Form::Slot { .. }, Some(sd), _) => read_update(sd, device::update_buffer()).await,
            _ => None,
        };
        let offer_update = match (form, update_len) {
            (Form::Slot { slot, .. }, Some(n)) => {
                let mut held = device::slot_writer(slot);
                let slots = [held.read_slot(0), held.read_slot(1)];
                update::update_offered(
                    form,
                    Some(&device::update_buffer()[..n]),
                    &[device::VARIANT],
                    slot::DELUGE,
                    slots,
                )
            }
            _ => false,
        };

        let entries = menu::entries(has_flash, sd_count, offer_update);
        let mut sd_names = [b"".as_slice(); file_browser::MAX_APPS];
        if let Some(list) = sd_listing.as_ref() {
            for (i, name) in sd_names.iter_mut().enumerate().take(sd_count) {
                *name = list.display_name(i);
            }
        }
        let mut name_refs = [b"".as_slice(); menu::MAX_ENTRIES];
        for (label, &entry) in name_refs.iter_mut().zip(entries.iter()) {
            *label = menu::label(entry, &sd_names);
        }
        let menu_total = entries.len();

        info!(
            "Boot menu: {} boot entry(ies) (flash={}, dev_mode={}, update={})",
            boot_total, has_flash, cfg.dev_mode, offer_update
        );
        info!(
            "Boot mode: {:?} at {} ms",
            mode,
            embassy_time::Instant::now().as_millis()
        );

        let selection = if mode == settings::BootMode::Instant {
            // The menu is never drawn: launch the default entry (index 0 — the
            // flash image when present, else the first SD app).  `boot_mode`
            // guarantees `boot_total > 0` here, so index 0 is a real boot target.
            info!("Auto-boot: INSTANT — launching the default entry");
            ui::Selection {
                index: 0,
                long_press: false,
            }
        } else {
            let countdown = match mode {
                settings::BootMode::Countdown(secs) => secs,
                // Wait: `run_selector` treats 0 as "no countdown, wait for a pick".
                _ => 0,
            };

            // In dev mode, race the menu selector against the background USB upload
            // listener: whichever resolves first wins.  The listener only ever
            // "returns" by loading and launching a received image (it never hands a
            // value back), so the menu branch is the only one that yields a selection
            // here.  Outside dev mode, just run the selector.
            //
            // Crucially, bring the USB device up *before* `run_selector` starts
            // drawing: USB bring-up reconfigures interrupts/clocks, and doing that
            // while an OLED frame DMA + PIC handshake is in flight can wedge the
            // display so the menu never redraws (`usbmsc` builds USB before
            // starting its OLED loop for the same reason).
            if cfg.dev_mode {
                use embassy_futures::select::{Either, select};
                let upload = device::start_dev_upload();
                match select(
                    ui::run_selector(&name_refs[..menu_total], 0, countdown),
                    device::run_dev_upload(upload),
                )
                .await
                {
                    Either::First(sel) => sel,
                    Either::Second(never) => never,
                }
            } else {
                ui::run_selector(&name_refs[..menu_total], 0, countdown).await
            }
        };
        let ui::Selection {
            index: selected,
            long_press,
        } = selection;

        // The menu branch won (or dev mode is off): if a CDC listener was brought
        // up, tear it down so the port is free for a later DATA TRANSFER MSC init.
        if cfg.dev_mode {
            device::stop_dev_upload();
        }

        match entries[selected] {
            // ---- Long-press on an SD `/APPS` ELF: store it to the flash slot ----
            // Whether confirmed, cancelled, or failed, the menu is rebuilt, so a
            // freshly-stored image shows up as the default FLASH entry.
            menu::Entry::SdApp(i) if long_press => {
                let list = sd_listing
                    .as_ref()
                    .expect("sd listing present for SD entry");
                let label = list.display_name(i);
                if ui::confirm_write_to_flash(label).await {
                    let sd = sd.as_mut().expect("the card is mounted for an SD entry");
                    device::store_flash_app(sd, list, i, label).await;
                }
            }

            // ---- SETTINGS screen (persists on exit, rebuilds the menu) ----
            // The screen edits a copy; we write flash once, only if something
            // actually changed, so the next pass re-reads it (updating the
            // auto-boot mode and whether the USB listener runs).
            menu::Entry::Settings => {
                // Let go of the card before touching the flash bus (the settings write
                // leaves memory-mapped read mode, like the app-slot store).
                drop(sd);
                let mut new_cfg = cfg;
                ui::run_settings(&mut new_cfg, form == Form::Stock).await;
                if new_cfg != cfg {
                    info!("Settings: {:?} -> {:?}", cfg, new_cfg);
                    if device::write_settings(&new_cfg).await {
                        ui::show_message(b"SETTINGS", b"SAVED").await;
                        embassy_time::Timer::after(embassy_time::Duration::from_millis(700)).await;
                    } else {
                        device::show_settings_write_failure().await;
                    }
                }
            }

            // ---- UPDATE LOADER: the card's image into the other boot slot ----
            menu::Entry::UpdateLoader => {
                drop(sd);
                let (Form::Slot { slot, .. }, Some(n)) = (form, update_len) else {
                    continue; // only offered in the slot form, with an image read
                };
                ui::show_message(b"UPDATING LOADER", b"DO NOT POWER OFF").await;
                let image = &mut device::update_buffer()[..n];
                match update::self_update(
                    &mut device::slot_writer(slot),
                    image,
                    &[device::VARIANT],
                    slot::DELUGE,
                ) {
                    Ok(plan) => {
                        info!(
                            "Loader update: written to slot {} as sequence {}",
                            plan.slot, plan.sequence
                        );
                        // Not a watchdog reset: stage-0 would take it for the new slot's
                        // failure and roll it back. A power cycle also releases any lock-down.
                        ui::show_message(b"LOADER UPDATED", b"POWER-CYCLE NOW").await;
                        loop {
                            embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
                        }
                    }
                    Err(update::UpdateError::Locked) => {
                        // The lock-down is armed: no write lifts it until the flash loses power.
                        device::power_off_for_unlock(b"TO UPDATE").await;
                    }
                    Err(e) => {
                        error!("Loader update failed: {:?}", e);
                        let line2: &[u8] = match e {
                            update::UpdateError::Foreign => b"WRONG VARIANT",
                            update::UpdateError::Invalid(_) => b"BAD IMAGE",
                            update::UpdateError::NoSequence => b"NO SEQUENCE",
                            update::UpdateError::Locked => b"LOCKED",
                            update::UpdateError::Write(_) => b"WRITE FAILED",
                        };
                        ui::show_message(b"UPDATE FAILED", line2).await;
                        embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
                    }
                }
            }

            // ---- DATA TRANSFER mode (returns on BACK) ----
            menu::Entry::DataTransfer => {
                // The host gets the raw card: let go of the mounted volume first.
                drop(sd);
                device::data_transfer().await;
            }

            // ---- Flash boot path (never returns) ----
            menu::Entry::FlashApp => {
                drop(sd);
                device::boot_flash_app(flash_img.expect("offered only with a flash app")).await;
            }

            // ---- SD ELF path (returns only if it can't be loaded) ----
            menu::Entry::SdApp(i) => {
                let list = sd_listing.expect("sd listing present for SD entry");
                let label = list.display_name(i);
                let sd = sd.expect("the card is mounted for an SD entry");
                device::launch_elf(sd, &list, i, label).await;
            }
        }
    }
}
