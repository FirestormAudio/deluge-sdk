//! The loader shim (Synthstrom Deluge, RZ/A1L): what stage-0 enters in a loader's boot slot.
//!
//! A boot slot holds this shim, executing in place right after the slot's header page, and at
//! [`PAYLOAD`] from the slot's start the app-loader itself as a flat RAM image under a
//! `deluge_image::app` header. Stage-0 has already checked the whole slot body's CRC, payload
//! included. The shim:
//!
//! 1. turns on the L1 I-cache (and nothing else), so the copy runs from cache instead of
//!    refetching every instruction over the boot ROM's slow reads, as stage-0 does;
//! 2. checks the payload's header (`deluge_image::app::validate_app_header`; the body's CRC is
//!    stage-0's, which checked the whole slot body) and that it loads into the loader's SRAM
//!    (`0x2002_0000..0x2030_0000`), before trusting where it loads and enters;
//! 3. copies the payload's body to its load address (the loader's SRAM, `0x2002_0000`);
//! 4. if its slot's confirm word is still pending, starts the watchdog at its longest timeout
//!    (`P0φ/16384`, about 125 ms), so a loader that hangs before it confirms is reset and stage-0
//!    rolls back to the other slot. The loader kicks it from its first line until it confirms;
//! 5. branches to the payload's entry, the I-cache invalidated.
//!
//! On a payload it refuses it blinks the SYNC LED (P6_7) `code` times per round, forever, where
//! `code` is [`AppError::code`] (1 bad magic … 8 body CRC). [`SHIM_STATUS`] records what happened
//! for a J-Link.
//!
//! The shim leaves the boot-info word stage-0 wrote (`0x2000_0000`) alone: the loader reads it.
//! It pulls in no HAL: the register addresses come from `rza1-pac`.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

use deluge_image::app::{APP_HEADER_LEN, AppError, validate_app_header};
use deluge_image::slot::{CONFIRMED, DELUGE, VARIANT_OLED};
// `DELUGE` gives the slot length; the slot's address comes from where the shim runs.

/// Where the payload starts, from the slot's start (`deluge_loader::update::PAYLOAD`; the word
/// before it, which `tools/loader-image.sh` writes, names the slot this shim is linked for).
const PAYLOAD: u32 = 0x1000;

unsafe extern "C" {
    /// The shim's first instruction, its vector table: right after the slot's header page.
    fn _start();
}
/// The confirm word's offset in the slot's header page.
const CONFIRM_WORD: u32 = 0xFC;
/// Where the payload may load: the loader's SRAM, above the retention RAM (the shim's own RAM and
/// the boot-info word) and below the SRAM's end.
const LOAD_RANGE: core::ops::Range<u32> = 0x2002_0000..0x2030_0000;

/// Set `SCTLR.I`. The reset handler has already invalidated the I-cache; the branch below
/// invalidates it again before the loader runs.
fn enable_icache() {
    // SAFETY: privileged mode, MMU off; only the I bit changes.
    unsafe {
        asm!(
            "mrc p15, 0, {r}, c1, c0, 0",
            "orr {r}, {r}, #(1 << 12)",
            "mcr p15, 0, {r}, c1, c0, 0",
            "isb",
            r = out(reg) _,
            options(nostack),
        );
    }
}

/// `[tag, state, error, payload length, entry]` for a J-Link: the tag `b"SHIM"` once `main`
/// runs; state 1 checking, 2 entering, 3 fault; [`AppError::code`] or `0`.
#[unsafe(no_mangle)]
static mut SHIM_STATUS: [u32; 5] = [0; 5];

const STATUS_TAG: u32 = 0x4D49_4853;

// RZ/A1L GPIO port 6 (as stage-0's): the SYNC LED, P6_7.
const PSR6: *mut u32 = 0xFCFE_3118 as *mut u32;
const PM6: *mut u16 = 0xFCFE_3318 as *mut u16;
const PMC6: *mut u16 = 0xFCFE_3418 as *mut u16;
const PNOT6: *mut u16 = 0xFCFE_3718 as *mut u16;
const SYNC_LED: u16 = 1 << 7;
/// Busy-wait iterations per half-blink (stage-0's).
const BLINK_SPINS: u32 = 1_500_000;

// The watchdog (RZ/A1L HW manual §12.3): keyed 16-bit writes.
/// `WTCSR`: watchdog mode, bits 4:3 reserved as 1, `CKS` = P0φ/16384 (the longest).
const WTCSR_MODE: u16 = 1 << 6 | 0b11 << 3 | 0b111;
const WTCSR_TME: u16 = 1 << 5;
const WRCSR_RSTE: u16 = 1 << 6;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// The shim never unmasks interrupts; one reaching here is a bug, so park.
#[rza1_rt::irq]
unsafe fn irq(_icciar: u32) {
    loop {
        core::hint::spin_loop();
    }
}

fn status(i: usize, v: u32) {
    // SAFETY: only this single thread touches the status block.
    unsafe { write_volatile(&raw mut SHIM_STATUS[i], v) }
}

fn spin(n: u32) {
    for _ in 0..n {
        core::hint::spin_loop();
    }
}

/// Blink `code` short blinks, pause, repeat: the refusal, readable without a probe.
fn fault(e: AppError) -> ! {
    status(1, 3);
    status(2, e.code());
    // SAFETY: port 6's registers; drive low first, then GPIO mode, then output (stage-0's order).
    unsafe {
        write_volatile(PSR6, u32::from(SYNC_LED) << 16);
        write_volatile(PMC6, read_volatile(PMC6) & !SYNC_LED);
        write_volatile(PM6, read_volatile(PM6) & !SYNC_LED);
    }
    loop {
        for _ in 0..e.code() {
            for _ in 0..2 {
                // SAFETY: port 6's output-invert register.
                unsafe { write_volatile(PNOT6, SYNC_LED) };
                spin(BLINK_SPINS);
            }
        }
        spin(4 * BLINK_SPINS);
    }
}

/// Start the watchdog in watchdog-timer mode, resetting the LSI on overflow, at its longest
/// timeout: stopped, mode and clock set, counter cleared, reset enabled, then started (§12.4.2).
fn start_watchdog() {
    let wdt = rza1_pac::WDT;
    let (wtcsr, wtcnt, wrcsr) = (
        wdt.wtcsr().as_ptr(),
        wdt.wtcnt().as_ptr(),
        wdt.wrcsr().as_ptr(),
    );
    // SAFETY: the watchdog's registers, keyed as §12.3.4 requires; nothing else runs.
    unsafe {
        write_volatile(wtcsr, 0xA500 | WTCSR_MODE);
        write_volatile(wtcnt, 0x5A00);
        write_volatile(wrcsr, 0x5A00 | WRCSR_RSTE);
        write_volatile(wtcsr, 0xA500 | WTCSR_MODE | WTCSR_TME);
    }
}

#[rza1_rt::entry]
fn main() -> ! {
    enable_icache();
    status(0, STATUS_TAG);
    status(1, 1);
    // Wherever the shim runs: built position-independent (ROPI), `_start`'s address is computed
    // from the PC, so the same bytes work in either slot.
    let slot = _start as usize as u32 - 0x100;
    let payload_len = (DELUGE.slot_len - PAYLOAD) as usize;
    // SAFETY: the slot lies wholly inside the memory-mapped flash window, read-only here.
    let payload =
        unsafe { core::slice::from_raw_parts((slot + PAYLOAD) as *const u8, payload_len) };
    let max_body = (payload_len - APP_HEADER_LEN) as u32;
    let app = match validate_app_header(payload, &[VARIANT_OLED], max_body) {
        Ok(h) => h,
        Err(e) => fault(e),
    };
    let end = u64::from(app.load_addr) + u64::from(app.body_len);
    if !LOAD_RANGE.contains(&app.load_addr) || end > u64::from(LOAD_RANGE.end) {
        fault(AppError::Entry);
    }
    status(3, app.body_len);
    status(4, app.entry);

    let body = &payload[APP_HEADER_LEN..APP_HEADER_LEN + app.body_len as usize];
    // SAFETY: `validate_app_header` bounded the body within the payload, and the check above put
    // its destination inside LOAD_RANGE; the destination is the loader's SRAM, which
    // nothing uses yet (the shim's own RAM is the retention RAM above 0x2001_0000).
    unsafe {
        core::ptr::copy_nonoverlapping(body.as_ptr(), app.load_addr as *mut u8, body.len());
    }

    // SAFETY: the slot's header page, in the flash window.
    let confirm = unsafe { read_volatile((slot + CONFIRM_WORD) as *const u32) };
    if confirm != CONFIRMED {
        start_watchdog();
    }

    status(1, 2);
    // SAFETY: the copy is done (the D-cache is off, so it is in RAM); invalidate the I-cache so
    // no line of what was there before survives, then enter the loader, whose own reset handler
    // sets up everything from scratch.
    unsafe {
        asm!(
            "mcr p15, 0, {z}, c7, c5, 0",
            "dsb",
            "isb",
            "bx {entry}",
            z = in(reg) 0u32,
            entry = in(reg) app.entry,
            options(noreturn),
        );
    }
}
