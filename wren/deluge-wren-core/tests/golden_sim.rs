// Golden test: pins CV-output behavior of the `SlotApi`-generic binding bodies
// on the wren-sys backend. Boots the real wren-sys VM (same path as `wren/wren-web`'s `sim_boot`), runs
// a script that drives Output, and asserts the resulting CV voltage.
//
// The bindings' native state (`bindings.rs`'s `STATE`/`MIDI`/`UI`) is
// documented single-threaded-only, and `cargo test` runs `#[test]` fns in
// parallel threads by default — so every test in this file must hold `VM_LOCK`
// for its whole body; without it the two tests race over that shared state and
// one reads the other's CV. The lock is explicit at each use rather than
// `#[serial]`-style, since it upholds a documented invariant.
use deluge_wren_core::test_support::{run_and_read_cv, run_tick_read_cv};
use std::sync::{Mutex, MutexGuard};

static VM_LOCK: Mutex<()> = Mutex::new(());

/// Take the VM lock, ignoring poisoning: a panic in one test has already failed
/// that test, and must not cascade into spurious failures in the others.
fn vm_lock() -> MutexGuard<'static, ()> {
    VM_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn output_sets_cv() {
    let _guard = vm_lock();
    let cv = run_and_read_cv("var o = Output.new(0)\no.volts = 1.0\n", 0);
    assert!((cv - 1.0).abs() < 1e-6, "cv1 = {cv}");
}

// Pins the callback-dispatch path (`tick` firing a stored `Metro` callback
// handle) through the generic `SlotApi`. A metro fires every tick; each fire bumps `n` and writes it to
// CV channel 0 — after 3 ticks the CV should have advanced to at least 3.0.
#[test]
fn metro_callback_fires() {
    let _guard = vm_lock();
    let cv = run_tick_read_cv(
        "var m = Metro.new()\nvar n = 0\nm.start(Fn.new {|stage|\n  n = n + 1\n  Output.new(0).volts = n\n}, 0.001)\n",
        /*ms_per_tick*/ 1,
        /*ticks*/ 3,
        /*ch*/ 0,
    );
    assert!(cv >= 3.0, "cv after 3 ticks = {cv}");
}
