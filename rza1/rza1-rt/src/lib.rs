//! `rza1-rt` — the runtime for the Renesas RZ/A1L (Cortex-A9): the reset vector and bootloader
//! metadata, the reset handler, the fault handlers, the GIC IRQ entry stub and the linker script.
//!
//! Link it once per image: nothing calls into it by name, so the board crate does
//! `use rza1_rt as _;`. The image must define `main` (the reset handler calls it) and
//! `gic_dispatch` (the IRQ stub calls it; `rza1_hal::gic` provides it), and the board crate puts
//! a `memory.x` on the linker search path for the script to INCLUDE.
#![no_std]

// ARM-mode assembly; nothing to build for a host target.
#[cfg(target_os = "none")]
mod startup;
