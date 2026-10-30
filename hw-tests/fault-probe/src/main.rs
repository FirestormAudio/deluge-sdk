//! Fault probe firmware: about two seconds after reset it faults, so deluge-bsp's `fault-blink`
//! handler takes over and blinks the SYNC LED (P6_7) with the fault's pulse count, pauses, and
//! repeats. Nothing else is initialised first: the fault path must work from a bare reset.
//!
//! By default the fault is an undefined instruction. The features pick the other faults and the
//! RTT dump's hard cases (see `Cargo.toml`); with `rtt` the dump goes to RTT channel 0 first.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// The HAL's `rt` feature supplies `gic_dispatch`, which the runtime's IRQ stub calls.
use rza1_hal as _;

/// A 1 MB section in I/O area 1 with nothing behind it on the Deluge, which the abort variants
/// mark as a translation fault.
#[cfg(any(feature = "pabt", feature = "dabt"))]
const FAULT_SECTION: u32 = 0x3000_0000;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[deluge_bsp::entry]
fn main() -> ! {
    #[cfg(feature = "early")]
    undefined();

    #[cfg(feature = "rtt")]
    {
        let channels = rtt_target::rtt_init! {
            up: { 0: { size: 16384, name: "Terminal", section: ".rtt_buffer" } }
            section_cb: ".rtt_buffer"
        };
        rtt_target::set_print_channel(channels.up.0);
        // Not the logger: `log::set_logger` is a compare-exchange, and exclusives need the MMU on.
        rtt_target::rprintln!("fault-probe: rtt up");
    }

    // A couple of seconds with the caches off, in the fault handler's two-instruction loop so the
    // length doesn't depend on the build's optimisation level.
    // SAFETY: a register-only countdown.
    unsafe {
        core::arch::asm!(
            "1:",
            "subs {0}, {0}, #1",
            "bne 1b",
            inout(reg) 0x0080_0000u32 => _,
            options(nomem, nostack),
        )
    };

    #[cfg(feature = "rtt-full-ring")]
    for i in 0..256 {
        rtt_target::rprintln!(
            "fault-probe: filler {:03} ................................................................",
            i
        );
    }

    #[cfg(any(feature = "pabt", feature = "dabt"))]
    // SAFETY: interrupts are still masked from reset; the flat map is built before the one section
    // changes, and nothing is cached yet (the caches are off).
    unsafe {
        rza1_hal::mmu::init_and_enable();
        // Attribute 0: descriptor type 0b00, a translation fault.
        rza1_hal::mmu::update_section(FAULT_SECTION, 0);
    }

    #[cfg(feature = "corrupt-wroff")]
    {
        unsafe extern "C" {
            static mut _SEGGER_RTT: u8;
        }
        rtt_target::rprintln!("fault-probe: corrupting WrOff");
        // Up-buffer 0's WrOff (rtt-target's control block: _SEGGER_RTT + 0x24), far past its end.
        // SAFETY: the probe is about to fault; nothing logs after this.
        unsafe { ((&raw mut _SEGGER_RTT).add(0x24) as *mut u32).write_volatile(0xFFFF_0000) };
    }

    #[cfg(feature = "pabt")]
    // SAFETY: deliberately faults; the fault handler never returns here.
    unsafe {
        core::arch::asm!("bx {0}", in(reg) FAULT_SECTION, options(noreturn))
    }
    #[cfg(feature = "dabt")]
    // SAFETY: deliberately faults; the fault handler never returns here.
    unsafe {
        core::ptr::read_volatile(FAULT_SECTION as *const u32);
    }
    #[cfg_attr(feature = "early", allow(unreachable_code))]
    undefined()
}

fn undefined() -> ! {
    // SAFETY: deliberately faults; the fault handler never returns here.
    unsafe { core::arch::asm!("udf #0", options(noreturn)) }
}
