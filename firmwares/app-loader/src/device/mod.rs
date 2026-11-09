//! The loader's hardware side, past the menu, the prompts and the card: the boot slots and the
//! watchdog, the stock bootloader's flash app slot and its image format, the SRAM trampoline
//! launch, USB0's mass storage and dev-mode upload, the settings sector, and the bottom-1 MB
//! protection (behind the `flash-protect` feature).

pub(crate) mod deluge_settings;
pub(crate) mod devupload;
pub(crate) mod elf;
pub(crate) mod flashboot;
pub(crate) mod launcher;
pub(crate) mod settings;
pub(crate) mod slots;
pub(crate) mod usbmsc;

use deluge::Sd;
use deluge_image::settings::Settings;
use log::{error, info, warn};

use crate::file_browser::{self, EntryList};
use core::sync::atomic::Ordering;

use crate::controls::BACK_PRESSED;
use crate::{hex2, screen, ui};

/// Stop all autonomous peripheral activity before branching to the app.
///
/// Must run after `cortex_ar::interrupt::disable()` and before the launch.
/// The app re-initialises every controller from scratch, so it only needs the
/// hardware to be *quiet*: no DMA channel still transferring into RAM, no timer
/// or GIC source able to fire.  The critical one is the SCIF1/PIC circular
/// receive DMA — see the call site.
pub(crate) unsafe fn quiesce_for_handoff() {
    // Every launch passes through here, so this is where the bootloaders get
    // locked: the loader itself runs with the status register writable, which is
    // what lets a J-Link clear `BP` on a unit whose loader can't hand off.
    #[cfg(feature = "flash-protect")]
    lock_for_handoff();
    unsafe {
        // Stop + software-reset every DMA channel and clear its DMARS request
        // route (covers PIC RX/TX, SD, and OLED channels). Clearing DMARS lets
        // Linux's rz-dmac claim SCIF1-RX cleanly.
        rza1_hal::dmac::stop_all();
        // Stop the Embassy time-driver timers (OSTM0 free-run + OSTM1 alarm).
        rza1_hal::time_driver::stop();
        // Turn the interrupt controller off entirely.
        rza1_hal::gic::shutdown();
    }
}

/// Draw the Deluge droplet right before handing off to a launched image, so the
/// panel holds the boot logo (not the loader's menu) all the way through U-Boot
/// and early kernel until the image's own display code takes over.
///
/// Must be called while interrupts, the executor, and the PIC service are still
/// live: `send_frame` awaits the OLED DMA-completion IRQ and the PIC chip-select
/// echo, so it cannot complete once the machine has been quiesced for handoff.
pub(crate) async fn show_boot_logo() {
    screen::show(&deluge_bsp::oled::boot_logo()).await;
}

/// The one-shot SSB update: if the `SSB UPDATE: ARM` toggle is set, unlock the
/// bootloader region, consume the toggle, and park with no app launched, so the
/// FSB's power-on update can reflash the SSB. Runs once at boot. Arming is not
/// done here but at hand-off ([`lock_for_handoff`]).
#[cfg(feature = "flash-protect")]
async fn ssb_update_pass() {
    use deluge_bsp::flash::{self, protect};

    let cfg = settings::read();
    if !cfg.ssb_write_enable {
        return;
    }
    let id = flash::identify();
    // SAFETY: the loader runs from SRAM; nothing is launched after this.
    let unlocked = unsafe { protect::unprotect(id) };
    let cleared = unsafe {
        settings::write(&Settings {
            ssb_write_enable: false,
            ..cfg
        })
        .await
    };
    info!(
        "Flash protect: SSB write-enable consumed (unlocked={:?}, toggle_cleared={})",
        unlocked, cleared
    );
    match unlocked {
        Ok(_) => ui::show_message(b"SSB UPDATE READY", b"POWER-CYCLE NOW").await,
        Err(_) => ui::show_message(b"SSB UNLOCK FAILED", b"POWER-CYCLE").await,
    }
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}

/// Store a selected SD `/APPS` ELF into the flash app slot.
///
/// Opens the file, flattens its `PT_LOAD` segments into the SDRAM staging window
/// ([`elf::flatten_to_flash_staging`]), then validates + programs it into the
/// slot ([`flashboot::store_image_to_slot`]).  Progress and the final result are
/// shown on the OLED.  On success the next boot-menu pass re-probes the slot and
/// the image appears as the default `BOOT FLASH` entry — no extra wiring needed.
///
/// Returns to the menu on any error; the slot is only ever touched after the
/// image passes FSB validation, so a bad ELF cannot brick an existing image.
async fn write_app_to_flash(sd: &mut Sd, apps: &file_browser::EntryList, idx: usize, label: &[u8]) {
    use embassy_time::{Duration, Timer};

    ui::show_progress(label, 0).await;

    let mut file = match file_browser::open_app(sd, apps, idx).await {
        Ok(f) => f,
        Err(e) => {
            error!("Flash store: open failed: {:?}", e);
            ui::show_message(b"FLASH ERROR", b"OPEN FAILED").await;
            Timer::after(Duration::from_secs(2)).await;
            return;
        }
    };

    // Flatten the ELF into SDRAM staging — first half of the progress bar.
    let stage = match unsafe {
        elf::flatten_to_flash_staging(&mut file, |done, total| async move {
            let pct = done.saturating_mul(50).checked_div(total).unwrap_or(50) as u8;
            ui::show_progress(label, pct).await;
        })
        .await
    } {
        Ok(s) => s,
        Err(e) => {
            let line2: &[u8] = match e {
                elf::ElfError::BadMagic => b"BAD MAGIC",
                elf::ElfError::WrongFormat => b"WRONG FORMAT",
                elf::ElfError::NotFlashable => b"NOT FLASHABLE",
                elf::ElfError::TooLarge => b"TOO LARGE",
                _ => b"SEE LOG",
            };
            error!("Flash store: flatten failed: {:?}", e);
            let _ = file.close().await;
            ui::show_message(b"FLASH ERROR", line2).await;
            Timer::after(Duration::from_secs(2)).await;
            return;
        }
    };

    let _ = file.close().await;

    // Validate + program into the flash slot — second half of the progress bar.
    match unsafe {
        flashboot::store_image_to_slot(&stage, |done, total| async move {
            let pct = 50 + done.saturating_mul(50).checked_div(total).unwrap_or(50) as u8;
            ui::show_progress(label, pct.min(100)).await;
        })
        .await
    } {
        Ok(()) => {
            info!("Flash store: programmed {} bytes into the slot", stage.len);
            ui::show_message(b"FLASHED OK", b"NOW DEFAULT").await;
            Timer::after(Duration::from_secs(2)).await;
        }
        Err(e) => {
            error!("Flash store: image rejected: {:?}", e);
            let line2: &[u8] = match e {
                deluge_image::elf::FsbError::VerifyFailed(_) => b"VERIFY FAILED",
                deluge_image::elf::FsbError::TooLargeForSlot => b"TOO LARGE",
                _ => b"BAD IMAGE",
            };
            ui::show_message(b"FLASH ERROR", line2).await;
            Timer::after(Duration::from_secs(2)).await;
        }
    }
}

/// Identify the SPI-NOR part at runtime and hold the result on screen until BACK.
///
/// The JEDEC bytes disambiguate the three second-sourced chips (AT25SF321B /
/// S25FL132K / W25Q32JV) and hence which write-protection recipe applies (see
/// `deluge_bsp::flash::protect`). Read-only: no status-register writes.
///
/// Reading the ID leaves memory-mapped read mode briefly, so every caller must
/// have closed its FAT handles first — the `SETTINGS` menu branch does this
/// before it opens this screen.
async fn flash_info() {
    let id = deluge_bsp::flash::identify();
    let sr = rza1_hal::spibsc::read_status_reg();
    log::info!(
        "Flash: {} JEDEC={:02x} {:02x} {:02x} cap={:#x} SR={:#04x}",
        id.model,
        id.raw[0],
        id.raw[1],
        id.raw[2],
        id.capacity,
        sr
    );
    let mut line = *b"ID 00 00 00";
    hex2(&mut line[3..5], id.raw[0]);
    hex2(&mut line[6..8], id.raw[1]);
    hex2(&mut line[9..11], id.raw[2]);
    ui::show_message(id.model.as_bytes(), &line).await;

    // Hold the frame until BACK is pressed (clearing any stale press first so we
    // don't exit immediately), polling the flag `controls::run` sets.  Nothing else
    // redraws the OLED while we wait, so the frame persists.
    BACK_PRESSED.store(false, Ordering::Release);
    while !BACK_PRESSED.swap(false, Ordering::AcqRel) {
        embassy_time::Timer::after(embassy_time::Duration::from_millis(50)).await;
    }
}

/// Arm the lock-down right before a launch (`deluge_loader::boot::before_launch`), so the image
/// runs unable to clear the protection until the next power cycle. An unrecognized part, or any
/// refusal, launches unlocked (logged): the lock is a backstop.
#[cfg(feature = "flash-protect")]
fn lock_for_handoff() {
    let policy = deluge_loader::boot::BootPolicy { protect: true };
    match deluge_loader::boot::before_launch(&mut slots::DelugeFlash::new(), policy) {
        Ok(()) => info!("Flash protect: locked for hand-off"),
        Err(why) => warn!("Flash protect: launching unlocked, {:?}", why),
    }
}

/// The flash app slot's image, when it holds a valid one.
pub(crate) type FlashApp = flashboot::FlashImage;
/// The dev-mode upload listener, brought up before the menu draws.
pub(crate) type Upload = devupload::Listener;
/// The flash, as the boot sequence uses it.
pub(crate) type Flash = slots::DelugeFlash;
/// The loader's boot slots, as a self-update writes them.
pub(crate) type Slots = slots::DelugeSlots;

/// The loader's boot-slot variant code: the only one it accepts as an update.
pub(crate) const VARIANT: u16 = deluge_image::slot::VARIANT_OLED;

/// Read the boot-info word stage-0 left (`0` if none), and clear it.
pub(crate) fn take_boot_info() -> u32 {
    let p = deluge_image::slot::BOOT_INFO_ADDR as *mut u32;
    // SAFETY: the first word of data-retention RAM, which the reset handler made writable and
    // nothing uses until a launch (the trampoline); read before anything could overwrite it.
    unsafe {
        let w = core::ptr::read_volatile(p);
        core::ptr::write_volatile(p, 0);
        w
    }
}

/// Whether boot slot `n` is pending: its header parses and its confirm word isn't
/// confirmed. A header that doesn't parse is never written.
pub(crate) fn slot_pending(n: u8) -> bool {
    slots::pending(n)
}

/// Put the flash's read mode back on the stock bootloader's timing (stage-0 leaves the boot
/// ROM's slow reads).
pub(crate) fn fast_read() {
    // SAFETY: the loader runs from SRAM, and no flash operation is in progress.
    unsafe { deluge_bsp::flash::set_stock_read_mode() }
}

pub(crate) fn boot_flash() -> Flash {
    slots::DelugeFlash::new()
}

pub(crate) fn slot_writer(running: u8) -> Slots {
    slots::DelugeSlots::new(running)
}

/// RAM a whole slot image fits in, free while the menu runs.
pub(crate) fn update_buffer() -> &'static mut [u8] {
    // SAFETY: the SDRAM staging window, which only an app launch (after the menu) or a flash
    // store uses; the loader allocates nothing there. One slot's worth.
    unsafe {
        core::slice::from_raw_parts_mut(
            deluge_image::elf::SDRAM_STAGE_BASE as *mut u8,
            deluge_image::slot::SLOT_LEN as usize,
        )
    }
}

/// Restart the watchdog's count (a pending slot's shim started it). Harmless when it isn't
/// running.
pub(crate) fn kick_watchdog() {
    // SAFETY: the watchdog is the loader's alone; the shim that started it is gone.
    rza1_hal::wdt::Watchdog::new(unsafe { rza1_hal::peripherals::WDT::steal() }).kick()
}

/// Stop the watchdog (the running slot is confirmed).
pub(crate) fn stop_watchdog() {
    // SAFETY: as `kick_watchdog`.
    rza1_hal::wdt::Watchdog::new(unsafe { rza1_hal::peripherals::WDT::steal() }).stop()
}

/// The protection is locked down and the loader needs to write it: say how to power the
/// flash off, then `why` ("TO UPDATE"), and wait for it.
pub(crate) async fn power_off_for_unlock(why: &'static [u8]) -> ! {
    // The Deluge's power switch cuts the flash's supply: a power cycle releases the lock.
    ui::show_message(b"POWER-CYCLE", why).await;
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}

/// Start-up housekeeping, once, before the first menu.
pub(crate) async fn early(form: deluge_loader::boot::Form) {
    // Prime the flash write-bounds guard from the JEDEC ID before any flash write.
    // The AT25's density byte isn't a log2-size, so the SPIBSC `1<<density`
    // fallback would otherwise read capacity 0 and refuse every write. Read-only.
    deluge_bsp::flash::prime_capacity();

    // A loader in a boot slot has stage-0 in front of it: the stock bootloader's layout (the
    // settings at 0x07F000, its SSB update) isn't there.
    if form != deluge_loader::boot::Form::Stock {
        return;
    }

    // Bring the Deluge app's settings forward if they are still at the pre-
    // relocation offset. Unconditional (like the relocation itself) and ahead of
    // `ssb_update_pass`, which parks forever when it runs and would
    // otherwise skip the migration on that boot. See `deluge_settings`.
    match unsafe { deluge_settings::migrate() } {
        deluge_settings::Outcome::NotNeeded => {}
        deluge_settings::Outcome::Migrated => {
            info!("Deluge settings: migrated 0x07F000 -> 0x3D0000")
        }
        deluge_settings::Outcome::Failed => {
            warn!("Deluge settings: migration failed; the app will factory-reset")
        }
    }

    // If the one-shot SSB write-enable toggle is set, unlock the bootloader
    // region and park with no app. Protection itself is armed at hand-off
    // (`quiesce_for_handoff`), so the menu runs with the status register
    // writable. Compiled out (no status-register writes) unless the
    // `flash-protect` feature is on (see `deluge_bsp::flash::protect`).
    #[cfg(feature = "flash-protect")]
    ssb_update_pass().await;
}

/// The loader's persisted settings (the defaults when none are stored).
pub(crate) fn read_settings() -> Settings {
    settings::read()
}

/// Persist `s`; whether it reads back.
pub(crate) async fn write_settings(s: &Settings) -> bool {
    // SAFETY: the loader runs from SRAM, and the card has been let go of.
    unsafe { settings::write(s).await }
}

/// Say why a settings write didn't stick, on the display.
pub(crate) async fn show_settings_write_failure() {
    // The flash write didn't stick. Show the JEDEC ID and status register so the failure
    // can be diagnosed: a plausible non-`00`/`FF` ID (e.g. `EF 40 16` for the W25Q32JV)
    // confirms manual-mode works; status `BP[2:0]` (bits 2-4) set means the settings
    // sector is write-protected.
    let id = rza1_hal::spibsc::read_id();
    let sr = rza1_hal::spibsc::read_status_reg();
    error!(
        "Settings flash write failed: JEDEC={:02x} {:02x} {:02x}, SR={:#04x}",
        id[0], id[1], id[2], sr
    );
    let mut line = *b"ID...... SR..";
    hex2(&mut line[2..4], id[0]);
    hex2(&mut line[4..6], id[1]);
    hex2(&mut line[6..8], id[2]);
    hex2(&mut line[11..13], sr);
    ui::show_message(b"FLASH WRITE FAIL", &line).await;
    embassy_time::Timer::after(embassy_time::Duration::from_secs(4)).await;
}

/// The flash part's identity on the display, until BACK.
pub(crate) async fn show_flash_info() {
    flash_info().await
}

/// The flash app slot's image, if it holds a valid one.
pub(crate) fn flash_app() -> Option<FlashApp> {
    flashboot::probe()
}

/// Launch the flash app. The card must already be let go of.
pub(crate) async fn boot_flash_app(img: FlashApp) -> ! {
    info!(
        "Booting flash image: start={:#010x} end={:#010x} entry={:#010x}",
        img.code_start, img.code_end, img.entry
    );
    ui::show_message(b"BOOTING", b"FLASH FW").await;

    // Draw the boot droplet so the panel shows it through handoff.
    // Must run before interrupts/DMA are quiesced (see show_boot_logo).
    show_boot_logo().await;

    // Close FAT handles and quiesce, then copy from flash into SRAM via
    // the trampoline (single descriptor) like the SD ELF path.
    cortex_ar::interrupt::disable();
    unsafe { quiesce_for_handoff() };

    let desc = img.desc();
    unsafe { launcher::launch_via_trampoline(&[desc], img.entry) }
}

/// Store app `idx` of `apps` into the flash app slot, showing progress under `label`.
pub(crate) async fn store_flash_app(sd: &mut Sd, apps: &EntryList, idx: usize, label: &[u8]) {
    write_app_to_flash(sd, apps, idx, label).await
}

/// Load app `idx` of `apps` and launch it, showing progress under `label`. Returns only if
/// it can't be loaded, having said why on the display.
pub(crate) async fn launch_elf(mut sd: Sd, apps: &EntryList, idx: usize, label: &[u8]) {
    info!("Loading: {}", apps.name(idx));
    ui::show_progress(label, 0).await;

    // Open the selected file from /APPS/.
    let mut file = match file_browser::open_app(&mut sd, apps, idx).await {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open app: {:?}", e);
            ui::show_message(b"OPEN ERROR", b"BACK TO MENU").await;
            embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
            return;
        }
    };

    // Stream-load ELF segments.  SRAM-targeting segments are staged in SDRAM;
    // SDRAM-targeting segments are written to their final addresses directly.
    let load_result = match unsafe {
        elf::load_from_sd_with_progress(&mut file, |done, total| async move {
            let raw = done.saturating_mul(100).checked_div(total).unwrap_or(100) as u8;
            ui::show_progress(label, raw).await;
        })
        .await
    } {
        Ok(r) => r,
        Err(e) => {
            let line2: &[u8] = match e {
                elf::ElfError::BadMagic => b"BAD MAGIC",
                elf::ElfError::WrongFormat => b"WRONG FORMAT",
                elf::ElfError::BadLoadAddress => b"BAD LOAD ADDR",
                elf::ElfError::Unstreamable => b"BAD LAYOUT",
                _ => b"SEE LOG",
            };
            error!("ELF load error: {:?}", e);
            ui::show_message(b"LOAD ERROR", line2).await;
            embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
            let _ = file.close().await;
            return;
        }
    };

    info!(
        "ELF loaded, entry = {:#010x}, {} SRAM segment(s) staged",
        load_result.entry, load_result.n_sram
    );

    // Draw the boot droplet so the panel shows it through handoff — even if
    // the launched app never uses the display. Must run before interrupts/DMA
    // are quiesced (see show_boot_logo).
    show_boot_logo().await;

    // Close the file and let go of the card, then disable interrupts, before we hand off.
    let _ = file.close().await;
    drop(sd);
    cortex_ar::interrupt::disable();

    // Quiesce every peripheral that keeps running on its own.  Masking CPU
    // interrupts is not enough: the SCIF1/PIC receive DMA is a *circular*
    // channel that keeps writing incoming bytes into its ring buffer
    // independently of the CPU, and after the trampoline overwrites SRAM with
    // the app that buffer is the app's memory — corrupting its TTB/stack/data
    // and crashing early init.  Stop all DMA, the OSTM timers, and the GIC so
    // the app boots into a quiet machine like a cold start.
    unsafe { quiesce_for_handoff() };

    if load_result.n_sram > 0 {
        // One or more segments target SRAM.  Use the trampoline in retention
        // RAM to move them after the bootloader's SRAM is no longer needed.
        unsafe {
            launcher::launch_via_trampoline(
                &load_result.sram_descs[..load_result.n_sram],
                load_result.entry,
            )
        }
    } else {
        // Pure SDRAM app — all segments already at their final addresses.
        unsafe { launcher::launch(load_result.entry) }
    }
}

/// USB mass storage for the card, until BACK. The card must already be let go of.
pub(crate) async fn data_transfer() {
    usbmsc::run_data_transfer_mode().await
}

/// Bring up the dev-mode upload listener.
pub(crate) fn start_dev_upload() -> Upload {
    devupload::prepare()
}

/// Listen for an upload; returns only by launching it.
pub(crate) async fn run_dev_upload(upload: Upload) -> ! {
    upload.run().await
}

/// Tear the listener down (the menu won), freeing its USB port.
pub(crate) fn stop_dev_upload() {
    // SAFETY: the listener's session is over.
    rza1_hal::usb::disconnect(unsafe { rza1_hal::peripherals::USB0::steal() });
}
