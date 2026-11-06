//! Raw FFI bindings to `libdeluge`, the hardware library of the Deluge's Linux userland.
//!
//! The bindings are checked in (`src/bindings.rs`, generated for 32-bit Arm Linux); the
//! `bindgen` feature regenerates them from the installed headers instead. Use
//! [`libdeluge`](https://crates.io/crates/libdeluge) for a safe interface.

#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]

#[cfg(feature = "bindgen")]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
#[cfg(not(feature = "bindgen"))]
include!("bindings.rs");
