//! A trivial XIP boot-slot image: proof that stage-0 entered slot A.
//!
//! Linked to execute in place at `0x1801_0100`, right after slot A's header page
//! (`memory_slot_a.x`). It copies stage-0's boot-info word out of retention RAM into
//! [`SLOT_STATUS`] and clears it, turns on the I-cache, and double-blinks the SYNC LED (P6_7) so
//! it can't be mistaken for stage-0's fault blink, counting every toggle.
//!
//! Build it, wrap it in a slot header and compose it with stage-0 with `mkslot`
//! (`crates/deluge-image/examples/mkslot.rs`).

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

use deluge_image::slot::BOOT_INFO_ADDR;

/// `[heartbeat, boot info]`: the heartbeat starts at `0x51A7_0000` (proving `.data` was copied)
/// and counts toggles; the boot info is the word stage-0 left, as read before clearing it.
#[unsafe(no_mangle)]
static mut SLOT_STATUS: [u32; 2] = [0x51A7_0000, 0];

/// Busy-wait iterations per half-blink, the same as stage-0's (both run from the I-cache).
const BLINK_SPINS: u32 = 1_500_000;

// Port 6 registers, copied from stage-0 (there is no shared crate for one pin).
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

/// Interrupts are never unmasked; an IRQ reaching here is a bug, so park.
#[rza1_rt::irq]
unsafe fn irq(_icciar: u32) {
    loop {
        core::hint::spin_loop();
    }
}

fn spin(n: u32) {
    for _ in 0..n {
        core::hint::spin_loop();
    }
}

fn toggle_led() {
    // SAFETY: fixed GPIO register; the status block is only ever touched by this thread.
    unsafe {
        write_volatile(PNOT6, SYNC_LED);
        let hb = &raw mut SLOT_STATUS[0];
        write_volatile(hb, read_volatile(hb).wrapping_add(1));
    }
}

#[rza1_rt::entry]
fn main() -> ! {
    // SAFETY: `BOOT_INFO_ADDR` is retention RAM, made writable by the reset handler; the status
    // block as above; the GPIO registers are fixed; privileged mode with the MMU off, where only
    // `SCTLR.I` changes (the reset handler invalidated the I-cache).
    unsafe {
        let info = BOOT_INFO_ADDR as *mut u32;
        write_volatile(&raw mut SLOT_STATUS[1], read_volatile(info));
        write_volatile(info, 0);

        asm!(
            "mrc p15, 0, {r}, c1, c0, 0",
            "orr {r}, {r}, #(1 << 12)",
            "mcr p15, 0, {r}, c1, c0, 0",
            "isb",
            r = out(reg) _,
            options(nostack),
        );

        // Drive low (the LED is active-high: off) first, then make the pin a GPIO output.
        write_volatile(PSR6, u32::from(SYNC_LED) << 16);
        write_volatile(PMC6, read_volatile(PMC6) & !SYNC_LED);
        write_volatile(PM6, read_volatile(PM6) & !SYNC_LED);
    }
    loop {
        for _ in 0..2 {
            toggle_led();
            spin(BLINK_SPINS);
            toggle_led();
            spin(BLINK_SPINS);
        }
        spin(6 * BLINK_SPINS);
    }
}
