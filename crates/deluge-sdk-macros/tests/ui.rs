//! Compile-fail tests for `#[deluge::app]`.
//!
//! These exercise the macro's own validation (async / arg-count / `self` /
//! attribute parsing), which emits `compile_error!` *before* any expansion — so
//! the cases need no embedded deps and the stderr is the macro's own message.
//! Run on the host bucket only.
#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
