//! App loader (second-stage bootloader) for the Synthstrom Deluge (RZ/A1L).
//!
//! Loaded by the Deluge first-stage bootloader from SPI flash into SRAM at
//! `0x20020000`.  On boot it:
//!   1. Initialises the platform (MMU, caches, SDRAM, GIC, OSTM).
//!   2. Mounts the SD card FAT filesystem.
//!   3. Lists ELF application images from `/APPS/` on the card.
//!   4. If more than one image is found, presents an OLED + encoder-wheel
//!      file-selector; otherwise auto-launches the only image.
//!   5. Seeks through the selected ELF file, loads PT_LOAD segments to their
//!      physical addresses, flushes all caches, and branches to `e_entry`.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]
#![allow(incomplete_features)]
#![feature(generic_const_exprs)]

mod elf;
mod file_browser;
mod flashboot;
mod ghostfat;
mod launcher;
mod uf2;
mod ui;
mod usbmsc;

/// Seconds the GRUB-style boot menu counts down before auto-booting the default
/// entry (the on-flash firmware when present).
const BOOT_COUNTDOWN_SECS: u8 = 5;

/// Label for the synthetic menu entry that enters USB UF2 update mode.
const UF2_MENU_LABEL: &[u8] = b"UPDATE FW";

/// Label for the synthetic menu entry that enters SD-card USB mass-storage mode.
const DATA_MENU_LABEL: &[u8] = b"DATA TRANSFER";

use core::mem::MaybeUninit;
use core::sync::atomic::AtomicBool;

/// Set by `pic_rx_task` when the BACK button is pressed.  USB modes poll this to
/// exit back to the boot menu (see [`usbmsc`]).
pub(crate) static BACK_PRESSED: AtomicBool = AtomicBool::new(false);
use core::panic::PanicInfo;

use embassy_executor::{Executor, Spawner};
use log::{error, info, warn};
use rza1l_hal::allocator;

use deluge_bsp::{oled, sd};

unsafe extern "C" {
    static __sram_heap_start: u8;
    static __sram_heap_end: u8;
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("PANIC: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

static mut EXECUTOR: MaybeUninit<Executor> = MaybeUninit::uninit();

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    #[cfg(feature = "rtt")]
    {
        let channels = rtt_target::rtt_init! {
            up: {
                0: {
                    size: 16384,
                    name: "Terminal",
                    section: ".rtt_buffer"
                }
            }
            section_cb: ".rtt_buffer"
        };
        rtt_target::set_print_channel(channels.up.0);
        rtt_target::init_logger_with_level(log::LevelFilter::Debug);
    }
    info!("app-loader: starting");

    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }

    unsafe { deluge_bsp::system::init_clocks() };
    info!("platform: clocks, MMU, cache, SDRAM, GIC ready");

    // SDRAM heap initialised but not used by the bootloader itself;
    // kept for consistency with the BSP init pattern.
    unsafe { allocator::SDRAM.init(0x0C00_0000 as *mut u8, 64 * 1024 * 1024) };

    // SCIF1 (PIC32 link): configures pins, baud, TX, and — crucially — the
    // DMA-RX ring that `uart::read_byte` reads from.  Must run before the
    // executor starts (global IRQs still masked).  Without it the OLED
    // chip-select echo handshake can never be received and `oled::init` hangs.
    unsafe { deluge_bsp::uart::init_pic(31_250) };

    unsafe { deluge_bsp::cv_gate::init() };

    #[allow(static_mut_refs)]
    let executor = unsafe {
        EXECUTOR.write(Executor::new());
        EXECUTOR.assume_init_mut()
    };
    executor.run(|spawner| {
        spawner.spawn(boot_task(spawner).unwrap());
    });
}

/// PIC32 receive pump.
///
/// Owns the PIC init handshake (`pic::init`, including the 31 250 → 200 000 bps
/// baud switch) and then continuously parses incoming PIC bytes.  It must be
/// live before `oled::init()` runs: the OLED chip-select handshake
/// (`pic::wait_oled_selected`) only unblocks when the PIC's echoed SELECT /
/// DESELECT bytes (248 / 249) are decoded here and forwarded to the matching
/// `notify_*` signal.  Button presses confirm the file selector.
///
/// Runs until the bootloader hands off to the app (interrupts are disabled at
/// that point, so the underlying DMA-RX waker never fires again).
#[embassy_executor::task]
async fn pic_rx_task() {
    use core::sync::atomic::Ordering;
    use deluge_bsp::{
        controls,
        pic::{self, Event},
    };

    // Run the PIC init sequence here (mirrors the firmware's `pic_task`), then
    // parse RX.  `boot_task` waits on `pic::wait_ready()` for this to finish.
    pic::init().await;

    let mut parser = pic::Parser::new();
    loop {
        let byte = rza1l_hal::uart::read_byte(deluge_bsp::uart::PIC_CH).await;
        match parser.push(byte) {
            Some(Event::OledSelected) => pic::notify_oled_selected(),
            Some(Event::OledDeselected) => pic::notify_oled_deselected(),
            Some(Event::ButtonPress { id }) if id == controls::encoder_button::SELECT => {
                ui::CONFIRM.store(true, Ordering::Release)
            }
            // BACK exits an active USB mode back to the boot menu.
            Some(Event::ButtonPress { id }) if id == controls::button::BACK => {
                crate::BACK_PRESSED.store(true, Ordering::Release)
            }
            _ => {}
        }
    }
}

/// Stop all autonomous peripheral activity before branching to the app.
///
/// Must run after `cortex_ar::interrupt::disable()` and before the launch.
/// The app re-initialises every controller from scratch, so it only needs the
/// hardware to be *quiet*: no DMA channel still transferring into RAM, no timer
/// or GIC source able to fire.  The critical one is the SCIF1/PIC circular
/// receive DMA — see the call site.
unsafe fn quiesce_for_handoff() {
    unsafe {
        // Stop every DMA channel (covers PIC RX/TX, SD, and OLED channels).
        for ch in 0..16u8 {
            rza1l_hal::dmac::stop(ch);
        }
        // Stop the Embassy time-driver timers (OSTM0 free-run + OSTM1 alarm).
        rza1l_hal::ostm::stop(0);
        rza1l_hal::ostm::stop(1);
        // Turn the interrupt controller off entirely.
        rza1l_hal::gic::shutdown();
    }
}

#[embassy_executor::task]
async fn boot_task(spawner: Spawner) {
    // Global IRQs must be unmasked *before* the first interrupt-driven await.
    // pic::init() and oled::init() use embassy_time `Timer`s, which are woken
    // only by the OSTM1 GIC interrupt (see rza1l_hal::time_driver); with
    // CPSR.I still masked that interrupt can never be taken and the executor
    // parks in wfe() forever.  encoder::irq_init() registers the encoder GPIO
    // IRQ and, per its contract, must run before the global unmask.
    unsafe { deluge_bsp::encoder::irq_init() };
    unsafe { cortex_ar::interrupt::enable() };

    // Start the PIC receive pump *before* driving the OLED.  It owns
    // `pic::init()` and keeps parsing RX, so the OLED chip-select echo
    // handshake inside `oled::init()` / `send_frame()` has a live parser to
    // unblock it.  Wait for PIC init to finish before issuing OLED commands.
    spawner.spawn(pic_rx_task().unwrap());
    deluge_bsp::pic::wait_ready().await;

    oled::init().await;
    info!("OLED: ready");

    // Boot menu loop.  A bootable selection launches and never returns; the USB
    // modes (UF2 update / DATA TRANSFER) return here when BACK is pressed, so the
    // menu is rebuilt from fresh state (re-probing flash and re-listing the SD).
    //
    // `first_pass` gates the "nothing bootable → auto-enter UF2" shortcut to the
    // very first iteration only.  Otherwise, exiting UF2 with BACK on a unit with
    // no flash image and no usable card would immediately re-enter UF2, trapping
    // the user in an "INIT SD… ↔ UF2" flicker with no way out.
    let mut first_pass = true;
    loop {
        // Probe the on-flash firmware slot.  Independent of the SD card, so a
        // unit with valid flash firmware can boot with no card inserted.
        let flash_img = flashboot::probe();
        if flash_img.is_some() {
            info!("Flash: valid firmware image present in slot");
        }

        ui::show_message(b"DELUGE BOOT", b"INIT SD...").await;

        // Try the SD card.  A missing/failed card is not fatal.
        let sd_ok = match sd::init().await {
            Ok(()) => {
                info!("SD: card ready (HC={})", sd::is_hc());
                true
            }
            Err(e) => {
                warn!("SD: init failed: {:?}", e);
                false
            }
        };

        let mut vm = deluge_bsp::fat::new_volume_manager();
        let sd_listing = if sd_ok {
            match file_browser::list_apps(&mut vm) {
                Ok(result) => Some(result),
                Err(e) => {
                    error!("FAT: failed to list /APPS: {:?}", e);
                    None
                }
            }
        } else {
            None
        };

        // Build the menu: on-flash firmware (default) + SD `/APPS/` images, then
        // a trailing "UPDATE FW" entry and, when a card is present, a "DATA
        // TRANSFER" entry.
        let has_flash = flash_img.is_some();
        let flash_offset = has_flash as usize;
        let sd_count = sd_listing.as_ref().map_or(0, |(_, _, e)| e.len());
        let boot_total = flash_offset + sd_count; // real boot targets
        let uf2_idx = boot_total; // UF2 entry follows the boot targets
        // DATA TRANSFER only needs a *card*, not a working filesystem, and the
        // Deluge's card-detect pin is unreliable (it reads "no card" even with
        // one inserted — see the SSB RTT logs), so gating on detection hides the
        // recovery path exactly when it's needed.  Offer it unconditionally: the
        // SdBlock backend retries `sd::init` on demand when the host probes it,
        // so an unformatted/corrupt/initially-unresponsive card can still be
        // accessed or reformatted from the host; with no card the host simply
        // sees an empty drive.  This also guarantees the menu always has ≥2
        // entries, so it never silently auto-enters UF2.
        let show_data = true;
        let data_idx = uf2_idx + 1; // always valid (`show_data` is always true)
        let menu_total = boot_total + 1 + show_data as usize;

        let mut name_refs = [b"".as_slice(); file_browser::MAX_APPS + 3];
        if has_flash {
            name_refs[0] = b"BOOT FLASH";
        }
        if let Some((_, _, entries)) = sd_listing.as_ref() {
            for i in 0..sd_count {
                name_refs[flash_offset + i] = entries.display_name(i);
            }
        }
        name_refs[uf2_idx] = UF2_MENU_LABEL;
        if show_data {
            name_refs[data_idx] = DATA_MENU_LABEL;
        }

        info!(
            "Boot menu: {} boot entry(ies) (flash={}, data={})",
            boot_total, has_flash, show_data
        );

        // Countdown auto-boots the default only when there is a real boot target;
        // a single entry (UF2 only — no card, no flash) is entered immediately.
        let countdown = if boot_total >= 1 {
            BOOT_COUNTDOWN_SECS
        } else {
            0
        };
        let selected = if menu_total == 1 && first_pass {
            warn!("Nothing to boot — entering UF2 update mode");
            uf2_idx
        } else {
            ui::run_selector(&name_refs[..menu_total], 0, countdown).await
        };
        first_pass = false;

        // ---- UF2 update mode (returns on BACK) ----
        if selected == uf2_idx {
            // USB needs the GIC/timers/DMA running, so do NOT quiesce; just
            // release the FAT handles opened for the menu.
            if let Some((volume, _, entries)) = sd_listing {
                let _ = vm.close_volume(volume);
                drop(entries);
            }
            drop(vm);
            let image_len = flash_img.map_or(0, |i| i.code_end - i.code_start);
            usbmsc::run_uf2_mode(image_len).await;
            continue;
        }

        // ---- DATA TRANSFER mode (returns on BACK) ----
        if show_data && selected == data_idx {
            if let Some((volume, _, entries)) = sd_listing {
                let _ = vm.close_volume(volume);
                drop(entries);
            }
            drop(vm);
            usbmsc::run_data_transfer_mode().await;
            continue;
        }

        // ---- Flash boot path (never returns) ----
        if has_flash && selected == 0 {
            let img = flash_img.unwrap();
            info!(
                "Booting flash image: start={:#010x} end={:#010x} entry={:#010x}",
                img.code_start, img.code_end, img.entry
            );
            ui::show_message(b"BOOTING", b"FLASH FW").await;

            // Close FAT handles and quiesce, then copy from flash into SRAM via
            // the trampoline (single descriptor) like the SD ELF path.
            cortex_ar::interrupt::disable();
            if let Some((volume, _, entries)) = sd_listing {
                let _ = vm.close_volume(volume);
                drop(entries);
            }
            drop(vm);
            unsafe { quiesce_for_handoff() };

            let desc = img.desc();
            unsafe { launcher::launch_via_trampoline(&[desc], img.entry) }
        }

        // ---- SD ELF path ----
        let (volume, root, entries) =
            sd_listing.expect("sd listing must be present for an SD menu selection");
        let sd_idx = selected - flash_offset;
        let selected_entry = entries.get(sd_idx);
        let label = entries.display_name(sd_idx);
        info!("Loading: {:?}", selected_entry.name);
        ui::show_progress(label, 0).await;

        // Open the selected file from /APPS/.
        let file = match file_browser::open_app(&mut vm, root, selected_entry) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open app: {:?}", e);
                ui::show_message(b"OPEN ERROR", b"BACK TO MENU").await;
                embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
                continue;
            }
        };

        // Stream-load ELF segments.  SRAM-targeting segments are staged in SDRAM;
        // SDRAM-targeting segments are written to their final addresses directly.
        let load_result = match unsafe {
            elf::load_from_sd_with_progress(&mut vm, file, |done, total| async move {
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
                    _ => b"SEE LOG",
                };
                error!("ELF load error: {:?}", e);
                ui::show_message(b"LOAD ERROR", line2).await;
                embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
                continue;
            }
        };

        info!(
            "ELF loaded, entry = {:#010x}, {} SRAM segment(s) staged",
            load_result.entry, load_result.n_sram
        );

        // Disable interrupts and close FAT handles before we hand off.
        cortex_ar::interrupt::disable();
        let _ = vm.close_volume(volume);
        drop(vm);
        drop(entries);

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
}
