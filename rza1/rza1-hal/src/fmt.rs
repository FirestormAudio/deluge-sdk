//! Logging macros over `log`, `defmt` or nothing, after embassy-hal-internal's `fmt.rs`: the
//! crate logs through `trace!` … `error!`, and the `log` / `defmt` feature picks the backend. With
//! neither, the macros still evaluate (and so use) their arguments, and emit nothing.
//!
//! Format strings must suit both backends: positional `{}` arguments only (no inline `{name}`
//! captures, which `defmt` doesn't take and the no-backend arm couldn't see), and only the
//! `{:?}`/`{:x}`-style specifiers `defmt` understands.
#![macro_use]
#![allow(unused)]

#[cfg(all(feature = "defmt", feature = "log"))]
compile_error!("rza1-hal: enable at most one of the `log` and `defmt` features");

#[collapse_debuginfo(yes)]
macro_rules! trace {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::trace!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::trace!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! debug {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::debug!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::debug!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! info {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::info!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::info!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! warn {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::warn!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::warn!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}

#[collapse_debuginfo(yes)]
macro_rules! error {
    ($s:literal $(, $x:expr)* $(,)?) => {
        {
            #[cfg(feature = "log")]
            ::log::error!($s $(, $x)*);
            #[cfg(feature = "defmt")]
            ::defmt::error!($s $(, $x)*);
            #[cfg(not(any(feature = "log", feature="defmt")))]
            let _ = ($( & $x ),*);
        }
    };
}
