//! The debug agent hits a line breakpoint.
//!
//! `agent::debug_run` spawns a deluge VM (bindings + prelude, debugger
//! attached) on its own thread and hands back the `DebugSession` controller
//! immediately. This checks the whole path — VM boot, debugger install,
//! interpret — parks the VM thread at the requested source line.

mod common;

use std::collections::HashSet;

use wren_core::vm::DebugStop;

#[test]
fn stops_at_line_2() {
    let _g = common::VM_TEST_LOCK.lock().unwrap();

    let session = wren_web_debug::agent::debug_run(
        "var a = 1\nvar b = 2\nSystem.print(b)\n",
        vec![],
        HashSet::from([2]),
    );

    match session.wait_event() {
        DebugStop::Stopped { line, .. } => assert_eq!(line, 2),
        other => panic!("{other:?}"),
    }
}
