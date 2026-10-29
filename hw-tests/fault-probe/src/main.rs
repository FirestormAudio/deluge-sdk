//! Fault probe firmware: about two seconds after reset it executes an undefined instruction, so
//! deluge-bsp's `fault-blink` handler takes over and blinks the SYNC LED (P6_7) once, pauses,
//! and repeats. Nothing is initialised first: the fault path must work from a bare reset.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// The HAL's `rt` feature supplies `gic_dispatch`, which the runtime's IRQ stub calls.
use rza1_hal as _;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[deluge_bsp::entry]
fn main() -> ! {
    // Roughly two seconds at 400 MHz with the caches off, as the fault handler's own delays are.
    for _ in 0..0x0800_0000u32 {
        core::hint::spin_loop();
    }
    // SAFETY: deliberately faults; the fault handler never returns here.
    unsafe { core::arch::asm!("udf #0", options(noreturn)) }
}
