//! Stage-0 for the Synthstrom Deluge (RZ/A1L): the first code the boot ROM runs.
//!
//! The ROM branches to the start of the SPI flash window (`0x1800_0000`) in SVC mode with
//! interrupts masked and the flash controller doing plain `03h` reads. This image executes in
//! place from there (rza1-rt's `link_xip.x`): rza1-rt's reset handler installs the vector table,
//! copies `.data` from flash and sets the stacks, then `main`:
//!
//! 1. turns on the L1 I-cache (and nothing else: the MMU and D-cache stay off), so the CRC loop
//!    runs from cache instead of refetching every instruction over the slow SPI reads;
//! 2. starts the PMU cycle counter, to time the check;
//! 3. validates boot slot A ([`deluge_image::slot::validate`]: header page, then body CRC)
//!    through the XIP window;
//! 4. on success writes the boot-info word into retention RAM and branches to the slot's entry;
//!    on failure blinks the SYNC LED (P6_7) `code` times per round, where `code` is
//!    [`SlotError::code`], forever.
//!
//! [`STAGE0_STATUS`] records what happened, for a J-Link to read without resetting the core.
//! Stage-0 never writes flash and never touches clocks, SDRAM, the MMU, the D-cache, the flash
//! controller or any peripheral but the one GPIO pin.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

use deluge_image::slot::{self, BOOT_INFO_ADDR, SLOT_A_OFFSET, SLOT_LEN, SlotError};

/// Where the SPI flash is mapped, and where the ROM enters stage-0.
const FLASH_BASE: u32 = 0x1800_0000;

/// Counts SYNC LED toggles of the fault blink. Initialised to a non-zero value so that reading
/// `0x5A5A_xxxx` over J-Link proves the reset handler copied `.data` from flash.
#[unsafe(no_mangle)]
static mut STAGE0_HEARTBEAT: u32 = 0x5A5A_0000;

/// What stage-0 did this boot, laid out for a J-Link `mem32 <STAGE0_STATUS> 8`.
#[repr(C)]
struct Stage0Status {
    /// [`STATUS_TAG`] once `main` has started.
    tag: u32,
    /// [`STATE_VALIDATING`], [`STATE_ENTERING`] or [`STATE_FAULT`].
    state: u32,
    /// [`SlotError::code`] of the refusal, or `0`.
    error: u32,
    /// CPU cycles spent in `validate` (PMCCNTR, ÷64 divider off).
    crc_cycles: u32,
    /// The raw `CPG.FRQCR`, so `crc_cycles` can be turned into time.
    frqcr: u16,
    _pad: u16,
    /// The entered slot's header `sequence`, or `0`.
    sequence: u32,
    /// The entered slot's header `body_len`, or `0`.
    body_len: u32,
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
    error: 0,
    crc_cycles: 0,
    frqcr: 0,
    _pad: 0,
    sequence: 0,
    body_len: 0,
};

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
    unsafe {
        write_volatile(&raw mut STAGE0_STATUS.error, e.code());
        write_volatile(&raw mut STAGE0_STATUS.state, STATE_FAULT);
    }
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

#[rza1_rt::entry]
fn main() -> ! {
    enable_icache();
    start_cycle_counter();
    // SAFETY: fixed GPIO and CPG registers of this SoC, and the status block; nothing else runs.
    unsafe {
        // Drive low (the LED is active-high: off) first, then hand the pin to GPIO and make it
        // an output (rza1-hal's order), so each blink is a lit pulse on a dark LED.
        write_volatile(PSR6, u32::from(SYNC_LED) << 16);
        write_volatile(PMC6, read_volatile(PMC6) & !SYNC_LED);
        write_volatile(PM6, read_volatile(PM6) & !SYNC_LED);

        write_volatile(&raw mut STAGE0_STATUS.tag, STATUS_TAG);
        write_volatile(&raw mut STAGE0_STATUS.state, STATE_VALIDATING);
        write_volatile(&raw mut STAGE0_STATUS.frqcr, read_volatile(FRQCR));
    }

    let base = FLASH_BASE + SLOT_A_OFFSET;
    // SAFETY: slot A lies wholly inside the memory-mapped flash window, which is read-only and
    // never written while stage-0 runs.
    let slot_a = unsafe { core::slice::from_raw_parts(base as *const u8, SLOT_LEN as usize) };
    let c0 = cycles();
    let result = slot::validate(slot_a);
    let c1 = cycles();
    // SAFETY: as above.
    unsafe { write_volatile(&raw mut STAGE0_STATUS.crc_cycles, c1.wrapping_sub(c0)) };

    let header = match result {
        Ok(h) => h,
        Err(e) => fault(e),
    };
    // SAFETY: the status block as above; `BOOT_INFO_ADDR` is retention RAM, which the reset
    // handler made writable; PSR6 as above.
    unsafe {
        write_volatile(&raw mut STAGE0_STATUS.sequence, header.sequence);
        write_volatile(&raw mut STAGE0_STATUS.body_len, header.body_len);
        write_volatile(&raw mut STAGE0_STATUS.state, STATE_ENTERING);
        write_volatile(BOOT_INFO_ADDR as *mut u32, slot::boot_info(0, 0));
        write_volatile(PSR6, u32::from(SYNC_LED) << 16);
        // `validate` checked the entry is 4-aligned and inside the body, so this is ARM code in
        // slot A. The slot's reset handler sets up everything it needs from scratch.
        asm!(
            "dsb",
            "isb",
            "bx {entry}",
            entry = in(reg) base + header.entry_offset,
            options(noreturn),
        );
    }
}
