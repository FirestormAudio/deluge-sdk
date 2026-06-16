//! Trace / debug self-test image for the probe-rs `trace-a9` fixes (RZ/A1L, Cortex-A9).
//!
//! Deliberately minimal and with a *predictable* control flow so the trace decoder
//! can be checked against ground truth. Boots via the standard `rza1l-hal` startup
//! (`_start` → `_reset_handler` → `bl main`), so it runs and resumes exactly like the
//! real firmware.
//!
//! ## What it exercises
//!
//! - **H1** (register writeback): the loop keeps simple, observable invariants in
//!   `r4`-ish locals and the counters; combined with the GDB register round-trip
//!   (`h1_registers.gdb`) this confirms r0–r14 survive a PC-writing resume.
//! - **H2** (Thumb MSR-CPSR on resume): [`thumb_busy`] / [`thumb_leaf`] / [`thumb_node`]
//!   are compiled as Thumb (`#[instruction_set(arm::t32)]`); the ARM `main` reaches
//!   them via an interworking `blx`. Halt inside any of them (e.g. a GDB breakpoint at
//!   `thumb_busy`) to land with `CPSR.T = 1`, then resume and confirm `THUMB_TICKS`
//!   keeps advancing. Pre-fix, the CPSR write on resume corrupts state and the image
//!   stalls/faults.
//! - **H3** (branch-address decode): the loop repeatedly calls the same named ARM and
//!   Thumb leaf/node functions, so `read-trace --flow --elf` must show BRANCH targets
//!   resolving to `arm_leaf`/`arm_node` (ARM, ×4 scaling) and `thumb_leaf`/`thumb_node`
//!   (Thumb, ×2 scaling + Thumb bit). Wrong scaling shows up as garbage targets.
//!
//! ## Observing progress
//!
//! Three counters live at fixed, named addresses (find them with
//! `arm-none-eabi-nm trace-selftest | grep -iE 'heartbeat|ticks'`, or in GDB
//! `print &HEARTBEAT`). Read them live with `probe-rs read` / GDB while the image runs:
//!   - `HEARTBEAT`   — bumped once per outer loop iteration
//!   - `ARM_TICKS`   — bumped inside the ARM call chain
//!   - `THUMB_TICKS` — bumped inside the Thumb call chain

#![no_std]
#![no_main]

use core::hint::black_box;
use core::panic::PanicInfo;
use core::ptr::{addr_of_mut, read_volatile, write_volatile};

// Force the startup object (vector table, _start, _reset_handler) to be linked.
use rza1l_hal as _;

// Plain volatile counters in .bss. We intentionally avoid atomics: their LDREX/STREX
// lowering can spin forever on a bare-metal Cortex-A9 if the exclusive monitor / SMP
// bit isn't configured. A debugger reads these by symbol address.
#[unsafe(no_mangle)]
static mut HEARTBEAT: u32 = 0;
#[unsafe(no_mangle)]
static mut ARM_TICKS: u32 = 0;
#[unsafe(no_mangle)]
static mut THUMB_TICKS: u32 = 0;

#[inline(always)]
fn bump(counter: *mut u32) {
    // SAFETY: each counter is a distinct, valid, 4-byte-aligned static; single-core,
    // no concurrent access.
    unsafe { write_volatile(counter, read_volatile(counter).wrapping_add(1)) }
}

#[inline(never)]
fn spin(cycles: u32) {
    let mut i = 0u32;
    while i < cycles {
        black_box(&i);
        i = i.wrapping_add(1);
    }
}

// ── ARM (A32) call chain — known symbols, ×4-scaled branch targets ───────────────

#[inline(never)]
#[unsafe(no_mangle)]
extern "C" fn arm_leaf(x: u32) -> u32 {
    bump(addr_of_mut!(ARM_TICKS));
    x.wrapping_mul(3).wrapping_add(1)
}

#[inline(never)]
#[unsafe(no_mangle)]
extern "C" fn arm_node(x: u32) -> u32 {
    arm_leaf(x).wrapping_add(arm_leaf(x ^ 0x5a5a))
}

// ── Thumb (T32) call chain — reached via interworking blx, ×2-scaled targets ─────

#[inline(never)]
#[unsafe(no_mangle)]
#[instruction_set(arm::t32)]
extern "C" fn thumb_leaf(x: u32) -> u32 {
    bump(addr_of_mut!(THUMB_TICKS));
    x.rotate_left(1).wrapping_add(7)
}

#[inline(never)]
#[unsafe(no_mangle)]
#[instruction_set(arm::t32)]
extern "C" fn thumb_node(x: u32) -> u32 {
    thumb_leaf(x).wrapping_add(thumb_leaf(x ^ 0x1234))
}

/// Longer-running Thumb loop. Halt here (e.g. a GDB breakpoint) to land in Thumb
/// state deterministically for the H2 resume test.
#[inline(never)]
#[unsafe(no_mangle)]
#[instruction_set(arm::t32)]
extern "C" fn thumb_busy(iters: u32) {
    let mut i = 0u32;
    while i < iters {
        bump(addr_of_mut!(THUMB_TICKS));
        spin(256);
        i = i.wrapping_add(1);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> ! {
    let mut acc: u32 = 0x1357_9bdf;
    loop {
        // ARM phase: two levels of ARM calls.
        acc = arm_node(acc);
        acc = black_box(acc);

        // Thumb phase: ARM -> Thumb interworking, two levels of Thumb calls.
        acc = thumb_node(acc);
        acc = black_box(acc);

        // Spend a visible chunk of time in Thumb state (H2 halt target).
        thumb_busy(64);

        bump(addr_of_mut!(HEARTBEAT));

        // Small ARM-state gap between iterations.
        spin(1024);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
