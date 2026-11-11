//! USB **dev-mode upload** listener: a background CDC-ACM serial endpoint that
//! receives a framed ELF straight from the host (`cargo deluge run`) and loads +
//! launches it to RAM — the same hand-off the SD `/APPS/` boot path uses, but
//! sourced from USB and with no SD shuffling.
//!
//! Unlike [`super::usbmsc`], this is **not a mode the user enters**: when dev
//! mode is on, `boot_task` races [`Listener::run`] against the menu selector
//! while the boot menu is shown.  The listener draws nothing until a valid
//! upload header arrives; the moment a complete, CRC-checked image
//! is received it performs the launch itself and never returns.  A bad frame is
//! resynced and listening continues.
//!
//! ## Wire protocol (host → device, little-endian)
//! ```text
//! magic b"DLUP" | version u8 | flags u8 | len u32 | crc32 u32 | <len ELF bytes>
//! ```
//! `crc32` is the shared [`deluge_image::crc32`] of the `len` ELF bytes.  The
//! image is streamed straight to each `PT_LOAD` segment's load / SRAM-staging
//! address as it arrives, checksummed incrementally, and launched once the CRC
//! verifies.

use core::sync::atomic::Ordering;

use embassy_futures::join::join;
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, State};
use log::{info, warn};

use rza1_hal::usb::{Rusb1Driver, disconnect, init_device_mode};

use deluge_image::Crc32;
use deluge_image::elf::{MAX_PHDRS, SDRAM_HI, SDRAM_LO, StreamRouter, LoadResult};

use super::{elf, launcher};
use crate::ui;

use deluge::usb_upload;

pub struct DevUploader {
}

impl usb_upload::UploadMixin for DevUploader {
    fn set_upload_active(&self, active: bool) {
        ui::UPLOAD_ACTIVE.store(active, Ordering::Release);
    }
    async fn ui_show_message(&self, line1: &[u8], line2: &[u8]) {
        ui::show_message(line1, line2).await
    }
    async fn ui_show_progress(&self, label: &[u8], percent: u8) {
        ui::show_progress(label, percent).await
    }
    async fn handoff(&self, result: LoadResult) {
        handoff(result).await
    }
}

pub async fn run_devupload(listener : usb_upload::Listener) -> ! {
    let x : DevUploader = DevUploader {};
    listener.run(&x).await
}

/// Final handoff: tear down USB cleanly (so the host re-enumerates the app's own
/// usb-log CDC) and launch the loaded image. Mirrors the SD ELF path in
/// `boot_task` (boot logo, disable interrupts, quiesce, launch). Never returns.
async fn handoff(result: LoadResult) -> ! {
    ui::show_message(b"LAUNCHING", b"FROM USB").await;

    // Draw the boot droplet before interrupts/DMA are quiesced (must run while
    // the executor and the PIC service are still live — see `super::show_boot_logo`).
    super::show_boot_logo().await;

    // Unplug from the host: it sees a clean disconnect and re-enumerates the
    // launched app's own USB stack (e.g. the usb-log CDC for `cargo deluge run
    // --log`).
    // SAFETY: the listener's session is over: nothing drives USB0 past this point.
    disconnect(unsafe { rza1_hal::peripherals::USB0::steal() });

    cortex_ar::interrupt::disable();
    unsafe { super::quiesce_for_handoff() };

    if result.n_sram > 0 {
        unsafe {
            launcher::launch_via_trampoline(&result.sram_descs[..result.n_sram], result.entry)
        }
    } else {
        unsafe { launcher::launch(result.entry) }
    }
}
