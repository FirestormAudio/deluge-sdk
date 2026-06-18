//! First-stage-bootloader flasher for the Synthstrom Deluge (RZ/A1L).
//!
//! A standalone **recovery tool**: loaded directly into SRAM over SWD/JTAG
//! (J-Link) and run from there — never executed from flash — so it can take the
//! SPI flash bus into manual mode and erase/program **sector 0**, the first-stage
//! bootloader (FSB) region that the normal `spibsc` driver deliberately refuses
//! to touch.
//!
//! On start it:
//!   1. Initialises the platform (clocks, MMU, caches, SDRAM, GIC, OSTM) and the
//!      OLED (via the PIC link).
//!   2. Confirms the SPI flash responds (`spibsc::read_id`), which also primes the
//!      anti-aliasing chip-capacity guard.
//!   3. Mounts the SD card and reads `BOOT.BIN` from the card root into an SDRAM
//!      staging buffer.
//!   4. Erases the FSB sector and programs the image at flash offset `0x0`.
//!   5. Verifies the write through the uncached flash mirror and reports the
//!      result on the OLED, then halts (operator power-cycles).
//!
//! ## Danger
//! This overwrites the first-stage bootloader. A bad or interrupted write bricks
//! the unit until it is reflashed over JTAG/SPI. Only run it with a recovery
//! probe attached. See `DelugeBootloader/README.md`.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use core::panic::PanicInfo;

use embassy_executor::{Executor, Spawner};
use log::{error, info, warn};

use deluge_bsp::oled::text::draw_str;
use deluge_bsp::oled::{self, FrameBuffer, WIDTH};
use deluge_bsp::{fat, flash, sd};
use rza1l_hal::{allocator, spibsc};

/// Fixed 8.3 filename the FSB image is read from (card root).
const IMAGE_NAME: &str = "BOOT.BIN";

/// The FSB occupies the reserved region below the device-settings sector at
/// `0x40000`; an image must fit within it (256 KB = 4 × 64 KB erase blocks) so we
/// never spill into settings.  (This is the FSB region size, not the erase-block
/// size — the two used to coincide at 256 KB but the erase block is now 64 KB.)
const MAX_IMAGE_LEN: usize = 0x0004_0000;

/// SDRAM staging buffer base (CS3). The image is read here before programming;
/// 256 KB easily fits in the 64 MB SDRAM.
const STAGING_ADDR: usize = 0x0C00_0000;

/// Top padding (px). The Deluge OLED panel's top 5 rows sit off-screen.
const TOP_PAD: usize = 5;

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

static mut EXECUTOR: core::mem::MaybeUninit<Executor> = core::mem::MaybeUninit::uninit();

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    #[cfg(feature = "rtt")]
    {
        let channels = rtt_target::rtt_init! {
            up: { 0: { size: 16384, name: "Terminal", section: ".rtt_buffer" } }
            section_cb: ".rtt_buffer"
        };
        rtt_target::set_print_channel(channels.up.0);
        rtt_target::init_logger_with_level(log::LevelFilter::Debug);
    }
    info!("bootloader-flasher: starting");

    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        allocator::SRAM.init(start, size);
    }

    unsafe { deluge_bsp::system::init_clocks() };
    info!("platform: clocks, MMU, cache, SDRAM, GIC ready");

    unsafe { allocator::SDRAM.init(STAGING_ADDR as *mut u8, 64 * 1024 * 1024) };

    // SCIF1 (PIC32 link): pins, baud, TX, and the DMA-RX ring. Must run before
    // the executor starts (global IRQs still masked) or the OLED chip-select
    // handshake in `oled::init` can never be received.
    unsafe { deluge_bsp::uart::init_pic(31_250) };

    // RSPI0 bring-up — shared between OLED (8-bit) and CV DAC (32-bit). This is
    // the function that puts RSPI0 into master mode (SPCR.MSTR/SPE + baud);
    // oled::init()'s enter_8bit() only sets the frame mode and assumes the
    // channel is already enabled. Without this, OLED send8 spins forever on
    // SPTEF (the TX FIFO fills but never clocks out). Must run before the
    // executor, matching every other firmware.
    unsafe { deluge_bsp::cv_gate::init() };
    info!("RSPI0: initialised via cv_gate::init");

    #[allow(static_mut_refs)]
    let executor = unsafe {
        EXECUTOR.write(Executor::new());
        EXECUTOR.assume_init_mut()
    };
    executor.run(|spawner| {
        spawner.spawn(flash_task(spawner).unwrap());
    });
}

/// PIC receive pump. Owns `pic::init()` and forwards the OLED chip-select echo so
/// `oled::init()` / `send_frame()` can complete. The flasher needs no buttons, so
/// only the OLED events are handled.
#[embassy_executor::task]
async fn pic_rx_task() {
    use deluge_bsp::pic::{self, Event};

    pic::init().await;

    let mut parser = pic::Parser::new();
    loop {
        let byte = rza1l_hal::uart::read_byte(deluge_bsp::uart::PIC_CH).await;
        match parser.push(byte) {
            Some(Event::OledSelected) => pic::notify_oled_selected(),
            Some(Event::OledDeselected) => pic::notify_oled_deselected(),
            _ => {}
        }
    }
}

#[embassy_executor::task]
async fn flash_task(spawner: Spawner) {
    use embassy_time::{Duration, Timer};

    // Bring up interrupts, then the PIC pump and the OLED (see app-loader).
    unsafe { cortex_ar::interrupt::enable() };
    spawner.spawn(pic_rx_task().unwrap());
    deluge_bsp::pic::wait_ready().await;
    oled::init().await;
    info!("OLED: ready");

    match run().await {
        Ok(len) => {
            info!("flash: wrote and verified {} bytes of {}", len, IMAGE_NAME);
            show_message(b"BOOTLOADER", b"FLASHED OK").await;
        }
        Err(e) => {
            error!("flash: {}", e.log());
            show_message(b"FLASH FAILED", e.line()).await;
        }
    }

    // Done — hold the result on screen. The operator resets / power-cycles.
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Failure modes, each with a short OLED line and a longer log string.
enum FlashErr {
    NoFlash,
    SdInit,
    Fat,
    NotFound,
    Empty,
    TooBig,
    Verify,
}

impl FlashErr {
    fn line(&self) -> &'static [u8] {
        match self {
            FlashErr::NoFlash => b"NO SPI FLASH",
            FlashErr::SdInit => b"NO SD CARD",
            FlashErr::Fat => b"BAD FILESYSTEM",
            FlashErr::NotFound => b"NO BOOT.BIN",
            FlashErr::Empty => b"EMPTY FILE",
            FlashErr::TooBig => b"FILE TOO BIG",
            FlashErr::Verify => b"VERIFY FAILED",
        }
    }
    fn log(&self) -> &'static str {
        match self {
            FlashErr::NoFlash => "SPI flash did not respond to RDID",
            FlashErr::SdInit => "SD card init failed",
            FlashErr::Fat => "could not open FAT volume / root dir",
            FlashErr::NotFound => "BOOT.BIN not found in card root",
            FlashErr::Empty => "BOOT.BIN is empty",
            FlashErr::TooBig => "BOOT.BIN larger than the FSB sector (256 KB)",
            FlashErr::Verify => "read-back verify mismatch",
        }
    }
}

/// Read `BOOT.BIN`, erase the FSB sector, program it at flash offset 0, verify.
/// Returns the number of bytes written on success.
async fn run() -> Result<usize, FlashErr> {
    show_message(b"FLASH FSB", b"CHECK FLASH").await;

    // Confirm the SPI flash responds. This also primes the cached chip capacity
    // so the anti-aliasing bound on the forced write path is armed.
    let id = spibsc::read_id();
    info!("flash JEDEC id: {:02X} {:02X} {:02X}", id[0], id[1], id[2]);
    if id[0] == 0x00 || id[0] == 0xFF {
        return Err(FlashErr::NoFlash);
    }

    // Mount the SD card.
    show_message(b"FLASH FSB", b"INIT SD...").await;
    sd::init().await.map_err(|e| {
        warn!("SD: init failed: {:?}", e);
        FlashErr::SdInit
    })?;
    info!("SD: card ready (HC={})", sd::is_hc());

    let vm = fat::new_volume_manager();
    let volume = vm
        .open_raw_volume(fat::VolumeIdx(0))
        .map_err(|_| FlashErr::Fat)?;
    let root = vm.open_root_dir(volume).map_err(|_| FlashErr::Fat)?;

    let file = vm
        .open_file_in_dir(root, IMAGE_NAME, fat::Mode::ReadOnly)
        .map_err(|_| FlashErr::NotFound)?;

    // Read the whole image into the SDRAM staging buffer. embedded_sdmmc returns
    // 0 from `read` at EOF; we cap at the FSB sector size.
    let staging =
        unsafe { core::slice::from_raw_parts_mut(STAGING_ADDR as *mut u8, MAX_IMAGE_LEN) };
    let mut total = 0usize;
    loop {
        if total == MAX_IMAGE_LEN {
            // Buffer full — peek one more byte to tell "exactly full" from "too big".
            let mut extra = [0u8; 1];
            let n = vm.read(file, &mut extra).map_err(|_| FlashErr::Fat)?;
            if n != 0 {
                let _ = vm.close_file(file);
                return Err(FlashErr::TooBig);
            }
            break;
        }
        let n = vm
            .read(file, &mut staging[total..])
            .map_err(|_| FlashErr::Fat)?;
        if n == 0 {
            break;
        }
        total += n;
    }
    let _ = vm.close_file(file);

    if total == 0 {
        return Err(FlashErr::Empty);
    }
    info!("read {} bytes from {}", total, IMAGE_NAME);
    let image = &staging[..total];

    // Erase the FSB sector, then program the image at offset 0 in 4 KB chunks so
    // the OLED can show progress. `force_*` bypasses the writable-window guard
    // (keeping the chip-bounds guard) — recovery-only, enabled by the
    // `unlock-bootloader` feature.
    show_progress(b"ERASING", 0).await;
    unsafe { spibsc::force_erase_range(0, total as u32, flash::SECTOR_SIZE) };

    const CHUNK: usize = 0x1000;
    let mut off = 0usize;
    while off < total {
        let n = CHUNK.min(total - off);
        unsafe { spibsc::force_program(off as u32, &image[off..off + n], flash::PAGE) };
        off += n;
        let pct = (off * 100 / total) as u8;
        show_progress(b"WRITING", pct).await;
    }

    // Verify through the uncached mirror — the cached window or the ARM caches may
    // hold stale bytes; the uncached mirror always sees current flash.
    let mirror = spibsc::SPI_FLASH_BASE + rza1l_hal::UNCACHED_MIRROR_OFFSET as u32;
    for (i, &want) in image.iter().enumerate() {
        let got = unsafe { core::ptr::read_volatile((mirror + i as u32) as *const u8) };
        if got != want {
            error!(
                "verify mismatch at {:#x}: got {:#x} want {:#x}",
                i, got, want
            );
            return Err(FlashErr::Verify);
        }
    }

    Ok(total)
}

/// Two centred lines of text on the OLED.
async fn show_message(line1: &[u8], line2: &[u8]) {
    let mut fb = FrameBuffer::new();
    fb.fill(0x00);
    let x1 = (WIDTH.saturating_sub(line1.len() * 6)) / 2;
    draw_str(&mut fb, x1, TOP_PAD + 16, line1);
    let x2 = (WIDTH.saturating_sub(line2.len() * 6)) / 2;
    draw_str(&mut fb, x2, TOP_PAD + 28, line2);
    oled::send_frame(&fb).await;
}

/// A labelled progress bar (0–100%).
async fn show_progress(label: &[u8], percent: u8) {
    let mut fb = FrameBuffer::new();
    fb.fill(0x00);

    let title = b"FLASH FSB";
    let title_x = (WIDTH.saturating_sub(title.len() * 6)) / 2;
    draw_str(&mut fb, title_x, TOP_PAD + 8, title);

    let label_x = (WIDTH.saturating_sub(label.len() * 6)) / 2;
    draw_str(&mut fb, label_x, TOP_PAD + 18, label);

    let bar_x = 8usize;
    let bar_y = TOP_PAD + 30;
    let bar_w = WIDTH.saturating_sub(16);
    let bar_h = 10usize;
    for x in bar_x..(bar_x + bar_w) {
        fb.set_pixel(x, bar_y, true);
        fb.set_pixel(x, bar_y + bar_h - 1, true);
    }
    for y in bar_y..(bar_y + bar_h) {
        fb.set_pixel(bar_x, y, true);
        fb.set_pixel(bar_x + bar_w - 1, y, true);
    }
    let pct = core::cmp::min(percent, 100) as usize;
    let fill_w = (bar_w.saturating_sub(2) * pct) / 100;
    for y in (bar_y + 1)..(bar_y + bar_h - 1) {
        for x in (bar_x + 1)..(bar_x + 1 + fill_w) {
            fb.set_pixel(x, y, true);
        }
    }
    oled::send_frame(&fb).await;
}
