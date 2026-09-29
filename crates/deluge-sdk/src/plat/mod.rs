//! Backend selection. Each capability module delegates its backend-divergent
//! operations here; exactly one submodule compiles per build:
//!   device  — target_os = "none"                     (deluge-bsp peripherals)
//!   sim     — host, the `sim` feature                 (deluge-sim-link panel)
//!   linux   — Linux userspace, the `linux` feature    (libdeluge)
#[cfg(target_os = "none")]
mod device;
#[cfg(target_os = "none")]
pub(crate) use device::*;

#[cfg(all(not(target_os = "none"), feature = "sim"))]
mod sim;
#[cfg(all(not(target_os = "none"), feature = "sim"))]
pub(crate) use sim::*;

#[cfg(all(not(target_os = "none"), feature = "linux"))]
mod linux;
#[cfg(all(not(target_os = "none"), feature = "linux"))]
pub(crate) use linux::*;
