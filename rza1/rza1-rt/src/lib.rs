//! `rza1-rt` — the runtime for the Renesas RZ/A1L (Cortex-A9): the vector table, the reset
//! handler, the fault handlers, the GIC IRQ entry stub and the linker script.
//!
//! Requires a nightly Rust toolchain (the workspace's `rust-toolchain.toml` pins one).
//!
//! Link it once per image: the board crate does `use rza1_rt as _;` and puts a `memory.x` on the
//! linker search path for the script to INCLUDE. The image defines its entry with [`entry`], and
//! optionally its fault handler with [`fault`] (the default parks the core) and its IRQ
//! dispatcher with [`irq`] (`rza1_hal::gic` provides one with its `rt` feature).
//!
//! A board's boot header (the words its first-stage loader checks) goes in the `.boot_header`
//! section, labelled `__rza1_boot_header`; link.x places it at `_start + 0x20`, straight after
//! the vector table.
#![no_std]
#![deny(missing_docs)]

// ARM-mode assembly; nothing to build for a host target.
#[cfg(target_os = "none")]
mod startup;

mod fault;
pub use fault::{FaultInfo, FaultKind};
pub use rza1_rt_macros::{entry, fault, irq};
