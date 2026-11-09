//! The Deluge app-loader's logic, host-tested.
//!
//! The loader firmware (`firmwares/app-loader`) runs this against the hardware: each module
//! here takes the hardware as a small trait, so the order of what it does — reload the
//! protection, provision, confirm the running slot, lock down before an app — is pinned by tests
//! rather than by reading the firmware.

#![cfg_attr(not(test), no_std)]

pub mod boot;
pub mod menu;
pub mod stage;
pub mod update;
