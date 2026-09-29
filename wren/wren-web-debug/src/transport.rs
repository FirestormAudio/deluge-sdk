//! Transport-agnostic driver loop for a [`DebugSession`].
//!
//! `wren-dap`'s `spawn_driver` couples the DAP request/response *shape* with
//! the stdio/JSON transport that carries it. This module keeps the shape
//! (`DebugCmd`/`DebugEvent`, DAP-field-named so they serialize as JSON over a
//! `SharedArrayBuffer`) but puts the transport behind a trait, so the same
//! `serve` loop runs over an in-process `mpsc` pair (native tests) or the
//! SAB worker protocol (`sab.rs`).
//!
//! # Deviation from wren-dap: no `Launch` command
//! In `wren-dap`, `DriverCommand::Launch` is what *builds* the
//! [`DebugSession`]. Here, [`debug_run`] takes the entry source directly and
//! returns a live session before `serve` is called, so `serve` takes
//! `&DebugSession` up front and there is no `DebugCmd::Launch`. The
//! equivalent of wren-dap's post-Launch `run_until_stop` runs as `serve`'s
//! first step, pumping whatever the session already has buffered (typically
//! the initial breakpoint stop) before the command loop starts.
//!
//! [`debug_run`]: crate::agent::debug_run

use serde::{Deserialize, Serialize};
use wren_core::vm::{DebugSession, DebugStop};

/// A command sent to the driver loop, mirroring wren-dap's `DriverCommand`
/// minus `Launch`/`Detach`/`Disconnect`/`ExceptionInfo`
/// (session lifecycle concerns this crate's single-VM-per-`debug_run` model
/// doesn't need — see the module docs). Field names match the DAP
/// `arguments` wren-dap reads out of the request JSON (`threadId`,
/// `frameId`, `variablesReference`, `expression`), just typed instead of
/// `serde_json::Value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum DebugCmd {
    /// DAP `continue`.
    Continue,
    /// DAP `threads`.
    Threads,
    /// DAP `stackTrace`.
    StackTrace {
        #[serde(rename = "threadId")]
        thread_id: i64,
    },
    /// DAP `scopes`.
    Scopes {
        #[serde(rename = "frameId")]
        frame_id: i64,
    },
    /// DAP `variables`.
    Variables {
        #[serde(rename = "variablesReference")]
        variables_reference: i64,
    },
    /// DAP `evaluate`.
    Evaluate {
        #[serde(rename = "frameId")]
        frame_id: i64,
        expression: String,
    },
    /// DAP `next` (step over).
    Next,
    /// DAP `stepIn`.
    StepIn,
    /// DAP `stepOut`.
    StepOut,
}

/// One DAP `Thread` body (`{ id, name }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadDto {
    pub id: i64,
    pub name: String,
}

/// One DAP `StackFrame` body, trimmed to the fields `DebugSession::stack_trace`
/// can produce (`FrameInfo { name, line, module, id }`). Where wren-dap nests
/// a `source: { name, path }` resolved through its `ModuleRegistry`, this
/// crate has no file paths, so `module` is carried as a plain field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrameDto {
    pub id: i64,
    pub name: String,
    pub line: i32,
    pub column: i32,
    pub module: String,
}

/// One DAP `Scope` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeDto {
    pub name: String,
    pub variables_reference: i64,
    pub expensive: bool,
}

/// One DAP `Variable` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableDto {
    pub name: String,
    pub value: String,
    pub variables_reference: i64,
}

/// An event emitted by the driver loop, mirroring the DAP events/response
/// bodies wren-dap writes in `spawn_driver`/`run_until_stop`. Unlike
/// wren-dap (which wraps every reply in a `response_with_seq` envelope with
/// a `request_seq`), these are just the *body* shapes — sequencing/envelope
/// concerns belong to whichever transport actually frames messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum DebugEvent {
    /// DAP `stopped`.
    Stopped {
        #[serde(rename = "threadId")]
        thread_id: i64,
        line: i32,
        reason: String,
        #[serde(rename = "allThreadsStopped")]
        all_threads_stopped: bool,
    },
    /// DAP `output`.
    Output { category: String, output: String },
    /// DAP `terminated`.
    Terminated,
    /// DAP `exited`.
    Exited {
        #[serde(rename = "exitCode")]
        exit_code: i32,
    },
    /// Response body for `DebugCmd::Threads`.
    Threads { threads: Vec<ThreadDto> },
    /// Response body for `DebugCmd::StackTrace`.
    StackTrace {
        #[serde(rename = "stackFrames")]
        stack_frames: Vec<StackFrameDto>,
        #[serde(rename = "totalFrames")]
        total_frames: usize,
    },
    /// Response body for `DebugCmd::Scopes`.
    Scopes { scopes: Vec<ScopeDto> },
    /// Response body for `DebugCmd::Variables`.
    Variables { variables: Vec<VariableDto> },
    /// Successful response body for `DebugCmd::Evaluate`.
    Evaluate {
        result: String,
        #[serde(rename = "variablesReference")]
        variables_reference: i64,
    },
    /// Failed response body for `DebugCmd::Evaluate` (wren-dap uses an
    /// `error_response` envelope; this module has no envelopes).
    EvaluateError { message: String },
}

/// A transport for driving a [`DebugSession`], decoupled from any particular
/// wire format. `recv_cmd`/`send_event` are the two primitives [`serve`]
/// needs; how commands physically arrive and events physically leave (an
/// `mpsc` channel pair for native tests, a `SharedArrayBuffer`-backed
/// protocol for the browser worker) is up to the implementation.
pub trait DebugTransport {
    /// Block until the next command arrives. Returns `None` once no more
    /// commands will ever arrive (e.g. the command channel's sender was
    /// dropped); `serve` then stops its command loop, like wren-dap's
    /// reader-thread EOF.
    fn recv_cmd(&self) -> Option<DebugCmd>;

    /// Emit an event/response to the client.
    fn send_event(&self, ev: DebugEvent);
}

/// Drive `session` to completion over `transport`: a port of wren-dap's
/// `spawn_driver` command loop plus `run_until_stop`, generalized from the
/// DAP/stdio transport to [`DebugTransport`].
///
/// Blocks until the session terminates or `transport.recv_cmd()` returns
/// `None`. Every command that can change the VM's run state
/// (`Continue`/`Next`/`StepIn`/`StepOut`) is followed by a `run_until_stop`
/// pump; every inspection command (`Threads`/`StackTrace`/`Scopes`/
/// `Variables`/`Evaluate`) emits exactly one reply and returns to `recv_cmd`.
///
/// Since `session` is already live (there is no `Launch` command; see the
/// module docs), `serve` pumps once, unconditionally, before entering the
/// command loop.
pub fn serve(session: &DebugSession, transport: &impl DebugTransport) {
    // Equivalent of wren-dap's post-`Launch` `run_until_stop`: pump whatever
    // the session has already produced (usually a breakpoint stop) before
    // waiting on commands.
    if !run_until_stop(session, transport) {
        return;
    }

    while let Some(cmd) = transport.recv_cmd() {
        match cmd {
            DebugCmd::Threads => {
                let threads = session.threads();
                // As in wren-dap, a session with no fibers reported yet
                // still has an implicit "main" thread.
                let dtos = if threads.is_empty() {
                    vec![ThreadDto { id: 1, name: "main".to_string() }]
                } else {
                    threads.into_iter().map(|t| ThreadDto { id: t.id, name: t.name }).collect()
                };
                transport.send_event(DebugEvent::Threads { threads: dtos });
            }
            DebugCmd::StackTrace { thread_id } => {
                let frames = session.stack_trace(thread_id);
                let stack_frames: Vec<StackFrameDto> = frames
                    .into_iter()
                    .map(|f| StackFrameDto { id: f.id, name: f.name, line: f.line, column: 0, module: f.module })
                    .collect();
                let total_frames = stack_frames.len();
                transport.send_event(DebugEvent::StackTrace { stack_frames, total_frames });
            }
            DebugCmd::Scopes { frame_id } => {
                let scopes = session.scopes(frame_id);
                let dtos = scopes
                    .into_iter()
                    .map(|s| ScopeDto { name: s.name, variables_reference: s.var_ref, expensive: false })
                    .collect();
                transport.send_event(DebugEvent::Scopes { scopes: dtos });
            }
            DebugCmd::Variables { variables_reference } => {
                let vars = session.variables(variables_reference);
                let dtos = vars
                    .into_iter()
                    .map(|v| VariableDto { name: v.name, value: v.value, variables_reference: v.var_ref })
                    .collect();
                transport.send_event(DebugEvent::Variables { variables: dtos });
            }
            DebugCmd::Evaluate { frame_id, expression } => {
                match session.evaluate(frame_id, &expression) {
                    Ok(out) => {
                        transport.send_event(DebugEvent::Evaluate { result: out.result, variables_reference: out.var_ref });
                    }
                    Err(message) => transport.send_event(DebugEvent::EvaluateError { message }),
                }
            }
            DebugCmd::Continue => {
                session.resume();
                if !run_until_stop(session, transport) {
                    return;
                }
            }
            DebugCmd::Next => {
                session.step_over();
                if !run_until_stop(session, transport) {
                    return;
                }
            }
            DebugCmd::StepIn => {
                session.step_in();
                if !run_until_stop(session, transport) {
                    return;
                }
            }
            DebugCmd::StepOut => {
                session.step_out();
                if !run_until_stop(session, transport) {
                    return;
                }
            }
        }
    }
}

/// Pump the session's event stream until a stop or termination (a port of
/// wren-dap's `run_until_stop`). Forwards
/// `Output` events as-is, emits `Stopped` (with line/reason from the
/// session) on a breakpoint/step hit and returns `true` (there's more to
/// drive), or emits `Terminated` + `Exited` on program end and returns
/// `false` (the session is spent; callers must stop calling into it).
fn run_until_stop(session: &DebugSession, transport: &impl DebugTransport) -> bool {
    loop {
        match session.wait_event() {
            DebugStop::Output { text, category } => {
                transport.send_event(DebugEvent::Output { category: category.to_string(), output: text });
            }
            DebugStop::Stopped { line, reason } => {
                transport.send_event(DebugEvent::Stopped {
                    thread_id: 1,
                    line,
                    reason: reason.to_string(),
                    all_threads_stopped: true,
                });
                return true;
            }
            DebugStop::Terminated => {
                transport.send_event(DebugEvent::Terminated);
                transport.send_event(DebugEvent::Exited { exit_code: 0 });
                return false;
            }
        }
    }
}
