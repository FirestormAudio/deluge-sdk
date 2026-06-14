#![cfg_attr(target_os = "none", no_std)]
#![allow(dead_code)]

// Startup lives in rza1l_hal::startup. When rza1 is linked into any binary,
// startup.rs is included automatically because _start and the vector table
// are referenced by the linker script.

pub mod audio;
#[cfg(target_os = "none")]
pub mod bus;
pub mod controls;
pub mod cv_gate;
#[cfg(target_os = "none")]
pub mod encoder;
pub mod fat;
pub mod midi_gate;
pub mod oled;
pub mod pads;
pub mod pic;
pub mod scux_dvu_path;
pub mod scux_src_path;
pub mod scux_usb_tx_path;
pub mod sd;
pub mod sdram;
pub mod system;
#[cfg(target_os = "none")]
pub mod trigger_clock;
pub mod uart;
pub mod usb;

// RSPI0 arbitration between the OLED DMA path and the CV DAC now lives in
// [`bus`] as an owned, mutex-guarded resource (replacing the former
// `RSPI0_DMA_ACTIVE` spin-flag). See `docs/deluge-sdk.md` §6a.
