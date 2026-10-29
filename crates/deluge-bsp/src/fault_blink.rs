//! The `fault-blink` fault handler: a CPU fault blinks the SYNC LED (P6_7) in a repeating
//! pattern, as many short pulses as the fault's number, then a pause: 1 undefined instruction,
//! 2 SVC, 3 prefetch abort, 4 data abort, 5 anything else.
//!
//! It is the image's `rza1_cpu_fault_report`. An application with its own (a `#[rza1_rt::fault]`
//! handler, or a C reporter) turns the feature off, or the link fails on the duplicate symbol: an
//! SDK app turns `deluge`'s default features off (its `fault-blink` forwards here), any other
//! image deluge-bsp's.

use rza1_hal::gpio::{Level, Output};
use rza1_rt::{FaultInfo, FaultKind};

/// Busy-wait counts, the old assembly handler's: with the caches off (a fault before the MMU is
/// up), about 2 s per pulse half and 8 s between groups, as measured with `fault-probe`; much
/// shorter with the caches on.
const HALF_PULSE: u32 = 0x0080_0000;
const GROUP_GAP: u32 = 0x0200_0000;

fn pulses(kind: FaultKind) -> u32 {
    match kind {
        FaultKind::Undefined => 1,
        FaultKind::Svc => 2,
        FaultKind::PrefetchAbort => 3,
        FaultKind::DataAbort => 4,
        _ => 5,
    }
}

/// The old assembly's two-instruction loop, so the pulse is the same length in a debug build as
/// in a release one. `count` is non-zero.
fn spin(count: u32) {
    // SAFETY: a register-only countdown.
    unsafe {
        core::arch::asm!(
            "1:",
            "subs {0}, {0}, #1",
            "bne 1b",
            inout(reg) count => _,
            options(nomem, nostack),
        )
    };
}

#[rza1_rt::fault]
fn blink_status_led(info: &FaultInfo) -> ! {
    // SAFETY: the core is in the fault handler for good, with interrupts masked; whatever owned
    // P6_7 never runs again.
    let mut led = Output::new(unsafe { rza1_hal::peripherals::P6_7::steal() }, Level::Low);
    let n = pulses(info.kind);
    loop {
        for _ in 0..n {
            led.set_high();
            spin(HALF_PULSE);
            led.set_low();
            spin(HALF_PULSE);
        }
        spin(GROUP_GAP);
    }
}
