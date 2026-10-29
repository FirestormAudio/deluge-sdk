//! The `fault-blink` fault handler: a CPU fault blinks the SYNC LED (P6_7) in a repeating
//! pattern, as many short pulses as the fault's number, then a pause: 1 undefined instruction,
//! 2 SVC, 3 prefetch abort, 4 data abort, 5 anything else.
//!
//! It is the image's `rza1_cpu_fault_report`. An application with its own (a `#[rza1_rt::fault]`
//! handler, or a C reporter) turns the feature off, or the link fails on the duplicate symbol.

use rza1_hal::gpio::{Level, Output};
use rza1_rt::{FaultInfo, FaultKind};

/// Busy-wait counts: about a quarter second per pulse half, a second between groups, at the
/// Deluge's 400 MHz core clock with the caches in whatever state the fault left them.
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

fn spin(count: u32) {
    for _ in 0..count {
        core::hint::spin_loop();
    }
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
