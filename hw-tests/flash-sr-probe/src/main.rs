//! Flash status-register probe for validating `deluge_bsp::flash::protect` on hardware.
//!
//! Loaded into SRAM over J-Link and never flashed. It runs exactly one stage of the
//! validation procedure, logs every status-register value over RTT, then parks. The
//! stage is chosen at build time and defaults to the read-only census:
//!
//! ```text
//! FLASH_SR_PROBE_MODE=read cargo build -p flash-sr-probe
//! ```
//!
//! | Mode        | Writes                                                          |
//! |-------------|-----------------------------------------------------------------|
//! | `read`      | nothing                                                         |
//! | `volatile`  | volatile SR1 = top 64 KB (lost at power cycle)                  |
//! | `nv-top`    | non-volatile SR1 = top 64 KB; one test page at `0x3F0000`       |
//! | `lock-top`  | top 64 KB + lock-down; a clear attempt; software reset          |
//! | `bottom`    | bottom 1 MB + lock-down; one test page at `0x040000`; as above |
//! | `unprotect` | clears the region and the lock (the recovery step)              |
//! | `restore`   | erases a test block holding exactly the probe's mark           |
//! | `volatile-combined` | a volatile two-byte `01h` (W25 / S25): checks its byte order, then resets |
//!
//! Every status-register write goes through `deluge_bsp::flash::protect`'s
//! planner, so none can carry `SRP0` or an `LB` bit. The one exception, the clear
//! attempt while locked, sends the planner's own unprotect bytes (zeros and `QE`)
//! to prove the chip refuses them. A stage stops at its first unexpected result.

#![no_std]
#![no_main]

use core::ops::Range;
use core::panic::PanicInfo;
use core::sync::atomic::AtomicBool;

use log::{error, info};

use deluge_bsp::flash::{self, FlashId, protect};
use protect::{Goal, ProtectionRecipe, SrBus};
use rza1_hal::spibsc::{self, FlashMap, WriteMethod};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Read,
    Volatile,
    NvTop,
    LockTop,
    Bottom,
    Unprotect,
    Restore,
    VolatileCombined,
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn parse(s: &str) -> Mode {
    if same(s, "read") {
        Mode::Read
    } else if same(s, "volatile") {
        Mode::Volatile
    } else if same(s, "nv-top") {
        Mode::NvTop
    } else if same(s, "lock-top") {
        Mode::LockTop
    } else if same(s, "bottom") {
        Mode::Bottom
    } else if same(s, "unprotect") {
        Mode::Unprotect
    } else if same(s, "restore") {
        Mode::Restore
    } else if same(s, "volatile-combined") {
        Mode::VolatileCombined
    } else {
        panic!(
            "FLASH_SR_PROBE_MODE: read, volatile, nv-top, lock-top, bottom, unprotect, restore or volatile-combined"
        )
    }
}

/// The stage this image runs. An unknown name fails the build.
const MODE: Mode = match option_env!("FLASH_SR_PROBE_MODE") {
    None => Mode::Read,
    Some(s) => parse(s),
};

/// Unused per the flash map: the top 128 KB, and the vacated old device-settings
/// area below the legacy settings record at `0x07F000`.
const BLANK_TOP: Range<u32> = 0x3E_0000..0x40_0000;
const BLANK_LOW: Range<u32> = 0x04_0000..0x07_F000;
/// The one page each rejection test programs, and the writable window that
/// allows it (the normal guarded driver, no forced path).
static TOP_PAGE: [Range<u32>; 1] = [0x3F_0000..0x3F_0100];
static LOW_PAGE: [Range<u32>; 1] = [0x04_0000..0x04_0100];
/// The 64 KB erase blocks holding those pages, each the only window `restore`
/// may erase.
static TOP_BLOCK: [Range<u32>; 1] = [0x3F_0000..0x40_0000];
static LOW_BLOCK: [Range<u32>; 1] = [0x04_0000..0x05_0000];

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

#[deluge_bsp::entry]
fn main() -> ! {
    let channels = rtt_target::rtt_init! {
        up: { 0: { size: 16384, name: "Terminal", section: ".rtt_buffer" } }
        section_cb: ".rtt_buffer"
    };
    rtt_target::set_print_channel(channels.up.0);
    rtt_target::init_logger_with_level(log::LevelFilter::Info);

    unsafe {
        let start = core::ptr::addr_of!(__sram_heap_start) as *mut u8;
        let size = core::ptr::addr_of!(__sram_heap_end) as usize - start as usize;
        deluge_bsp::heap::SRAM.init(start, size);
    }
    let _board = deluge_bsp::Board::new(rza1_hal::init(deluge_bsp::system::BOARD_CONFIG));
    flash::prime_capacity();

    info!("flash-sr-probe: mode {:?}", MODE);
    match run() {
        Ok(()) => info!(
            "flash-sr-probe: DONE {:?} — halt the core (J-Link `h`)",
            MODE
        ),
        Err(()) => error!(
            "flash-sr-probe: STOPPED {:?} — record the values above; write nothing more",
            MODE
        ),
    }
    loop {
        core::hint::spin_loop();
    }
}

fn run() -> Result<(), ()> {
    let id = identify()?;
    let Some(r) = protect::recipe(id) else {
        error!("unknown part: nothing will be written");
        return Err(());
    };
    report_regs("before");
    match MODE {
        Mode::Read => {
            check_blank(BLANK_TOP)?;
            check_blank(BLANK_LOW)?;
        }
        Mode::Volatile => {
            static FAULTED: AtomicBool = AtomicBool::new(false);
            step(
                "volatile top 64 KB",
                protect::apply(&mut Volatile, &FAULTED, id, Goal::top_test(false)),
            )?;
            info!("expect: after a power cycle `read` shows the `before` values again");
        }
        Mode::NvTop => {
            check_blank(BLANK_TOP)?;
            step("top 64 KB", apply(id, Goal::top_test(false)))?;
            program_test(&TOP_PAGE)?;
        }
        Mode::LockTop => {
            check_blank(BLANK_TOP)?;
            step("top 64 KB + lock", apply(id, Goal::top_test(true)))?;
            clear_attempt(r)?;
            reset_check(id)?;
        }
        Mode::Bottom => {
            check_blank(BLANK_LOW)?;
            step("bottom 1 MB + lock", apply(id, Goal::lockdown(r)))?;
            program_test(&LOW_PAGE)?;
            clear_attempt(r)?;
            reset_check(id)?;
        }
        Mode::Unprotect => {
            step("unprotect", apply(id, Goal::unprotect()))?;
        }
        Mode::Restore => restore()?,
        Mode::VolatileCombined => volatile_combined(id)?,
    }
    report_regs("after");
    Ok(())
}

/// The JEDEC ID, read three times; it must agree and name a known part.
fn identify() -> Result<FlashId, ()> {
    let id = flash::identify();
    for _ in 0..2 {
        if flash::identify().raw != id.raw {
            error!("JEDEC ID unstable");
            return Err(());
        }
    }
    info!(
        "part: {} ({:02x} {:02x} {:02x})",
        id.model, id.raw[0], id.raw[1], id.raw[2]
    );
    Ok(id)
}

fn report_regs(when: &str) {
    let (sr1, sr2) = spibsc::protection_regs();
    let (again1, again2) = spibsc::protection_regs();
    info!(
        "{}: SR1={:02x} SR2={:02x} (region={:02x} SRP0={} lock={} CMP={} QE={} LB/other={:02x}){}",
        when,
        sr1,
        sr2,
        sr1 & 0x7C,
        sr1 >> 7,
        sr2 & 1,
        (sr2 >> 6) & 1,
        (sr2 >> 1) & 1,
        sr2 & 0xBC,
        if (sr1, sr2) == (again1, again2) {
            ""
        } else {
            " UNSTABLE"
        }
    );
}

fn step(what: &str, r: Result<protect::Applied, protect::Refusal>) -> Result<(), ()> {
    match r {
        Ok(done) => {
            info!("{}: {:?}", what, done);
            Ok(())
        }
        Err(why) => {
            error!("{}: refused, {:?}", what, why);
            Err(())
        }
    }
}

fn apply(id: FlashId, goal: Goal) -> Result<protect::Applied, protect::Refusal> {
    // SAFETY: the probe runs from SRAM.
    let mut bus = unsafe { protect::Spibsc::new() };
    protect::apply(&mut bus, &protect::FAULTED, id, goal)
}

/// Every byte of `range` reads `FF` (through the uncached mirror).
fn check_blank(range: Range<u32>) -> Result<(), ()> {
    for off in range.clone() {
        if read(off) != 0xFF {
            error!(
                "{:#08x}..{:#08x} is not blank (byte at {:#08x}); stopping",
                range.start, range.end, off
            );
            return Err(());
        }
    }
    info!("{:#08x}..{:#08x} blank", range.start, range.end);
    Ok(())
}

fn read(offset: u32) -> u8 {
    let addr = spibsc::SPI_FLASH_BASE + offset + rza1_hal::UNCACHED_MIRROR_OFFSET as u32;
    // SAFETY: a volatile byte read of the memory-mapped flash mirror, inside the chip.
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

/// Program 16 zero bytes at the window's start through the normal guarded driver;
/// a protected block must leave them `FF`.
fn program_test(window: &'static [Range<u32>; 1]) -> Result<(), ()> {
    let map = FlashMap {
        sector_size: flash::SECTOR_SIZE,
        page: flash::PAGE,
        writable: window,
    };
    let at = window[0].start;
    // SAFETY: running from SRAM; the window is one unused page.
    unsafe { map.program_page(at, &[0u8; protect::TEST_MARK_LEN]) };
    if (0..protect::TEST_MARK_LEN as u32).all(|i| read(at + i) == 0xFF) {
        info!("program at {:#08x}: rejected (protected)", at);
        Ok(())
    } else {
        error!(
            "program at {:#08x}: WRITTEN — the block is NOT protected",
            at
        );
        Err(())
    }
}

/// The test block at `window`, through the uncached mirror.
fn block(window: &[Range<u32>; 1]) -> &'static [u8] {
    let w = &window[0];
    let addr = spibsc::SPI_FLASH_BASE + w.start + rza1_hal::UNCACHED_MIRROR_OFFSET as u32;
    // SAFETY: a read-only view of the memory-mapped flash mirror, inside the chip;
    // nothing writes the flash while it is in use.
    unsafe { core::slice::from_raw_parts(addr as *const u8, (w.end - w.start) as usize) }
}

/// Erase each test block that holds exactly the probe's mark, after checking
/// that protection is off and that neither block holds anything else.
fn restore() -> Result<(), ()> {
    let (sr1, sr2) = spibsc::protection_regs();
    if protect::protection_bits(sr1, sr2) != protect::protection_bits(0, sr2 & 0x02) {
        error!(
            "protection is on (SR1={:02x} SR2={:02x}): run `unprotect` first",
            sr1, sr2
        );
        return Err(());
    }
    let blocks: [&'static [Range<u32>; 1]; 2] = [&TOP_BLOCK, &LOW_BLOCK];
    for w in blocks {
        let state = protect::test_block_state(block(w));
        info!("{:#08x}: {:?}", w[0].start, state);
        if state == protect::TestBlock::Other {
            error!(
                "{:#08x} holds something other than the probe's mark: not touching it",
                w[0].start
            );
            return Err(());
        }
    }
    for w in blocks {
        if protect::test_block_state(block(w)) != protect::TestBlock::ProbeMark {
            continue;
        }
        let map = FlashMap {
            sector_size: flash::SECTOR_SIZE,
            page: flash::PAGE,
            writable: w,
        };
        // SAFETY: running from SRAM; the window is this one test block.
        unsafe { map.erase_sector(w[0].start) };
        match protect::test_block_state(block(w)) {
            protect::TestBlock::Blank => info!("{:#08x}: erased, blank again", w[0].start),
            other => {
                error!("{:#08x}: after erase {:?}", w[0].start, other);
                return Err(());
            }
        }
    }
    Ok(())
}

/// Send SR1 = top 64 KB and SR2 = `QE` as one volatile two-byte `01h` — the
/// S25's combined write — and tell from the read-back which byte arrived first;
/// then software-reset and check the registers came back. Volatile throughout:
/// nothing outlives a power cycle, and the values are chosen so that neither order
/// can carry an `LB` bit.
fn volatile_combined(id: FlashId) -> Result<(), ()> {
    if id.raw == [0x1F, 0x87, 0x01] {
        error!("the AT25 has no two-byte 01h");
        return Err(());
    }
    let before = spibsc::protection_regs();
    let qe = before.1 & 0x02;
    if protect::protection_bits(before.0, before.1) != protect::protection_bits(0, qe) {
        error!(
            "start from an unprotected chip (SR1={:02x} SR2={:02x})",
            before.0, before.1
        );
        return Err(());
    }
    let (sr1, sr2) = Goal::top_test(false).target(before.1);
    // SAFETY: running from SRAM; a volatile write of region and QE bits only.
    unsafe { spibsc::write_protection_volatile(WriteMethod::Combined, Some(sr1), Some(sr2)) };
    let after = spibsc::protection_regs();
    let got = protect::protection_bits(after.0, after.1);
    let order = if got == protect::protection_bits(sr1, sr2) {
        "SR1 first (as sent)"
    } else if got == protect::protection_bits(sr2, sr1) {
        "SWAPPED (SR2 first)"
    } else {
        "neither order"
    };
    info!(
        "sent SR1={:02x} SR2={:02x}, read SR1={:02x} SR2={:02x}: {}",
        sr1, sr2, after.0, after.1, order
    );
    // SAFETY: running from SRAM.
    unsafe { spibsc::software_reset() };
    let reset = spibsc::protection_regs();
    let restored =
        protect::protection_bits(reset.0, reset.1) == protect::protection_bits(before.0, before.1);
    info!(
        "after software reset: SR1={:02x} SR2={:02x} (restored={})",
        reset.0, reset.1, restored
    );
    if got == protect::protection_bits(sr1, sr2) && restored {
        Ok(())
    } else {
        Err(())
    }
}

/// With the lock-down armed, send the unprotect bytes anyway: the registers must
/// not change.
fn clear_attempt(r: ProtectionRecipe) -> Result<(), ()> {
    let before = spibsc::protection_regs();
    let (sr1, sr2) = Goal::unprotect().target(before.1);
    // SAFETY: running from SRAM; the bytes are the planner's (zeros and QE).
    unsafe { spibsc::write_protection(r.method(), Some(sr1), Some(sr2)) };
    let after = spibsc::protection_regs();
    if protect::protection_bits(after.0, after.1) == protect::protection_bits(before.0, before.1) {
        info!("clear while locked: rejected (lock holds)");
        Ok(())
    } else {
        error!(
            "clear while locked: CHANGED {:02x?} -> {:02x?} — the lock-down did not hold",
            before, after
        );
        Err(())
    }
}

/// Clock out `66h 99h` and report whether the lock-down survived. It must on the
/// AT25 and W25; the S25FL132K's datasheet says it clears there (note 33).
fn reset_check(id: FlashId) -> Result<(), ()> {
    // SAFETY: running from SRAM.
    unsafe { spibsc::software_reset() };
    let (_, sr2) = spibsc::protection_regs();
    let locked = sr2 & 1 != 0;
    let expect_locked = id.raw != [0x01, 0x40, 0x16];
    info!(
        "after software reset: locked={} (expected {})",
        locked, expect_locked
    );
    report_regs("after reset");
    if locked == expect_locked {
        info!("expect: after a power cycle `read` shows lock=0 with the region still set");
        Ok(())
    } else {
        error!("software reset: the lock-down did not behave as the datasheet says");
        Err(())
    }
}

/// The volatile status-register path, for the transfer-encoding check.
struct Volatile;

impl SrBus for Volatile {
    fn read_id(&mut self) -> [u8; 3] {
        spibsc::read_id()
    }
    fn read_regs(&mut self) -> (u8, u8) {
        spibsc::protection_regs()
    }
    fn write(&mut self, method: protect::WriteMethod, sr1: Option<u8>, sr2: Option<u8>) -> bool {
        // SAFETY: the probe runs from SRAM.
        unsafe { spibsc::write_protection_volatile(protect::hal_method(method), sr1, sr2) }
    }
    fn write_volatile(
        &mut self,
        method: protect::WriteMethod,
        sr1: Option<u8>,
        sr2: Option<u8>,
    ) -> bool {
        self.write(method, sr1, sr2)
    }
    fn software_reset(&mut self) {
        // SAFETY: the probe runs from SRAM.
        unsafe { spibsc::software_reset() }
    }
}
