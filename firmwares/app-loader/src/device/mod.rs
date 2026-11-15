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
use crate::{hex2, splash, ui};

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

/// `SETTINGS` → `SSB UPDATE`: unlock the bootloader region now and park with no app, so the
/// FSB's power-on update can reflash the SSB after the user power-cycles.
///
/// This runs only from the menu, never from a stored flag: the settings sector sits outside
/// the protected region, so a flag there is one any app could set. The lock-down is armed at
/// hand-off and cleared by a power cycle, so after a cold boot the unlock succeeds. After a
/// warm reset the lock-down is still armed and the unlock is refused: the user is told to
/// power-cycle and retry, and BACK returns to the menu.
#[cfg(feature = "flash-protect")]
pub(crate) async fn ssb_update_now() {
    use deluge_bsp::flash::{self, protect};

    let id = flash::identify();
    // SAFETY: the loader runs from SRAM; on success nothing is launched after this.
    let unlocked = unsafe { protect::unprotect(id) };
    info!(
        "Flash protect: SSB update unlock from the menu ({:?})",
        unlocked
    );
    if unlocked.is_ok() {
        ui::show_message(b"SSB UPDATE READY", b"POWER-CYCLE NOW").await;
        loop {
            embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
        }
    }
    ui::show_message(b"SSB UNLOCK REFUSED", b"POWER-CYCLE, RETRY").await;
    BACK_PRESSED.store(false, Ordering::Release);
    while !BACK_PRESSED.swap(false, Ordering::AcqRel) {
        embassy_time::Timer::after(embassy_time::Duration::from_millis(50)).await;
    }
}

/// Store a selected SD `/APPS` ELF into the flash app slot.
///
/// Opens the file, flattens its `PT_LOAD` segments into the SDRAM staging window
/// ([`elf::flatten_to_flash_staging`]), puts a DAPP header on it (load address from the
/// flattened image's FSB metadata, as is its entry), and programs it into the slot
/// ([`program_with_progress`]).  Progress and the final result are shown on the OLED.  On
/// success the next boot-menu pass re-probes the slot and the image appears as the default
/// `BOOT FLASH` entry — no extra wiring needed.
///
/// Returns to the menu on any error; the slot is only ever touched after the image passes
/// validation, so a bad ELF cannot brick an existing image.
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

    // SAFETY: the staging window holds `stage.len` flattened bytes until the next launch or
    // store, and nothing else runs meanwhile.
    let body = unsafe { core::slice::from_raw_parts(stage.ptr, stage.len) };
    let entry = match deluge_image::elf::validate_fsb_metadata(body, stage.code_start) {
        Ok(meta) => meta.entry,
        Err(e) => {
            error!("Flash store: no boot header: {:?}", e);
            ui::show_message(b"FLASH ERROR", b"BAD IMAGE").await;
            Timer::after(Duration::from_secs(2)).await;
            return;
        }
    };
    if body.len() as u32 > flashboot::MAX_BODY {
        ui::show_message(b"FLASH ERROR", b"TOO LARGE").await;
        Timer::after(Duration::from_secs(2)).await;
        return;
    }
    let header = deluge_image::app::encode_app_header(
        body,
        deluge_image::slot::VARIANT_OLED,
        stage.code_start,
        entry,
    );

    // Program into the flash slot — second half of the progress bar.
    match program_with_progress(&header, body, label, 50).await {
        Ok(()) => {
            info!("Flash store: programmed {} bytes into the slot", stage.len);
            ui::show_message(b"FLASHED OK", b"NOW DEFAULT").await;
            Timer::after(Duration::from_secs(2)).await;
        }
        Err(e) => {
            error!("Flash store: write failed: {:?}", e);
            let line2: &[u8] = match e {
                flashboot::FlashWriteError::Verify(_) => b"VERIFY FAILED",
                flashboot::FlashWriteError::TooLarge => b"TOO LARGE",
            };
            ui::show_message(b"FLASH ERROR", line2).await;
            Timer::after(Duration::from_secs(2)).await;
        }
    }
}

/// Program `header` and `body` into the app slot, the progress bar under `label` running from
/// `from` to 100 %.
async fn program_with_progress(
    header: &[u8; deluge_image::app::APP_HEADER_LEN],
    body: &[u8],
    label: &[u8],
    from: u8,
) -> Result<(), flashboot::FlashWriteError> {
    let span = u32::from(100 - from);
    // SAFETY: the loader runs from SRAM; nothing executes from flash during the write.
    unsafe {
        flashboot::program_slot(header, body, |done, total| async move {
            let pct =
                u32::from(from) + done.saturating_mul(span).checked_div(total).unwrap_or(span);
            ui::show_progress(label, pct.min(100) as u8).await;
        })
        .await
    }
}

/// Write a validated DAPP `image` (header then body) into the flash app slot, with a progress
/// bar under `label`.
pub(crate) async fn write_flash_app(
    image: &[u8],
    label: &[u8],
) -> Result<(), flashboot::FlashWriteError> {
    let (header, body) = image.split_at(deluge_image::app::APP_HEADER_LEN);
    let header: &[u8; deluge_image::app::APP_HEADER_LEN] =
        header.try_into().expect("split at the header's length");
    program_with_progress(header, body, label, 0).await
}

/// `USB UPDATE`'s buffer: the bottom 4 MiB of SDRAM, `0x0C00_0000..0x0C40_0000`. It shares the
/// SDRAM app region with the dev-mode upload, which never runs at the same time, and is clear of
/// the launch staging window at the top of SDRAM ([`deluge_image::elf::SDRAM_STAGE_BASE`]).
///
/// # Safety
/// One user at a time: every call hands out the same window.
pub(crate) unsafe fn uf2_buffer() -> &'static mut [u8] {
    const BASE: u32 = deluge_image::elf::SDRAM_LO;
    const LEN: usize = 4 << 20;
    const _: () = assert!(BASE as usize + LEN <= deluge_image::elf::SDRAM_STAGE_BASE as usize);
    // SAFETY: SDRAM is up before the menu runs, and the window is used by `USB UPDATE` alone
    // while it runs (see above).
    unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, LEN) }
}

/// Serve `dev` (the `USB UPDATE` drive) as USB mass storage until `exit` is set.
pub(crate) async fn uf2_session<B: usb_classes::bot::BlockDevice>(
    dev: B,
    exit: &core::sync::atomic::AtomicBool,
    status: impl core::future::Future<Output = core::convert::Infallible>,
) {
    usbmsc::run_session(dev, "Deluge UF2 Update", exit, status).await
}

/// Reset the chip through the watchdog: stage-0 boots the confirmed loader slot again (or, in
/// the stock form, the stock bootloader starts the loader).
pub(crate) fn restart() -> ! {
    cortex_ar::interrupt::disable();
    // SAFETY: the loader is done; nothing else uses the watchdog from here.
    rza1_hal::wdt::Watchdog::new(unsafe { rza1_hal::peripherals::WDT::steal() }).reset_now()
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
pub(crate) type FlashApp = deluge_image::app::AppHeader;
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
    // relocation offset. Unconditional, like the relocation itself. See `deluge_settings`.
    match unsafe { deluge_settings::migrate() } {
        deluge_settings::Outcome::NotNeeded => {}
        deluge_settings::Outcome::Migrated => {
            info!("Deluge settings: migrated 0x07F000 -> 0x3D0000")
        }
        deluge_settings::Outcome::Failed => {
            warn!("Deluge settings: migration failed; the app will factory-reset")
        }
    }
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
pub(crate) async fn boot_flash_app(app: FlashApp) -> ! {
    info!(
        "Booting the flash app: load={:#010x} len={:#x} entry={:#010x}",
        app.load_addr, app.body_len, app.entry
    );
    splash::start_load().await;
    let (desc, entry) = match flashboot::launch_with_progress(&app, splash::progress).await {
        Ok(l) => l,
        Err(e) => {
            error!("The flash app failed its check: {:?}", e);
            splash::stop_pre_roll();
            // A power cycle would only auto-boot it again: holding LOAD reaches the menu.
            ui::show_message(b"FLASH APP BAD", b"HOLD LOAD AT POWER-ON").await;
            loop {
                embassy_time::Timer::after(embassy_time::Duration::from_secs(60)).await;
            }
        }
    };

    // The whole droplet, so the panel shows it through the hand-off. Must run before
    // interrupts/DMA are quiesced (see `splash::finish`).
    splash::finish().await;

    // Quiesce, then copy the body from the flash window into SRAM via the trampoline (single
    // descriptor) like the SD ELF path.
    cortex_ar::interrupt::disable();
    unsafe { quiesce_for_handoff() };
    unsafe { launcher::launch_via_trampoline(&[desc], entry) }
}

/// Store app `idx` of `apps` into the flash app slot, showing progress under `label`.
pub(crate) async fn store_flash_app(sd: &mut Sd, apps: &EntryList, idx: usize, label: &[u8]) {
    write_app_to_flash(sd, apps, idx, label).await
}

/// Load app `idx` of `apps` and launch it, the boot droplet tracking the load. Returns only if
/// it can't be loaded, having said why on the display.
pub(crate) async fn launch_elf(mut sd: Sd, apps: &EntryList, idx: usize) {
    info!("Loading: {}", apps.name(idx));
    splash::start_load().await;

    // Open the selected file from /APPS/.
    let mut file = match file_browser::open_app(&mut sd, apps, idx).await {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open app: {:?}", e);
            splash::stop_pre_roll();
            ui::show_message(b"OPEN ERROR", b"BACK TO MENU").await;
            embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
            return;
        }
    };

    // Stream-load ELF segments.  SRAM-targeting segments are staged in SDRAM;
    // SDRAM-targeting segments are written to their final addresses directly.
    let load_result =
        match unsafe { elf::load_from_sd_with_progress(&mut file, splash::progress).await } {
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
                splash::stop_pre_roll();
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

    // The whole droplet, so the panel shows it through the hand-off — even if
    // the launched app never uses the display. Must run before interrupts/DMA
    // are quiesced (see `splash::finish`).
    splash::finish().await;

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
