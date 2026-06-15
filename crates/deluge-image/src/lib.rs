//! Pure firmware-image format logic for the Deluge SSB, with **no hardware
//! dependencies** so it unit-tests on the host and is shared by the on-device
//! [`app-loader`] and the host `elf2uf2` tool — keeping the two sides of every
//! format from drifting apart.
//!
//! * [`uf2`] — the UF2 block wire format: validate/classify an incoming block
//!   ([`uf2::classify_block`]), build outgoing blocks ([`uf2::build_block`]),
//!   and track erase-on-first-touch flash sectors ([`uf2::EraseMap`]).
//! * [`elf`] — pure `PT_LOAD` classification used by the streaming SD loader:
//!   uncached-mirror resolution and SRAM-staging / SDRAM-direct decisions.
//!
//! The crate is `no_std`; `cfg(test)` pulls in `std` so the test harness works.
#![cfg_attr(not(test), no_std)]

pub mod elf;
pub mod uf2;
