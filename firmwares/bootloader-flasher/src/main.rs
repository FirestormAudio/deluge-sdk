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
//!   2. Identifies the SPI flash (`flash::identify`), primes the chip-capacity
//!      guard from the JEDEC table, and clears the bottom-1 MB hardware block
//!      protection if it is armed (see below).
//!   3. Gets the image (see *Modes*).
//!   4. Erases what the image covers (`wipe`: the whole chip, checked blank) and
//!      programs the image at flash offset `0x0`.
//!   5. Verifies the write through the uncached flash mirror and reports the
//!      result on the OLED and in the log (`bootloader-flasher: DONE <n> bytes` /
//!      `bootloader-flasher: FAILED <reason>`), then halts (operator power-cycles).
//!
//! ## Modes
//! Fixed at build time by `BOOTLOADER_FLASHER_MODE` (`build.rs` refuses anything else):
//! - `fsb` (default): read `BOOT.BIN` from the SD card root into an SDRAM staging
//!   buffer; at most 256 KB, the stock FSB region.
//! - `image`: the file named by `BOOTLOADER_FLASHER_IMAGE`, embedded in this tool, so
//!   nothing is written to the SD card; at most 1 MB, the bootloader region.
//! - `wipe`: erase the **entire chip** and check it reads blank, then as `image`. The
//!   sectors above the image go first (top down) and the image's own sectors last, right
//!   before it is programmed, so a failure in the wipe still ends with the image written. Used to give a unit a pristine flash for the
//!   from-scratch boot chain (stage-0); a development unit only.
//!
//! ## Hardware write protection
//! The `unlock-bootloader` forced path bypasses only the *driver's* writable-window
//! guard. The bottom-1 MB block protection this SDK can arm (`flash::protect`) is enforced **inside the flash chip**, against any
//! SPI master — so it blocks this tool too, J-Link or not. Hence step 2: clear `BP`
//! before erasing. The one unrecoverable-from-here state is Power-Supply-Lock-Down,
//! which makes the status register read-only until the next power cycle; the tool
//! detects it and says to power-cycle rather than failing obscurely.
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

use deluge_bsp::flash;
use deluge_bsp::oled::text::draw_str;
use deluge_bsp::oled::{self, FrameBuffer, WIDTH};
#[cfg(flasher_mode = "fsb")]
use deluge_bsp::{fat, sd};
use rza1_alloc as allocator;
use rza1_hal::spibsc;

/// Fixed 8.3 filename the FSB image is read from (card root).
#[cfg(flasher_mode = "fsb")]
const IMAGE_NAME: &str = "BOOT.BIN";

/// The image the `image` and `wipe` modes program (`build.rs` checks it is 1..=1 MB).
#[cfg(not(flasher_mode = "fsb"))]
static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded.bin"));

/// The OLED title for this mode.
const TITLE: &[u8] = if cfg!(flasher_mode = "wipe") {
    b"WIPE + IMAGE"
} else if cfg!(flasher_mode = "image") {
    b"FLASH IMAGE"
} else {
    b"FLASH FSB"
};

/// The FSB occupies the reserved region below the device-settings sector at
/// `0x40000`; an image must fit within it (256 KB = 4 × 64 KB erase blocks) so we
/// never spill into settings.  (This is the FSB region size, not the erase-block
/// size — the two used to coincide at 256 KB but the erase block is now 64 KB.)
#[cfg(flasher_mode = "fsb")]
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

#[deluge_bsp::entry]
fn main() -> ! {
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

    let board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));
    info!("platform: clocks, MMU, cache, SDRAM, GIC ready");

    unsafe { allocator::SDRAM.init(STAGING_ADDR as *mut u8, 64 * 1024 * 1024) };

    // SCIF1 (PIC32 link): pins, baud, TX, and the DMA-RX ring. Must run before
    // the executor starts (global IRQs still masked) or the OLED chip-select
    // handshake in `oled::init` can never be received.
    deluge_bsp::uart::init_pic(
        board.pic_uart,
        board.pins.pic,
        board.dma.pic_rx,
        board.dma.pic_tx,
        31_250,
    );
    // The SD slot's controller, once; `sd::init` identifies the card in `run`.
    deluge_bsp::sd::init_hw(
        board.sd_card,
        board.pins.sd,
        board.dma.sd_tx,
        board.dma.sd_rx,
    );

    // The bus brings RSPI0 up (shared by the OLED and the CV DAC); without it
    // the OLED's frames never clock out. Must run before the executor, matching
    // every other firmware.
    deluge_bsp::bus::init(board.shared_spi, board.pins.spi, board.dma.oled);
    deluge_bsp::cv_gate::init(board.pins.cv_gate);

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
        let byte = pic::read_byte().await;
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
            info!("bootloader-flasher: DONE {} bytes", len);
            show_message(b"BOOTLOADER", b"FLASHED OK").await;
        }
        Err(e) => {
            error!("bootloader-flasher: FAILED {}", e.log());
            show_message(b"FLASH FAILED", e.line()).await;
        }
    }

    // Done — hold the result on screen. The operator resets / power-cycles.
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Failure modes, each with a short OLED line and a longer log string. The SD ones belong to
/// `fsb`, `NotBlank` to `wipe`; the other modes leave them unconstructed.
#[allow(dead_code)]
enum FlashErr {
    NoFlash,
    UnknownPart,
    Locked,
    Unprotect,
    SdInit,
    Fat,
    NotFound,
    Empty,
    TooBig,
    NotBlank,
    Verify,
}

impl FlashErr {
    fn line(&self) -> &'static [u8] {
        match self {
            FlashErr::NoFlash => b"NO SPI FLASH",
            FlashErr::UnknownPart => b"UNKNOWN FLASH",
            FlashErr::Locked => b"LOCKED: PWR CYCLE",
            FlashErr::Unprotect => b"UNPROTECT FAILED",
            FlashErr::SdInit => b"NO SD CARD",
            FlashErr::Fat => b"BAD FILESYSTEM",
            FlashErr::NotFound => b"NO BOOT.BIN",
            FlashErr::Empty => b"EMPTY FILE",
            FlashErr::TooBig => b"FILE TOO BIG",
            FlashErr::NotBlank => b"NOT BLANK",
            FlashErr::Verify => b"VERIFY FAILED",
        }
    }
    fn log(&self) -> &'static str {
        match self {
            FlashErr::NoFlash => "SPI flash did not respond to RDID",
            FlashErr::UnknownPart => {
                "flash JEDEC ID is not a known part - capacity unknown, writes refused"
            }
            FlashErr::Locked => {
                "bottom-1MB lock-down is armed: the status register is read-only until \
                 the next power cycle - power-cycle the unit and run this again"
            }
            FlashErr::Unprotect => "could not clear bottom-1MB block protection",
            FlashErr::SdInit => "SD card init failed",
            FlashErr::Fat => "could not open FAT volume / root dir",
            FlashErr::NotFound => "BOOT.BIN not found in card root",
            FlashErr::Empty => "BOOT.BIN is empty",
            FlashErr::TooBig => "BOOT.BIN larger than the FSB sector (256 KB)",
            FlashErr::NotBlank => "chip not blank after erase",
            FlashErr::Verify => "read-back verify mismatch",
        }
    }
}

/// Get the image, erase what it needs (`wipe`: the whole chip), program it at flash
/// offset 0, verify. Returns the number of bytes written on success.
async fn run() -> Result<usize, FlashErr> {
    show_message(TITLE, b"CHECK FLASH").await;

    // Confirm the SPI flash responds, and identify the part.
    let id = flash::identify();
    info!(
        "flash: {} (JEDEC {:02X} {:02X} {:02X})",
        id.model, id.raw[0], id.raw[1], id.raw[2]
    );
    if id.raw[0] == 0x00 || id.raw[0] == 0xFF {
        return Err(FlashErr::NoFlash);
    }

    // Prime the chip-bounds guard from the JEDEC *table*. `read_id` primes
    // nothing on its own (despite what this tool used to claim), and the
    // controller's `1 << density` fallback rejects the AT25SF321B's density byte
    // (`0x01`) as implausible and yields capacity 0 — which makes `in_chip`
    // refuse every forced write, so on that part the flasher would erase and
    // program nothing and then fail the read-back verify.
    flash::prime_capacity();
    if id.capacity == 0 {
        // Not in the table. The controller can still size parts whose density
        // byte is a true log2(size); a recovery tool should stay usable on an
        // unlisted-but-sane part. Only refuse when neither method can size it,
        // since then every write is guaranteed to be rejected anyway.
        if !(18..=26).contains(&id.raw[2]) {
            return Err(FlashErr::UnknownPart);
        }
        warn!("flash: unlisted part - relying on the 1 << density chip bound");
    }

    // Hardware block protection (`flash::protect`). `force_*` only
    // bypasses the driver's *software* window guard; BP is enforced inside the
    // flash chip against any SPI master, so a protected sector 0 is refused even
    // from a J-Link-loaded recovery tool. Clear it here, before the erase.
    // `unprotect` clears any region (not just the recipe's), verifies the
    // read-back, and writes nothing when there is nothing to clear.
    let (sr1, sr2) = spibsc::protection_regs();
    info!("flash: before unprotect SR1={:02X} SR2={:02X}", sr1, sr2);
    if flash::protect::recipe(id).is_some() {
        show_message(TITLE, b"UNPROTECT...").await;
        // SAFETY: this tool runs from SRAM, never from flash.
        match unsafe { flash::protect::unprotect(id) } {
            Ok(done) => info!("flash: block protection clear ({:?})", done),
            // The lock-down is armed: the status register is read-only until the
            // next power cycle, so retrying is futile.
            Err(flash::protect::Refusal::Locked) => return Err(FlashErr::Locked),
            Err(why) => {
                warn!("flash: unprotect refused: {:?}", why);
                return Err(FlashErr::Unprotect);
            }
        }
    }
    // A part with no recipe: protection state unknowable — proceed and let the
    // verify fail if it is in fact protected.
    let (sr1, sr2) = spibsc::protection_regs();
    info!("flash: after unprotect SR1={:02X} SR2={:02X}", sr1, sr2);

    #[cfg(flasher_mode = "fsb")]
    let image = read_sd_image().await?;
    #[cfg(not(flasher_mode = "fsb"))]
    let image = IMAGE;

    let total = image.len();

    // Wipe everything *above* the image's sectors first, so offset 0 is only erased right
    // before the image goes in below. A failed blank check is reported after the image is
    // written and verified, so the chip still boots something.
    #[cfg(flasher_mode = "wipe")]
    let wiped = {
        // Only a part whose size the JEDEC table knows: the erase must cover the whole chip.
        if id.capacity == 0 {
            return Err(FlashErr::UnknownPart);
        }
        let image_end = (total as u32).div_ceil(flash::SECTOR_SIZE) * flash::SECTOR_SIZE;
        wipe_chip(id.capacity, image_end).await
    };

    // Erase what the image covers, then program it at offset 0 in 4 KB chunks so the OLED
    // can show progress. `force_*` bypasses the writable-window guard (keeping the
    // chip-bounds guard) — recovery-only, enabled by the `unlock-bootloader` feature.
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
    for (i, &want) in image.iter().enumerate() {
        let got = unsafe { core::ptr::read_volatile((mirror() + i as u32) as *const u8) };
        if got != want {
            error!(
                "verify mismatch at {:#x}: got {:#x} want {:#x}",
                i, got, want
            );
            return Err(FlashErr::Verify);
        }
    }

    #[cfg(flasher_mode = "wipe")]
    wiped?;
    Ok(total)
}

/// The uncached mirror of flash offset 0.
fn mirror() -> u32 {
    spibsc::SPI_FLASH_BASE + rza1_hal::UNCACHED_MIRROR_OFFSET as u32
}

/// Erase every sector of a `capacity`-byte chip from `keep` (a sector boundary) up, top
/// sector first, then check every byte there reads `0xFF`. `0..keep` is left for the image.
#[cfg(flasher_mode = "wipe")]
async fn wipe_chip(capacity: u32, keep: u32) -> Result<(), FlashErr> {
    let first = keep / flash::SECTOR_SIZE;
    let sectors = capacity / flash::SECTOR_SIZE;
    for (done, s) in (first..sectors).rev().enumerate() {
        unsafe {
            spibsc::force_erase_range(
                s * flash::SECTOR_SIZE,
                flash::SECTOR_SIZE,
                flash::SECTOR_SIZE,
            )
        };
        let pct = (done as u32 + 1) * 100 / (sectors - first);
        show_progress(b"WIPING", pct as u8).await;
    }
    info!("flash: erased sectors {}..{}", first, sectors);
    for off in (keep..capacity).step_by(4) {
        let got = unsafe { core::ptr::read_volatile((mirror() + off) as *const u32) };
        if got != u32::MAX {
            error!("blank check: {:#x} reads {:#010x}", off, got);
            return Err(FlashErr::NotBlank);
        }
    }
    info!("flash: {:#x}..{:#x} blank", keep, capacity);
    Ok(())
}

/// Read `BOOT.BIN` from the SD card root into the SDRAM staging buffer.
#[cfg(flasher_mode = "fsb")]
async fn read_sd_image() -> Result<&'static [u8], FlashErr> {
    // Mount the SD card.
    show_message(TITLE, b"INIT SD...").await;
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
    Ok(&staging[..total])
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

    let title_x = (WIDTH.saturating_sub(TITLE.len() * 6)) / 2;
    draw_str(&mut fb, title_x, TOP_PAD + 8, TITLE);

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
