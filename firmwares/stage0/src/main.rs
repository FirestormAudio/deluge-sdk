//! Stage-0 for the Synthstrom Deluge (RZ/A1L): the first code the boot ROM runs.
//!
//! The ROM branches to the start of the SPI flash window (`0x1800_0000`) in SVC mode with
//! interrupts masked and the flash controller doing plain `03h` reads. This image executes in
//! place from there (rza1-rt's `link_xip.x`): rza1-rt's reset handler installs the vector table,
//! copies `.data` from flash and sets the stacks, then `main`:
//!
//! 0. switches the flash reads to the stock bootloader's fast reads
//!    ([`stage0_set_fast_read`], run from RAM): the ROM's `03h` reads at its slow clock with the
//!    read cache off made the slot check and the shim's copy take about 1.6 s;
//! 1. turns on the L1 I-cache (and nothing else: the MMU and D-cache stay off), so the CRC loop
//!    runs from cache instead of refetching every instruction over SPI;
//! 2. starts the PMU cycle counter, to time the checks;
//! 3. reads and clears the watchdog's overflow flag (`WRCSR.WOVF`, which the watchdog's own
//!    internal reset leaves set, HW manual §12.5.5): set, the last boot was killed by the
//!    watchdog;
//! 4. parses both slots' header pages through the XIP window, then picks one
//!    ([`deluge_image::slot::select_lazy`]: the newest valid slot, falling back from a pending one
//!    the watchdog killed), CRC-checking only the bodies that choice depends on, normally just
//!    the booted slot's, with a table held in RAM;
//! 5. writes the boot-info word into retention RAM and branches
//!    to the slot's entry; with neither slot valid it blinks the SYNC LED (P6_7) `code` times
//!    per round, where `code` is slot A's [`SlotError::code`], forever.
//!
//! [`STAGE0_STATUS`] records what happened, for a J-Link to read without resetting the core.
//! Stage-0 never writes flash and never touches clocks, SDRAM, the MMU, the D-cache or any
//! peripheral but the flash controller's read mode, the watchdog's reset flag and the one GPIO
//! pin. The register addresses come from `rza1-pac`; it pulls in no HAL.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

use deluge_image::crc::{CrcTable, IEEE_TABLE, crc32_with};
use deluge_image::slot::{
    self, BOOT_INFO_ADDR, Body, Choice, DELUGE, SLOT_LEN, SlotError, SlotHeader,
};

/// Where the SPI flash is mapped, and where the ROM enters stage-0.
const FLASH_BASE: u32 = 0x1800_0000;

/// The CRC table, in RAM: the reset handler copies `.data` from flash. A table read through the
/// uncached XIP window would cost a flash read per byte checked.
#[unsafe(link_section = ".data")]
static CRC_TABLE: CrcTable = IEEE_TABLE;

/// The status block's error code for a body [`slot::select_lazy`] didn't need to read.
const UNCHECKED: u32 = 0xFF;

/// Counts SYNC LED toggles of the fault blink. Initialised to a non-zero value so that reading
/// `0x5A5A_xxxx` over J-Link proves the reset handler copied `.data` from flash.
#[unsafe(no_mangle)]
static mut STAGE0_HEARTBEAT: u32 = 0x5A5A_0000;

/// What stage-0 did this boot, laid out for a J-Link `mem32 <STAGE0_STATUS> 10`.
#[repr(C)]
struct Stage0Status {
    /// [`STATUS_TAG`] once `main` has started.
    tag: u32,
    /// [`STATE_VALIDATING`], [`STATE_ENTERING`] or [`STATE_FAULT`].
    state: u32,
    /// `1` if the watchdog had overflowed (the last boot ended in a watchdog reset), else `0`.
    wovf: u32,
    /// Slot A's and B's [`SlotError::code`], `0` for a valid slot, or [`UNCHECKED`] for a slot
    /// whose header is valid and whose body wasn't needed.
    error: [u32; 2],
    /// Slot A's and B's header `sequence`, or `0` for an invalid slot.
    sequence: [u32; 2],
    /// The boot-info word written for the entered slot, or `0`.
    choice: u32,
    /// CPU cycles spent validating the slots (PMCCNTR, ÷64 divider off).
    crc_cycles: u32,
    /// The raw `CPG.FRQCR`, so `crc_cycles` can be turned into time.
    frqcr: u16,
    _pad: u16,
}

/// `b"0GTS"` read as a little-endian word.
const STATUS_TAG: u32 = 0x5354_4730;
const STATE_VALIDATING: u32 = 1;
const STATE_ENTERING: u32 = 2;
const STATE_FAULT: u32 = 3;

#[unsafe(no_mangle)]
static mut STAGE0_STATUS: Stage0Status = Stage0Status {
    tag: 0,
    state: 0,
    wovf: 0,
    error: [0; 2],
    sequence: [0; 2],
    choice: 0,
    crc_cycles: 0,
    frqcr: 0,
    _pad: 0,
};

/// `WRCSR.WOVF` (HW manual §12.3.3).
const WOVF: u8 = 1 << 7;

/// Busy-wait iterations per half-blink, ~125 ms (about 4 toggles/s within a burst). Measured
/// with the I-cache on and the boot ROM's clocks at ~85 ns per iteration.
const BLINK_SPINS: u32 = 1_500_000;

/// CPG.FRQCR, the clock-divider register (read only, for the status block).
const FRQCR: *const u16 = 0xFCFE_0010 as *const u16;

// Port 6 registers (RZ/A1L GPIO, as rza1-hal's gpio module and its tests use them).
/// PSR6: bits 31..16 select which of bits 15..0 are written.
const PSR6: *mut u32 = 0xFCFE_3118 as *mut u32;
/// PM6: 0 = output.
const PM6: *mut u16 = 0xFCFE_3318 as *mut u16;
/// PMC6: 0 = port (GPIO) mode.
const PMC6: *mut u16 = 0xFCFE_3418 as *mut u16;
/// PNOT6: writing a 1 inverts that bit's output.
const PNOT6: *mut u16 = 0xFCFE_3718 as *mut u16;
/// P6_7, the SYNC LED.
const SYNC_LED: u16 = 1 << 7;

// SPIBSC channel 0, the flash controller (HW manual §17.4), as `rza1_hal::spibsc::set_fast_read`
// programs it. Offsets from the base.
const SPIBSC0: usize = 0x3FEF_A000;
const CMNCR: usize = 0x00;
const SSLDR: usize = 0x04;
const SPBCR: usize = 0x08;
const DRCR: usize = 0x0C;
const DRCMR: usize = 0x10;
const DREAR: usize = 0x14;
const DROPR: usize = 0x18;
const DRENR: usize = 0x1C;
const CMNSR: usize = 0x48;
const DRDMCR: usize = 0x58;
/// `DRCR.SSLN`: negate the chip select after the current access.
const DRCR_SSLN: u32 = 1 << 24;
/// `DRCR`: 2-beat bursts, the read cache on, SSL kept between accesses.
const DRCR_FAST: u32 = 1 << 16 | 1 << 8 | 1;
/// `DRCR.RCF`: flush the read cache.
const DRCR_RCF: u32 = 1 << 9;
/// `CMNSR.SSLF`: the chip select is asserted.
const CMNSR_SSLF: u32 = 1 << 1;
/// `CMNCR`'s fields set here, and their values: read mode, SFDE, idle pins Hi-Z, data sampled on
/// the even edge, SPI mode 0, one flash.
const CMNCR_MASK: u32 = 0x81FF_F37B;
const CMNCR_FAST: u32 = 0x01FF_F320;
/// `SSLDR`'s three delays (0 is one SPBCLK each).
const SSLDR_DELAYS: u32 = 0x0007_0707;
/// Polls of `CMNSR.SSLF` before going ahead anyway.
const SSL_TIMEOUT: u32 = 2_000_000;

/// Leave the boot ROM's flash reads (`03h` at its slow clock, the read cache off) for the stock
/// bootloader's: `0Bh` fast read with 8 dummy cycles, SPBR = 1, BRDV = 0 (66.67 Mbps), the read
/// cache on. The same values the loader and every stock unit run with. Measured on the bench
/// (2026-11-15): stage-0's slot check 1.14 s -> 0.17 s, power-on to the loader 1.58 s -> 0.24 s.
///
/// Runs from RAM (in `.data`, which the reset handler copied): nothing may execute from the flash
/// window while its timing changes. It touches only the controller's registers, with no calls,
/// so no instruction is fetched from flash until it returns: `tools/check-xip-image.py
/// --ram-code stage0_set_fast_read` checks the built image for that (stage-0 is built release).
///
/// # Safety
/// No flash operation in progress. Called through a pointer, so the call itself can't go by way
/// of a linker veneer in flash.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.stage0_fast_read")]
#[inline(never)]
unsafe extern "C" fn stage0_set_fast_read() {
    let reg = |off: usize| (SPIBSC0 + off) as *mut u32;
    // SAFETY: the flash controller's registers; this code runs from RAM.
    unsafe {
        write_volatile(reg(DRCR), read_volatile(reg(DRCR)) | DRCR_SSLN);
        let mut n = SSL_TIMEOUT;
        while read_volatile(reg(CMNSR)) & CMNSR_SSLF != 0 && n != 0 {
            n -= 1;
        }
        write_volatile(
            reg(CMNCR),
            (read_volatile(reg(CMNCR)) & !CMNCR_MASK) | CMNCR_FAST,
        );
        write_volatile(reg(SSLDR), read_volatile(reg(SSLDR)) & !SSLDR_DELAYS);
        write_volatile(reg(SPBCR), 1 << 8);
        write_volatile(reg(DRCR), DRCR_FAST);
        write_volatile(reg(DRCMR), 0x0B << 16);
        write_volatile(reg(DREAR), 0);
        write_volatile(reg(DROPR), 0);
        // Command, 24-bit address and data, all single-bit; dummy cycles on.
        write_volatile(reg(DRENR), 1 << 15 | 1 << 14 | 0x7 << 8);
        write_volatile(reg(DRDMCR), 7);
        write_volatile(reg(DRCR), DRCR_FAST | DRCR_RCF);
        let _ = read_volatile(reg(DRCR));
        asm!("dsb", "isb", options(nostack, preserves_flags));
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Stage-0 never unmasks interrupts; an IRQ reaching here is a bug, so park.
#[rza1_rt::irq]
unsafe fn irq(_icciar: u32) {
    loop {
        core::hint::spin_loop();
    }
}

/// Set `SCTLR.I`. The reset handler has already invalidated the I-cache; the slot's own reset
/// handler disables and invalidates it again before relying on it, so no stale line crosses the
/// branch into the slot.
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

/// Reset and start PMCCNTR, counting every CPU cycle (PMCR.E = 1, C = 1, D = 0).
fn start_cycle_counter() {
    // SAFETY: CPU-internal PMU registers, privileged mode.
    unsafe {
        asm!(
            "mcr p15, 0, {pmcr}, c9, c12, 0",
            "mcr p15, 0, {en}, c9, c12, 1",
            "isb",
            pmcr = in(reg) 0b101u32,
            en = in(reg) 1u32 << 31,
            options(nostack),
        );
    }
}

fn cycles() -> u32 {
    let c: u32;
    // SAFETY: reads PMCCNTR.
    unsafe { asm!("mrc p15, 0, {c}, c9, c13, 0", c = out(reg) c, options(nostack)) };
    c
}

fn spin(n: u32) {
    for _ in 0..n {
        core::hint::spin_loop();
    }
}

fn toggle_led() {
    // SAFETY: fixed GPIO register; the heartbeat is only ever touched here.
    unsafe {
        write_volatile(PNOT6, SYNC_LED);
        let hb = &raw mut STAGE0_HEARTBEAT;
        write_volatile(hb, read_volatile(hb).wrapping_add(1));
    }
}

/// Blink `code` short blinks, pause, repeat: the refusal, readable without a probe.
fn fault(e: SlotError) -> ! {
    // SAFETY: the status block is only ever touched by this single thread.
    unsafe { write_volatile(&raw mut STAGE0_STATUS.state, STATE_FAULT) };
    loop {
        for _ in 0..e.code() {
            toggle_led();
            spin(BLINK_SPINS);
            toggle_led();
            spin(BLINK_SPINS);
        }
        spin(4 * BLINK_SPINS);
    }
}

/// Whether the watchdog overflowed since the flag was last cleared, clearing it if so. The
/// register takes 8-bit reads and a keyed 16-bit write: `0xA5_00` clears WOVF (§12.3.4). It must
/// be cleared: while it is set, a further overflow resets nothing (§12.5.5).
fn take_overflow() -> bool {
    let wrcsr = rza1_pac::WDT.wrcsr().as_ptr() as usize;
    // SAFETY: the watchdog's reset control/status register; reading it has no side effect and
    // the clearing write touches only WOVF.
    unsafe {
        if read_volatile(wrcsr as *const u8) & WOVF == 0 {
            return false;
        }
        write_volatile(wrcsr as *mut u16, 0xA500);
    }
    true
}

/// Slot `n` (`0` = A) through the XIP window.
fn slot_bytes(n: usize) -> &'static [u8] {
    let base = FLASH_BASE + DELUGE.slot_offsets[n];
    // SAFETY: both slots lie wholly inside the memory-mapped flash window, which is read-only and
    // never written while stage-0 runs.
    unsafe { core::slice::from_raw_parts(base as *const u8, SLOT_LEN as usize) }
}

/// Record slot `n`'s check in the status block: its header check, and what was learned of its body.
fn record(n: usize, header: &Result<SlotHeader, SlotError>, body: Body) {
    let (error, sequence) = match (header, body) {
        (Err(e), _) => (e.code(), 0),
        (Ok(h), Body::Valid) => (0, h.sequence),
        (Ok(h), Body::Unchecked) => (UNCHECKED, h.sequence),
        (Ok(_), Body::Invalid(e)) => (e.code(), 0),
    };
    // SAFETY: the status block as in `fault`.
    unsafe {
        write_volatile(&raw mut STAGE0_STATUS.error[n], error);
        write_volatile(&raw mut STAGE0_STATUS.sequence[n], sequence);
    }
}

/// Slot `n`'s error for the fault blink: its header error, else its body error.
fn slot_error(header: &Result<SlotHeader, SlotError>, body: Body) -> SlotError {
    match (header, body) {
        (Err(e), _) => *e,
        (Ok(_), Body::Invalid(e)) => e,
        // `select_lazy` only reports NoSlot once every header-valid body failed.
        _ => SlotError::BadMagic,
    }
}

#[rza1_rt::entry]
fn main() -> ! {
    // SAFETY: nothing is mid-operation on the flash; the call goes through a pointer to the RAM
    // copy.
    unsafe {
        let fast_read: unsafe extern "C" fn() = core::hint::black_box(stage0_set_fast_read);
        fast_read();
    }
    enable_icache();
    start_cycle_counter();
    let wovf = take_overflow();
    // SAFETY: fixed GPIO and CPG registers of this SoC, and the status block; nothing else runs.
    unsafe {
        // Drive low (the LED is active-high: off) first, then hand the pin to GPIO and make it
        // an output (rza1-hal's order), so each blink is a lit pulse on a dark LED.
        write_volatile(PSR6, u32::from(SYNC_LED) << 16);
        write_volatile(PMC6, read_volatile(PMC6) & !SYNC_LED);
        write_volatile(PM6, read_volatile(PM6) & !SYNC_LED);

        write_volatile(&raw mut STAGE0_STATUS.tag, STATUS_TAG);
        write_volatile(&raw mut STAGE0_STATUS.state, STATE_VALIDATING);
        write_volatile(&raw mut STAGE0_STATUS.wovf, wovf as u32);
        write_volatile(&raw mut STAGE0_STATUS.frqcr, read_volatile(FRQCR));
    }

    let c0 = cycles();
    let headers = [slot::parse(slot_bytes(0)), slot::parse(slot_bytes(1))];
    let (choice, body) = slot::select_lazy(headers[0], headers[1], wovf, |n| {
        let h = headers[n as usize].as_ref().map_err(|e| *e)?;
        slot::check_body(slot_bytes(n as usize), h, |d| crc32_with(&CRC_TABLE, d))
    });
    let c1 = cycles();
    record(0, &headers[0], body[0]);
    record(1, &headers[1], body[1]);
    // SAFETY: as above.
    unsafe { write_volatile(&raw mut STAGE0_STATUS.crc_cycles, c1.wrapping_sub(c0)) };

    let ((n, reason), header) = match choice {
        Choice::Boot { slot, reason } => match headers[slot as usize] {
            Ok(h) => ((slot, reason), h),
            // `select_lazy` only boots a slot whose header parsed.
            Err(e) => fault(e),
        },
        // No slot is valid; A's error is the one blinked.
        Choice::NoSlot => fault(slot_error(&headers[0], body[0])),
    };
    let info = slot::boot_info(n, reason as u8);
    let base = FLASH_BASE + DELUGE.slot_offsets[n as usize];
    // SAFETY: the status block as above; `BOOT_INFO_ADDR` is retention RAM, which the reset
    // handler made writable; PSR6 as above.
    unsafe {
        write_volatile(&raw mut STAGE0_STATUS.choice, info);
        write_volatile(&raw mut STAGE0_STATUS.state, STATE_ENTERING);
        write_volatile(BOOT_INFO_ADDR as *mut u32, info);
        write_volatile(PSR6, u32::from(SYNC_LED) << 16);
        // `parse` checked the entry is 4-aligned and inside the body, so this is ARM code in
        // the chosen slot. The slot's reset handler sets up everything it needs from scratch.
        asm!(
            "dsb",
            "isb",
            "bx {entry}",
            entry = in(reg) base + header.entry_offset,
            options(noreturn),
        );
    }
}
